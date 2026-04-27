use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;
use futures::stream::{FuturesUnordered, StreamExt};

use crate::agent::{AgentRunner, runner_for};
use crate::cli::{Cli, Commands, ConfigArgs, InitArgs, RenderArgs, ReviewArgs};
use crate::config::{AgentConfig, AgentKind, ConfigFile, ScopeConfig};
use crate::error::BlickError;
use crate::git::{DiffBundle, collect_diff_in};
use crate::render::{self, RenderContext};
use crate::review::{Finding, ReviewOutcome, ReviewReport, render_report, run_review};
use crate::run_record::resolve_run_dir;
use crate::run_record::{
    RunManifest, TaskRecord, TaskRef, task_filename, write_manifest, write_task_record,
};
use crate::scope::{load_scopes, owner_for};

pub async fn run() -> Result<(), BlickError> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init(args) => init(args),
        Commands::Review(args) => review(args).await,
        Commands::Config(args) => show_config(args),
        Commands::Render(args) => render_run(args),
    }
}

fn render_run(args: RenderArgs) -> Result<(), BlickError> {
    let repo_root = args
        .repo
        .clone()
        .unwrap_or_else(|| PathBuf::from("."))
        .canonicalize()?;
    let run_dir = resolve_run_dir(&repo_root, args.run.as_deref())?;
    let ctx = RenderContext {
        head_sha: args.head_sha.as_deref(),
        commit_sha: args.head_sha.as_deref(),
    };
    let rendered = render::render(&run_dir, args.format, ctx)?;
    println!("{rendered}");
    Ok(())
}

fn init(args: InitArgs) -> Result<(), BlickError> {
    if args.path.exists() && !args.force {
        return Err(BlickError::Config(format!(
            "{} already exists. Re-run with --force to overwrite it.",
            args.path.display()
        )));
    }

    let config = ConfigFile::starter(args.agent, args.model);
    fs::write(&args.path, config.to_toml()?)?;
    println!("Created {}", args.path.display());
    Ok(())
}

async fn review(args: ReviewArgs) -> Result<(), BlickError> {
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

    let base = resolve_base(&scopes, args.base.as_deref());
    let max_diff = scopes
        .iter()
        .map(|s| s.defaults.max_diff_bytes)
        .max()
        .unwrap_or(120_000);
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
        let scope = scopes
            .iter()
            .find(|s| s.root == scope_root)
            .expect("owner scope must exist")
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

fn update_latest_pointer(runs_root: &Path, run_id: &str) {
    let latest = runs_root.join("latest");
    let _ = fs::remove_file(&latest);
    let _ = fs::remove_dir_all(&latest);
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let _ = symlink(run_id, &latest);
    }
    #[cfg(not(unix))]
    {
        // Fall back to writing the run-id into a plain text file so non-unix
        // platforms can still resolve `latest`.
        let _ = fs::write(&latest, run_id);
    }
}

struct TaskInput {
    scope: Arc<ScopeConfig>,
    scope_label: String,
    runner: Arc<dyn AgentRunner>,
    review: crate::config::ReviewEntry,
    base: String,
    diff: DiffBundle,
}

struct TaskResult {
    scope_root: PathBuf,
    scope_label: String,
    review_name: String,
    outcome: ReviewOutcome,
    log_path: PathBuf,
    #[allow(dead_code)]
    record_path: PathBuf,
}

async fn execute_task(
    task: TaskInput,
    run_id: Arc<String>,
    logs_dir: Arc<PathBuf>,
) -> Result<TaskResult, (String, BlickError)> {
    let label = format!("{}/{}", task.scope_label, task.review.name);
    let outcome = run_review(
        &*task.runner,
        &task.scope,
        &task.review,
        &task.base,
        &task.diff,
    )
    .await
    .map_err(|err| (label.clone(), err))?;

    let log_name = format!(
        "{}--{}.log",
        task.scope_label.replace('/', "_"),
        task.review.name
    );
    let log_path = logs_dir.join(log_name);
    let _ = write_task_log(&log_path, &label, &outcome);

    let record = TaskRecord {
        run_id: (*run_id).clone(),
        scope_label: task.scope_label.clone(),
        review_name: task.review.name.clone(),
        base: task.base.clone(),
        files: task.diff.files.clone(),
        diff: task.diff.diff.clone(),
        report: outcome.report.clone(),
    };
    let record_path = logs_dir.join(task_filename(&task.scope_label, &task.review.name));
    let _ = write_task_record(&record_path, &record);

    Ok(TaskResult {
        scope_root: task.scope.root.clone(),
        scope_label: task.scope_label,
        review_name: task.review.name,
        outcome,
        log_path,
        record_path,
    })
}

