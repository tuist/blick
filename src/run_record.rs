use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::BlickError;
use crate::review::ReviewReport;

/// Persisted record of a single `(scope, review)` execution. One JSON file
/// per task is written under `.blick/runs/<run-id>/`. Renderers read these
/// to produce GitHub PR reviews, Check Runs, and so on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub run_id: String,
    pub scope_label: String,
    pub review_name: String,
    pub base: String,
    pub files: Vec<String>,
    pub diff: String,
    pub report: ReviewReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunManifest {
    pub run_id: String,
    pub base: String,
    pub tasks: Vec<TaskRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRef {
    pub scope_label: String,
    pub review_name: String,
    pub record: PathBuf,
}

pub fn write_task_record(path: &Path, record: &TaskRecord) -> Result<(), BlickError> {
    let body = serde_json::to_string_pretty(record)
        .map_err(|err| BlickError::Config(format!("failed to serialize task record: {err}")))?;
    fs::write(path, body)?;
    Ok(())
}

pub fn write_manifest(path: &Path, manifest: &RunManifest) -> Result<(), BlickError> {
    let body = serde_json::to_string_pretty(manifest)
        .map_err(|err| BlickError::Config(format!("failed to serialize manifest: {err}")))?;
    fs::write(path, body)?;
    Ok(())
}

pub fn read_manifest(path: &Path) -> Result<RunManifest, BlickError> {
    let raw = fs::read_to_string(path)?;
    serde_json::from_str(&raw).map_err(|err| {
        BlickError::Config(format!(
            "failed to parse manifest {}: {err}",
            path.display()
        ))
    })
}

pub fn read_task_record(path: &Path) -> Result<TaskRecord, BlickError> {
    let raw = fs::read_to_string(path)?;
    serde_json::from_str(&raw).map_err(|err| {
        BlickError::Config(format!(
            "failed to parse task record {}: {err}",
            path.display()
        ))
    })
}

pub fn task_filename(scope_label: &str, review_name: &str) -> String {
    format!("{}--{}.json", scope_label.replace('/', "_"), review_name)
}

/// Resolve a `--run` argument into a runs directory path. Accepts:
/// - an absolute or relative directory path
/// - a run id (e.g. `20260427T123456Z`)
/// - the literal `latest` (or no argument), which reads
///   `<repo_root>/.blick/runs/latest`.
pub fn resolve_run_dir(repo_root: &Path, raw: Option<&str>) -> Result<PathBuf, BlickError> {
    let runs_root = repo_root.join(".blick").join("runs");
    let candidate = match raw {
        None => runs_root.join("latest"),
        Some("latest") => runs_root.join("latest"),
        Some(value) => {
            let as_path = PathBuf::from(value);
            if as_path.is_absolute() || value.contains('/') || value.starts_with('.') {
                as_path
            } else {
                runs_root.join(value)
            }
        }
    };

    if !candidate.exists() {
        return Err(BlickError::Config(format!(
            "run directory {} does not exist (try `blick review` first)",
            candidate.display()
        )));
    }

    Ok(fs::canonicalize(&candidate).unwrap_or(candidate))
}

pub fn list_task_records(run_dir: &Path) -> Result<Vec<TaskRecord>, BlickError> {
    let manifest_path = run_dir.join("manifest.json");
    let manifest = read_manifest(&manifest_path)?;
    let mut records = Vec::with_capacity(manifest.tasks.len());
    for entry in &manifest.tasks {
        let path = if entry.record.is_absolute() {
            entry.record.clone()
        } else {
            run_dir.join(&entry.record)
        };
        records.push(read_task_record(&path)?);
    }
    Ok(records)
}
