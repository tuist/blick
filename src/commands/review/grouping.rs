//! Partition a top-level diff into per-scope sub-diffs.
//!
//! `git diff` output is a sequence of file blocks separated by
//! `diff --git a/<path> b/<path>` headers. We use those headers to chop the
//! diff into chunks per owning scope so each review only sees its own files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::ScopeConfig;
use crate::git::DiffBundle;
use crate::scope::owner_for;

/// Partition the diff by owning scope. Files that don't belong to any scope
/// are dropped silently.
pub(super) fn group_changes_by_scope(
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

/// Extract just the file-blocks of `diff` whose `diff --git` header
/// references one of `files`. Falls back to the full diff if nothing
/// matches (so we don't accidentally hand the agent an empty diff).
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
        diff.to_owned()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_DIFF: &str = "diff --git a/apps/web/main.rs b/apps/web/main.rs\n\
--- a/apps/web/main.rs\n\
+++ b/apps/web/main.rs\n\
@@\n\
-old\n\
+new\n\
diff --git a/server/lib.rs b/server/lib.rs\n\
--- a/server/lib.rs\n\
+++ b/server/lib.rs\n\
@@\n\
-x\n\
+y\n";

    #[test]
    fn slice_keeps_only_matching_file_blocks() {
        let chunk = slice_diff_by_files(SAMPLE_DIFF, &["apps/web/main.rs".to_owned()]);
        assert!(chunk.contains("a/apps/web/main.rs"));
        assert!(!chunk.contains("server/lib.rs"));
    }

    #[test]
    fn slice_falls_back_to_whole_diff_when_no_match() {
        let chunk = slice_diff_by_files(SAMPLE_DIFF, &["nope/missing.rs".to_owned()]);
        // Fallback so we don't hand the agent an empty diff.
        assert_eq!(chunk, SAMPLE_DIFF);
    }

    #[test]
    fn slice_handles_multiple_matching_files() {
        let chunk = slice_diff_by_files(
            SAMPLE_DIFF,
            &["apps/web/main.rs".to_owned(), "server/lib.rs".to_owned()],
        );
        assert!(chunk.contains("apps/web/main.rs"));
        assert!(chunk.contains("server/lib.rs"));
    }
}