fn write_task_log(path: &Path, label: &str, outcome: &ReviewOutcome) -> std::io::Result<()> {
    let mut file = fs::File::create(path)?;
    writeln!(file, "# blick task log: {label}")?;
    writeln!(file, "## stdout")?;
    file.write_all(outcome.run.stdout.as_bytes())?;
    if !outcome.run.stdout.ends_with('\n') {
        writeln!(file)?;
    }
    writeln!(file, "## stderr")?;
    file.write_all(outcome.run.stderr.as_bytes())?;
    if !outcome.run.stderr.ends_with('\n') {
        writeln!(file)?;
    }
    writeln!(file, "## text")?;
    file.write_all(outcome.run.text.as_bytes())?;
    if !outcome.run.text.ends_with('\n') {
        writeln!(file)?;
    }
    Ok(())
}

fn emit_task_block(result: &TaskResult, stream: bool) {
    eprintln!(
        "✓ {}/{} done ({} findings) — log: {}",
        result.scope_label,
        result.review_name,
        result.outcome.report.findings.len(),
        result.log_path.display()
    );
    if !stream {
        return;
    }
    let label = format!("{}/{}", result.scope_label, result.review_name);
    if !result.outcome.run.stdout.trim().is_empty() {
        eprintln!("--- {label} stdout ---");
        for line in result.outcome.run.stdout.lines() {
            eprintln!("[{label}] {line}");
        }
    }
    if !result.outcome.run.stderr.trim().is_empty() {
        eprintln!("--- {label} stderr ---");
        for line in result.outcome.run.stderr.lines() {
            eprintln!("[{label}] {line}");
        }
    }
}

fn scope_label_for(scope_root: &Path, repo_root: &Path) -> String {
    scope_root
        .strip_prefix(repo_root)
        .map(|p| {
            if p.as_os_str().is_empty() {
                ".".to_owned()
            } else {
                p.display().to_string()
            }
        })
        .unwrap_or_else(|_| {
            scope_root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| scope_root.display().to_string())
        })
}

fn combine_reports(reports: Vec<(PathBuf, String, ReviewReport)>) -> ReviewReport {
    if reports.len() == 1 {
        return reports.into_iter().next().unwrap().2;
    }

    let mut combined = ReviewReport::empty(String::new());
    let mut summaries = Vec::new();
    for (scope_root, review_name, report) in reports {
        let scope_label = scope_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| scope_root.display().to_string());
        if !report.summary.trim().is_empty() {
            summaries.push(format!("[{scope_label}/{review_name}] {}", report.summary));
        }
        for finding in report.findings {
            combined.findings.push(Finding {
                title: format!("[{scope_label}/{review_name}] {}", finding.title),
                ..finding
            });
        }
    }
    combined.summary = if summaries.is_empty() {
        "No findings.".to_owned()
    } else {
        summaries.join("\n")
    };
    combined
}

fn show_config(args: ConfigArgs) -> Result<(), BlickError> {
    let repo_root = args
        .repo
        .clone()
        .unwrap_or_else(|| PathBuf::from("."))
        .canonicalize()?;
    let scopes = load_scopes(&repo_root)?;

    if scopes.is_empty() {
        println!("No blick.toml found under {}", repo_root.display());
        return Ok(());
    }

    for scope in &scopes {
        println!("# scope: {}", display_path(&scope.root, &repo_root));
        println!("  agent.kind  = {}", scope.agent.kind.as_str());
        if let Some(model) = &scope.agent.model {
            println!("  agent.model = {}", model);
        }
        println!("  defaults.base           = {}", scope.defaults.base);
        println!(
            "  defaults.max_diff_bytes = {}",
            scope.defaults.max_diff_bytes
        );
        println!(
            "  defaults.fail_on        = {}",
            scope.defaults.fail_on.as_str()
        );

        if !scope.skills.is_empty() {
            println!("  skills:");
            for (name, resolved) in &scope.skills {
                println!(
                    "    - {name} ({}) [declared in {}]",
                    resolved.entry.source,
                    display_path(&resolved.declared_in, &repo_root)
                );
            }
        }

        if !scope.reviews.is_empty() {
            println!("  reviews:");
            for review_entry in &scope.reviews {
                println!(
                    "    - {} (skills: {})",
                    review_entry.name,
                    if review_entry.skills.is_empty() {
                        "<none>".to_owned()
                    } else {
                        review_entry.skills.join(", ")
                    }
                );
            }
        }

        if scope.reviews.is_empty() {
            println!("  ⚠ this scope defines no reviews; files here will be skipped.");
        }

        if args.explain {
            println!("  provenance:");
            for entry in &scope.provenance {
                println!(
                    "    {:<24} from {}",
                    entry.field,
                    display_path(&entry.source, &repo_root)
                );
            }
        }
        println!();
    }

    Ok(())
}

