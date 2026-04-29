//! Label helpers for displaying scope paths in run output and combining
//! per-scope reports into a single rolled-up [`ReviewReport`].

use std::path::{Path, PathBuf};

use crate::review::{Finding, ReviewReport};

/// Stable label for a scope, suitable for filenames and human display:
/// repo-root → `.`, otherwise the path relative to the repo root. Falls
/// back to the directory name (or absolute path) when stripping fails.
pub(super) fn scope_label_for(scope_root: &Path, repo_root: &Path) -> String {
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

/// Roll a per-task list of `(scope_root, review_name, report)` tuples into
/// a single [`ReviewReport`], prefixing each finding's title with the
/// `[scope/review]` origin. Single-task runs pass straight through unchanged.
pub(super) fn combine_reports(reports: Vec<(PathBuf, String, ReviewReport)>) -> ReviewReport {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Severity;

    #[test]
    fn scope_label_uses_dot_for_repo_root() {
        let root = Path::new("/repo");
        assert_eq!(scope_label_for(root, root), ".");
    }

    #[test]
    fn scope_label_strips_repo_prefix() {
        assert_eq!(
            scope_label_for(Path::new("/repo/apps/web"), Path::new("/repo")),
            "apps/web"
        );
    }

    #[test]
    fn scope_label_falls_back_to_basename_when_outside_repo() {
        assert_eq!(
            scope_label_for(Path::new("/elsewhere/scope"), Path::new("/repo")),
            "scope"
        );
    }

    fn report(summary: &str, findings: Vec<Finding>) -> ReviewReport {
        ReviewReport {
            summary: summary.into(),
            findings,
        }
    }

    fn finding(title: &str) -> Finding {
        Finding {
            severity: Severity::Low,
            file: "x".into(),
            line: None,
            title: title.into(),
            body: "b".into(),
        }
    }

    #[test]
    fn combine_passes_single_task_through_unchanged() {
        let r = report("hello", vec![finding("orig")]);
        let combined = combine_reports(vec![(PathBuf::from("/repo"), "default".into(), r)]);
        assert_eq!(combined.summary, "hello");
        assert_eq!(combined.findings[0].title, "orig");
    }

    #[test]
    fn combine_prefixes_findings_with_scope_review_label() {
        let combined = combine_reports(vec![
            (
                PathBuf::from("/repo/a"),
                "sec".into(),
                report("a-sum", vec![finding("foo")]),
            ),
            (
                PathBuf::from("/repo/b"),
                "perf".into(),
                report("b-sum", vec![finding("bar")]),
            ),
        ]);
        assert!(combined.summary.contains("[a/sec] a-sum"));
        assert!(combined.summary.contains("[b/perf] b-sum"));
        assert!(combined.findings.iter().any(|f| f.title == "[a/sec] foo"));
        assert!(combined.findings.iter().any(|f| f.title == "[b/perf] bar"));
    }

    #[test]
    fn combine_emits_no_findings_summary_when_all_summaries_blank() {
        let combined = combine_reports(vec![
            (PathBuf::from("/repo/a"), "x".into(), report("", vec![])),
            (PathBuf::from("/repo/b"), "y".into(), report("   ", vec![])),
        ]);
        assert_eq!(combined.summary, "No findings.");
    }
}
