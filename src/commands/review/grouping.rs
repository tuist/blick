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
/// references one of `files`. Returns an empty string when nothing
/// matches — for the focus-diff path where "this scope wasn't touched in
/// the latest push" must surface as "no findings to raise" rather than
/// silently widening to other scopes' changes.
pub(super) fn slice_focus_diff_by_files(diff: &str, files: &[String]) -> String {
    let mut out = String::new();
    let mut current_keep = false;
    for line in diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            current_keep = parse_diff_header_paths(line)
                .map(|(a, b)| files.iter().any(|f| f == a || f == b))
                .unwrap_or(false);
        }
        if current_keep {
            out.push_str(line);
        }
    }
    out
}

/// Extract just the file-blocks of `diff` whose `diff --git` header
/// references one of `files`. Falls back to the full diff if nothing
/// matches (so we don't accidentally hand the agent an empty diff).
fn slice_diff_by_files(diff: &str, files: &[String]) -> String {
    let mut out = String::new();
    let mut current_keep = false;
    for line in diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            current_keep = parse_diff_header_paths(line)
                .map(|(a, b)| files.iter().any(|f| f == a || f == b))
                .unwrap_or(false);
        }
        if current_keep {
            out.push_str(line);
        }
    }
    if out.is_empty() { diff.to_owned() } else { out }
}

/// Parse `diff --git a/<a> b/<b>` and return `(a, b)` with the `a/`/`b/`
/// prefixes stripped. Returns `None` for malformed or quoted headers; the
/// caller treats `None` as "don't include this block", and the trailing
/// "fall back to the whole diff if nothing matched" branch handles the
/// edge case where every header was unparseable.
///
/// Git quotes paths containing spaces, double quotes, or non-ASCII bytes
/// (`diff --git "a/with spaces" "b/with spaces"`). Decoding that quoting
/// correctly is non-trivial; rather than risk a wrong match, we return
/// `None` and let the fallback path handle it.
///
/// This replaces an earlier `line.contains("a/{f}")` substring match that
/// could match a longer file ending in the requested path's name (e.g.
/// `a/src/lib` matched both `src/lib.rs` and `src/library.rs`).
fn parse_diff_header_paths(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim_end_matches('\n').strip_prefix("diff --git ")?;
    if rest.starts_with('"') {
        return None;
    }
    // The format is `a/<path> b/<path>`. Split on the *last* ` b/` so a
    // path containing ` b/` as a substring inside `a/...` doesn't cause
    // a premature split.
    let split = rest.rfind(" b/")?;
    let a_part = rest.get(..split)?;
    let b_part = rest.get(split + 1..)?;
    let a = a_part.strip_prefix("a/")?;
    let b = b_part.strip_prefix("b/")?;
    Some((a, b))
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
    fn focus_slice_returns_empty_when_no_files_match() {
        // Unlike the regular slicer which falls back to the whole diff,
        // the focus slicer must surface "this scope wasn't touched" as an
        // empty diff so the agent doesn't see other scopes' changes.
        let chunk = slice_focus_diff_by_files(SAMPLE_DIFF, &["nope/missing.rs".to_owned()]);
        assert!(chunk.is_empty());
    }

    #[test]
    fn focus_slice_keeps_only_matching_blocks() {
        let chunk = slice_focus_diff_by_files(SAMPLE_DIFF, &["apps/web/main.rs".to_owned()]);
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

    #[test]
    fn slice_does_not_match_files_with_a_shared_prefix() {
        // Regression: an earlier `line.contains("a/{f}")` substring match
        // would treat `src/lib` as matching `src/library.rs`. With the
        // boundary-correct parse, the shorter path's block is *not*
        // included.
        let diff = "diff --git a/src/library.rs b/src/library.rs\n\
                    --- a/src/library.rs\n\
                    +++ b/src/library.rs\n\
                    @@\n\
                    -x\n\
                    +y\n";
        let chunk = slice_diff_by_files(diff, &["src/lib".to_owned()]);
        // No match → fallback returns the whole diff (so the agent always
        // sees something), but the parser must NOT have considered the
        // header to have matched the requested path.
        assert_eq!(chunk, diff);
    }

    #[test]
    fn slice_keeps_renamed_files_via_either_a_or_b_path() {
        // Rename diff: `a/old` and `b/new` differ. The block must be
        // kept whether the caller asked for the old or new path.
        let diff = "diff --git a/old/path.rs b/new/path.rs\n\
                    similarity index 95%\n\
                    rename from old/path.rs\n\
                    rename to new/path.rs\n\
                    --- a/old/path.rs\n\
                    +++ b/new/path.rs\n\
                    @@\n\
                    -x\n\
                    +y\n";
        let by_old = slice_diff_by_files(diff, &["old/path.rs".to_owned()]);
        let by_new = slice_diff_by_files(diff, &["new/path.rs".to_owned()]);
        assert!(by_old.contains("rename from old/path.rs"));
        assert!(by_new.contains("rename to new/path.rs"));
    }

    #[test]
    fn parse_diff_header_extracts_both_paths() {
        assert_eq!(
            parse_diff_header_paths("diff --git a/foo.rs b/foo.rs\n"),
            Some(("foo.rs", "foo.rs"))
        );
        assert_eq!(
            parse_diff_header_paths("diff --git a/old/x.rs b/new/x.rs\n"),
            Some(("old/x.rs", "new/x.rs"))
        );
    }

    #[test]
    fn parse_diff_header_returns_none_for_quoted_paths() {
        // Git quotes paths with spaces; decoding that quoting safely is
        // non-trivial, so we conservatively fail to parse.
        assert!(
            parse_diff_header_paths("diff --git \"a/with space\" \"b/with space\"\n").is_none()
        );
    }

    #[test]
    fn parse_diff_header_returns_none_for_malformed_lines() {
        assert!(parse_diff_header_paths("nonsense").is_none());
        assert!(parse_diff_header_paths("diff --git a/only-one-side\n").is_none());
    }
}
