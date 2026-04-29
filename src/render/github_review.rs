//! `POST /repos/.../pulls/{n}/reviews` payload.
//!
//! Bundles every finding in a run into a single review with line comments
//! the PR author can mark resolved. Findings whose lines aren't in the PR
//! diff (per GitHub's view of it) are folded into the review body so they
//! aren't dropped silently.

use serde_json::{Value, json};

use crate::config::Severity;
use crate::error::BlickError;
use crate::review::Finding;
use crate::run_record::TaskRecord;

use super::badges::severity_badge;
use super::details::agent_instructions;
use super::diff_lines::DiffLineIndex;
use super::markers::{LAST_REVIEWED_MARKER_PREFIX, LAST_REVIEWED_MARKER_SUFFIX};
use super::origin::origin_label;
use super::{RenderContext, count_findings};

const BLICK_FOOTER_LINK: &str = "[Blick](https://github.com/tuist/blick)";

pub(super) fn render_github_review(
    records: &[TaskRecord],
    ctx: RenderContext<'_>,
) -> Result<String, BlickError> {
    let commit_sha = ctx.commit_sha.or(ctx.head_sha).ok_or_else(|| {
        BlickError::Config("github-review requires --head-sha (the PR head commit)".to_owned())
    })?;

    let mut comments: Vec<Value> = Vec::new();
    let mut out_of_diff: Vec<&Finding> = Vec::new();
    let total_findings = count_findings(records);
    let mut summary_lines: Vec<String> = Vec::new();

    for record in records {
        let origin = origin_label(&record.scope_label, &record.review_name);
        let index = DiffLineIndex::from_unified(&record.diff);

        // Only mention reviews that actually contributed findings; otherwise
        // the body just repeats the "No findings" header.
        if !record.report.findings.is_empty() {
            summary_lines.push(format!(
                "**{} review** - {} ({} finding{})",
                origin,
                record.report.summary,
                record.report.findings.len(),
                if record.report.findings.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        }

        for finding in &record.report.findings {
            let body = format!(
                "{} **{}**\n\n{}\n\n{}\n\n— {} · `{}` review",
                severity_badge(finding.severity),
                finding.title,
                finding.body,
                agent_instructions(finding),
                BLICK_FOOTER_LINK,
                origin,
            );
            match finding.line {
                Some(line) if index.contains(&finding.file, line) => {
                    comments.push(json!({
                        "path": finding.file,
                        "line": line,
                        "side": "RIGHT",
                        "body": body,
                    }));
                }
                _ => out_of_diff.push(finding),
            }
        }
    }

    let mut body = build_review_body(records, total_findings, &summary_lines, &out_of_diff);
    body.push_str("\n\n");
    body.push_str(LAST_REVIEWED_MARKER_PREFIX);
    body.push_str(commit_sha);
    body.push_str(LAST_REVIEWED_MARKER_SUFFIX);
    let event = pick_review_event(records, total_findings);

    let payload = json!({
        "commit_id": commit_sha,
        "event": event,
        "body": body,
        "comments": comments,
    });
    Ok(serde_json::to_string_pretty(&payload).expect("serializable"))
}

/// `REQUEST_CHANGES` only when there's at least one high-severity finding;
/// otherwise `COMMENT` so the PR isn't blocked on a pile of low-severity
/// suggestions.
fn pick_review_event(records: &[TaskRecord], total_findings: usize) -> &'static str {
    if total_findings == 0 {
        "COMMENT"
    } else if records
        .iter()
        .flat_map(|r| r.report.findings.iter())
        .any(|f| f.severity == Severity::High)
    {
        "REQUEST_CHANGES"
    } else {
        "COMMENT"
    }
}

fn build_review_body(
    records: &[TaskRecord],
    total_findings: usize,
    summary_lines: &[String],
    out_of_diff: &[&Finding],
) -> String {
    let header = if total_findings == 0 {
        "### Blick review\n\nNo findings.".to_owned()
    } else {
        format!(
            "### Blick review\n\n{} finding{} across {} review{}.",
            total_findings,
            if total_findings == 1 { "" } else { "s" },
            records.len(),
            if records.len() == 1 { "" } else { "s" }
        )
    };

    let mut body = header;
    if !summary_lines.is_empty() {
        body.push_str("\n\n");
        body.push_str(&summary_lines.join("\n"));
    }

    if !out_of_diff.is_empty() {
        body.push_str("\n\n#### Findings outside this PR's diff\n");
        for finding in out_of_diff {
            let location = match finding.line {
                Some(line) => format!("`{}:{line}`", finding.file),
                None => format!("`{}`", finding.file),
            };
            body.push_str(&format!(
                "- {} {} - {}\n",
                severity_badge(finding.severity),
                location,
                finding.title
            ));
        }
    }

    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::ReviewReport;

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
    fn renders_inline_comments_and_body_overflow() {
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
        assert!(json.contains("\"commit_id\": \"deadbeef\""));
        assert!(json.contains("\"path\": \"src/main.rs\""));
        assert!(json.contains("\"line\": 2"));
        // Inline comment uses the P2 (medium) priority badge and links to Blick.
        assert!(json.contains("P2-yellow"));
        assert!(json.contains("[Blick]"));
        // Inline comments embed agent instructions in a collapsed <details>.
        assert!(json.contains("Instructions for AI agents"));
        assert!(json.contains("src/main.rs:2"));
        // Out-of-diff finding ends up in the review body.
        assert!(json.contains("Findings outside this PR"));
        assert!(json.contains("docs/old.md"));
        // Body carries the last-reviewed marker so future runs can do
        // incremental reviews against the SHA we just reviewed.
        assert!(json.contains("blick:last-reviewed=deadbeef"));
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

    #[test]
    fn high_severity_triggers_request_changes() {
        let record = record_with_findings(vec![Finding {
            severity: Severity::High,
            file: "src/main.rs".into(),
            line: Some(2),
            title: "Critical".into(),
            body: "…".into(),
        }]);
        assert_eq!(pick_review_event(&[record], 1), "REQUEST_CHANGES");
    }

    #[test]
    fn medium_only_stays_a_plain_comment() {
        let record = record_with_findings(vec![medium_finding_in_diff()]);
        assert_eq!(pick_review_event(&[record], 1), "COMMENT");
    }

    #[test]
    fn no_findings_stays_a_plain_comment() {
        let record = record_with_findings(vec![]);
        assert_eq!(pick_review_event(&[record], 0), "COMMENT");
    }
}
