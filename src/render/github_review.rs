//! `POST /repos/.../pulls/{n}/reviews` payload.
//!
//! Bundles every in-diff finding in a run into a single review with line
//! comments the PR author can mark resolved. Out-of-diff findings (the
//! agent commented on context it read) are surfaced through the
//! per-record check-run summary instead — see [`super::check_run`] — so
//! the conversation tab doesn't accumulate one summary post per push.

use serde_json::{Value, json};

use crate::error::BlickError;
use crate::run_record::TaskRecord;

use super::RenderContext;
use super::badges::severity_badge;
use super::details::agent_instructions;
use super::diff_lines::DiffLineIndex;
use super::origin::origin_label;

const BLICK_FOOTER_LINK: &str = "[Blick](https://github.com/tuist/blick)";

pub(super) fn render_github_review(
    records: &[TaskRecord],
    ctx: RenderContext<'_>,
) -> Result<String, BlickError> {
    let commit_sha = ctx.commit_sha.or(ctx.head_sha).ok_or_else(|| {
        BlickError::Config("github-review requires --head-sha (the PR head commit)".to_owned())
    })?;

    let mut comments: Vec<Value> = Vec::new();

    for record in records {
        let origin = origin_label(&record.scope_label, &record.review_name);
        let index = DiffLineIndex::from_unified(&record.diff);

        for finding in &record.report.findings {
            let Some(line) = finding.line else {
                continue;
            };
            if !index.contains(&finding.file, line) {
                continue;
            }
            let body = format!(
                "{} **{}**\n\n{}\n\n{}\n\n— {} · `{}` review",
                severity_badge(finding.severity),
                finding.title,
                finding.body,
                agent_instructions(finding),
                BLICK_FOOTER_LINK,
                origin,
            );
            comments.push(json!({
                "path": finding.file,
                "line": line,
                "side": "RIGHT",
                "body": body,
            }));
        }
    }

    // Body intentionally empty: severity is already encoded on each inline
    // comment and on the check-run conclusion, so a `### Blick review`
    // summary block just duplicates information and stacks up on the
    // conversation tab as a separate timeline entry per push. `event` is
    // always `COMMENT` for the same reason — a blocking review state is
    // redundant with the high-severity check-run conclusion.
    let payload = json!({
        "commit_id": commit_sha,
        "event": "COMMENT",
        "body": "",
        "comments": comments,
    });
    Ok(serde_json::to_string_pretty(&payload).expect("serializable"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Severity;
    use crate::review::{Finding, ReviewReport};

    fn record_with_findings(findings: Vec<Finding>) -> TaskRecord {
        TaskRecord {
            run_id: "test".into(),
            scope_label: "apps/web".into(),
            review_name: "security".into(),
            base: "origin/main".into(),
            files: vec!["src/main.rs".into()],
            diff: "diff --git a/src/main.rs b/src/main.rs\n\
                   --- a/src/main.rs\n\
                   +++ b/src/main.rs\n\
                   @@ -1,3 +1,3 @@\n\
                    fn main() {\n\
                   -    println!(\"hello\");\n\
                   +    println!(\"hello, blick\");\n\
                    }\n"
            .into(),
            report: ReviewReport {
                summary: "One issue".into(),
                findings,
            },
        }
    }

    fn medium_finding_in_diff() -> Finding {
        Finding {
            severity: Severity::Medium,
            file: "src/main.rs".into(),
            line: Some(2),
            title: "Use safer print".into(),
            body: "Consider using a structured logger.".into(),
        }
    }

    fn low_finding_out_of_diff() -> Finding {
        Finding {
            severity: Severity::Low,
            file: "docs/old.md".into(),
            line: Some(99),
            title: "Out of diff".into(),
            body: "This file is not in the PR diff.".into(),
        }
    }

    #[test]
    fn renders_inline_comments_for_in_diff_findings() {
        let record =
            record_with_findings(vec![medium_finding_in_diff(), low_finding_out_of_diff()]);
        let json = render_github_review(
            &[record],
            RenderContext {
                head_sha: Some("deadbeef"),
                commit_sha: None,
            },
        )
        .unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["commit_id"], "deadbeef");
        assert_eq!(value["event"], "COMMENT");
        assert_eq!(value["body"], "");
        let comments = value["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0]["path"], "src/main.rs");
        assert_eq!(comments[0]["line"], 2);
        let body = comments[0]["body"].as_str().unwrap();
        assert!(body.contains("P2-yellow"));
        assert!(body.contains("[Blick]"));
        assert!(body.contains("Instructions for AI agents"));
    }

    #[test]
    fn out_of_diff_findings_are_dropped_from_the_review() {
        // Out-of-diff findings render via per-record check runs now; the
        // PR review payload should only contain in-diff inline comments.
        let record = record_with_findings(vec![low_finding_out_of_diff()]);
        let json = render_github_review(
            &[record],
            RenderContext {
                head_sha: Some("deadbeef"),
                commit_sha: None,
            },
        )
        .unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["body"], "");
        assert!(value["comments"].as_array().unwrap().is_empty());
    }

    #[test]
    fn always_uses_comment_event_even_with_high_severity() {
        let record = record_with_findings(vec![Finding {
            severity: Severity::High,
            file: "src/main.rs".into(),
            line: Some(2),
            title: "Critical".into(),
            body: "...".into(),
        }]);
        let json = render_github_review(
            &[record],
            RenderContext {
                head_sha: Some("deadbeef"),
                commit_sha: None,
            },
        )
        .unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["event"], "COMMENT");
    }

    #[test]
    fn requires_a_commit_sha() {
        let record = record_with_findings(vec![]);
        let err = render_github_review(
            &[record],
            RenderContext {
                head_sha: None,
                commit_sha: None,
            },
        )
        .unwrap_err();
        assert!(matches!(err, BlickError::Config(_)));
    }
}
