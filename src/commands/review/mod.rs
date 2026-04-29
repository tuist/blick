//! `blick review` — load scopes, partition the diff, fan out to concurrent
//! tasks, persist results, and print a rolled-up report.

mod base;
mod grouping;
mod labels;
mod overrides;
mod run_dir;
mod task;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};

use crate::agent::{AgentRunner, runner_for};
use crate::cli::ReviewArgs;
use crate::error::BlickError;
use crate::git::collect_diff_in;
use crate::review::{ReviewReport, render_report};
use crate::run_record::{RunManifest, TaskRef, task_filename, write_manifest};
use crate::scope::load_scopes;

use base::{resolve_base, resolve_incremental_base};
use grouping::group_changes_by_scope;
use labels::{combine_reports, scope_label_for};
use overrides::apply_agent_overrides;
use run_dir::update_latest_pointer;
use task::{TaskInput, TaskResult, emit_task_block, execute_task};

pub async fn run(args: ReviewArgs) -> Result<(), BlickError> {
    let repo_root = args
        .repo
        .clone()
        .unwrap_or_else(|| PathBuf::from("."))
        .canonicalize()?;
    let mut scopes = load_scopes(&repo_root)?;
    if scopes.is_empty() {
        return Err(BlickError::Config(format!(
            "no blick.toml found under {}",
            repo_root.display()
        )));
    }

    apply_agent_overrides(&mut scopes, args.agent, args.model.as_deref());

    let configured_base = resolve_base(&scopes, args.base.as_deref());
    let base = if args.base.is_some() {
        // An explicit `--base` is honoured verbatim; the incremental marker
        // is only consulted when blick is auto-detecting a base in CI.
        configured_base
    } else {
        resolve_incremental_base(&repo_root, &configured_base)
    };
    // The `scopes.is_empty()` guard above means `.max()` always yields a
    // value; the canonical default lives in `EffectiveDefaults::default()`
    // and is propagated through every scope by `scope::inherit::build_scope`.
    let max_diff = scopes
        .iter()
        .map(|s| s.defaults.max_diff_bytes)
        .max()
        .expect("scopes is non-empty (checked above)");
    let diff = collect_diff_in(&repo_root, &base, max_diff)?;

    if diff.diff.is_empty() {
        let report = ReviewReport::empty(format!("No tracked changes found relative to {base}."));
        println!("{}", render_report(&report, args.json)?);
        return Ok(());
    }

    let owners = group_changes_by_scope(&repo_root, &scopes, &diff);
    if owners.is_empty() {
        let report = ReviewReport::empty(format!(
            "No changed files map to a known blick.toml scope (base {base})."
        ));
        println!("{}", render_report(&report, args.json)?);
        return Ok(());
    }

    let max_concurrency = args
        .max_concurrency
        .or_else(|| scopes.iter().map(|s| s.defaults.max_concurrency).min())
        .unwrap_or(4)
        .max(1);

    let run_id = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let runs_root = repo_root.join(".blick").join("runs");
    let logs_dir = runs_root.join(&run_id);
    fs::create_dir_all(&logs_dir)?;

    let mut tasks: Vec<TaskInput> = Vec::new();
    for (scope_root, scoped_diff) in owners {
        // `group_changes_by_scope` only emits roots from `scopes`, so this
        // lookup is guaranteed to hit. Surface a structured error rather
        // than panicking, in case a future refactor breaks the invariant.
        let scope = scopes
            .iter()
            .find(|s| s.root == scope_root)
            .ok_or_else(|| {
                BlickError::Config(format!(
                    "internal error: owner scope {} not in loaded scopes",
                    scope_root.display()
                ))
            })?
            .clone();
        let runner: Arc<dyn AgentRunner> = Arc::from(runner_for(&scope.agent)?);
        let reviews_to_run: Vec<_> = match args.name.as_deref() {
            Some(filter) => scope
                .reviews
                .iter()
                .filter(|r| r.name == filter)
                .cloned()
                .collect(),
            None => scope.reviews.to_vec(),
        };
        let scope_arc = Arc::new(scope);
        for review_entry in reviews_to_run {
            tasks.push(TaskInput {
                scope: scope_arc.clone(),
                scope_label: scope_label_for(&scope_arc.root, &repo_root),
                runner: runner.clone(),
                review: review_entry,
                base: base.clone(),
                diff: scoped_diff.clone(),
            });
        }
    }

    if tasks.is_empty() {
        // Still write an (empty) manifest so `blick publish` can tell the
        // difference between "review ran and matched nothing" and "review
        // crashed before writing anything". Without this, publish posts a
        // misleading "Blick review didn't run" notice whenever a diff doesn't
        // touch any configured scope.
        let manifest = RunManifest {
            run_id: run_id.clone(),
            base: base.clone(),
            tasks: Vec::new(),
        };
        let _ = write_manifest(&logs_dir.join("manifest.json"), &manifest);
        update_latest_pointer(&runs_root, &run_id);

        let report = ReviewReport::empty(format!(
            "No matching reviews found{}.",
            args.name
                .as_deref()
                .map(|n| format!(" for `{n}`"))
                .unwrap_or_default()
        ));
        println!("{}", render_report(&report, args.json)?);
        return Ok(());
    }

    for task in &tasks {
        eprintln!("▶ {}/{} starting…", task.scope_label, task.review.name);
    }

    let stream_mode = args.stream;
    let logs_dir = Arc::new(logs_dir);
    let run_id_arc = Arc::new(run_id.clone());
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrency));
    let mut futures = FuturesUnordered::new();
    for task in tasks {
        let logs_dir = logs_dir.clone();
        let run_id_arc = run_id_arc.clone();
        let permits = semaphore.clone();
        futures.push(async move {
            let _permit = permits.acquire_owned().await.expect("semaphore not closed");
            execute_task(task, run_id_arc, logs_dir).await
        });
    }

    let mut completed: Vec<TaskResult> = Vec::new();
    while let Some(result) = futures.next().await {
        match result {
            Ok(task_result) => {
                emit_task_block(&task_result, stream_mode);
                completed.push(task_result);
            }
            Err((label, err)) => {
                eprintln!("✖ {label} failed: {err}");
                return Err(err);
            }
        }
    }

    let manifest = RunManifest {
        run_id: run_id.clone(),
        base: base.clone(),
        tasks: completed
            .iter()
            .map(|r| TaskRef {
                scope_label: r.scope_label.clone(),
                review_name: r.review_name.clone(),
                record: PathBuf::from(task_filename(&r.scope_label, &r.review_name)),
            })
            .collect(),
    };
    let _ = write_manifest(&logs_dir.join("manifest.json"), &manifest);
    update_latest_pointer(&runs_root, &run_id);

    let combined = combine_reports(
        completed
            .into_iter()
            .map(|r| (r.scope_root, r.review_name, r.outcome.report))
            .collect(),
    );
    println!("{}", render_report(&combined, args.json)?);
    Ok(())
}