fn display_path(path: &Path, repo_root: &Path) -> String {
    path.strip_prefix(repo_root)
        .map(|p| {
            if p.as_os_str().is_empty() {
                ".".to_owned()
            } else {
                p.display().to_string()
            }
        })
        .unwrap_or_else(|_| path.display().to_string())
}

fn apply_agent_overrides(
    scopes: &mut [ScopeConfig],
    agent: Option<AgentKind>,
    model: Option<&str>,
) {
    let env_agent = env::var("BLICK_AGENT_KIND")
        .ok()
        .and_then(|raw| match raw.as_str() {
            "claude" => Some(AgentKind::Claude),
            "codex" => Some(AgentKind::Codex),
            "opencode" => Some(AgentKind::Opencode),
            _ => None,
        });
    let env_model = env::var("BLICK_AGENT_MODEL").ok();

    let final_agent = agent.or(env_agent);
    let final_model: Option<String> = model.map(ToOwned::to_owned).or(env_model);

    if final_agent.is_none() && final_model.is_none() {
        return;
    }

    for scope in scopes.iter_mut() {
        if let Some(kind) = final_agent {
            scope.agent = AgentConfig {
                kind,
                model: final_model
                    .clone()
                    .or_else(|| kind.default_model().map(ToOwned::to_owned)),
                binary: scope.agent.binary.clone(),
                args: scope.agent.args.clone(),
            };
        } else if let Some(model_value) = &final_model {
            scope.agent.model = Some(model_value.clone());
        }
    }
}

fn resolve_base(scopes: &[ScopeConfig], cli_base: Option<&str>) -> String {
    if let Some(base) = cli_base {
        return base.to_owned();
    }
    scopes
        .first()
        .map(|s| s.defaults.base.clone())
        .unwrap_or_else(|| "HEAD".to_owned())
}

/// Partition the diff by owning scope. Files that don't belong to any scope
/// are dropped (with a single aggregated note).
fn group_changes_by_scope(
    repo_root: &Path,
    scopes: &[ScopeConfig],
    diff: &DiffBundle,
) -> BTreeMap<PathBuf, DiffBundle> {
    let mut groups: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    for file in &diff.files {
        let absolute = repo_root.join(file);
        let Some(scope) = owner_for(scopes, &absolute) else {
            continue;
        };
        groups
            .entry(scope.root.clone())
            .or_default()
            .push(file.clone());
    }

    // For each scope, slice the diff into per-file chunks. Cheap heuristic:
    // include hunks whose `diff --git` headers reference files we own.
    let mut by_scope: BTreeMap<PathBuf, DiffBundle> = BTreeMap::new();
    for (scope_root, files) in groups {
        let chunk = slice_diff_by_files(&diff.diff, &files);
        by_scope.insert(
            scope_root,
            DiffBundle {
                files,
                diff: chunk,
                truncated: diff.truncated,
            },
        );
    }
    by_scope
}

fn slice_diff_by_files(diff: &str, files: &[String]) -> String {
    let mut out = String::new();
    let mut current_keep = false;
    for line in diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            current_keep = files
                .iter()
                .any(|f| line.contains(&format!("a/{f}")) || line.contains(&format!("b/{f}")));
        }
        if current_keep {
            out.push_str(line);
        }
    }
    if out.is_empty() {
        // Fallback: pass the whole diff if we couldn't slice it.
        diff.to_owned()
    } else {
        out
    }
}
