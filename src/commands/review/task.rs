//! Per-task execution: invoke the agent for one `(scope, review)` pair and
//! persist the log, prompt, and machine-readable record.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::agent::AgentRunner;
use crate::config::{ReviewEntry, ScopeConfig};
use crate::error::BlickError;
use crate::git::DiffBundle;
use crate::review::{ReviewOutcome, run_review};
use crate::run_record::{TaskRecord, task_filename, write_task_record};

/// Everything a single task needs to do its work. Held by value so it can
/// be moved into the spawned future.
pub(super) struct TaskInput {
    pub(super) scope: Arc<ScopeConfig>,
    pub(super) scope_label: String,
    pub(super) runner: Arc<dyn AgentRunner>,
    pub(super) review: ReviewEntry,
    pub(super) base: String,
    pub(super) diff: DiffBundle,
}

/// Successful task output. Errors are returned as `(label, error)` so the
/// orchestrator can attribute the failure when it propagates.
pub(super) struct TaskResult {
    pub(super) scope_root: PathBuf,
    pub(super) scope_label: String,
    pub(super) review_name: String,
    pub(super) outcome: ReviewOutcome,
    pub(super) log_path: PathBuf,
    #[allow(dead_code)]
    pub(super) record_path: PathBuf,
}

pub(super) async fn execute_task(
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

    let stem = task_log_stem(&task.scope_label, &task.review.name);
    let log_path = logs_dir.join(format!("{stem}.log"));
    let _ = write_task_log(&log_path, &label, &outcome);

    // Persist the assembled system prompt alongside the log so contributors
    // can verify which skills + overrides were actually composed for the
    // agent. Also picked up by the `blick-runs` CI artifact.
    let prompt_path = logs_dir.join(format!("{stem}.prompt.md"));
    let _ = fs::write(&prompt_path, &outcome.system_prompt);

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

/// Build a filesystem-safe stem for a task's log + prompt files by joining
/// the scope label and review name with `--` and replacing `/` in the
/// scope label with `_` (e.g. `("apps/web", "security")` →
/// `apps_web--security`).
fn task_log_stem(scope_label: &str, review_name: &str) -> String {
    format!("{}--{}", scope_label.replace('/', "_"), review_name)
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

/// Stream a task's stdout/stderr to the parent process's stderr with a
/// per-line label, so concurrent tasks don't get interleaved indecipherably.
pub(super) fn emit_task_block(result: &TaskResult, stream: bool) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_stem_replaces_slashes_in_scope_label() {
        assert_eq!(task_log_stem("apps/web", "security"), "apps_web--security");
    }

    #[test]
    fn log_stem_handles_root_scope() {
        assert_eq!(task_log_stem(".", "default"), ".--default");
    }
}
