pub mod diff_lines;

use std::path::Path;

use clap::ValueEnum;
use serde_json::{Value, json};

use crate::config::Severity;
use crate::error::BlickError;
use crate::review::Finding;
use crate::run_record::{TaskRecord, list_task_records};

use self::diff_lines::DiffLineIndex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// JSON for `POST /repos/.../pulls/{n}/reviews` — bundles every
    /// finding in the run into a single review with line comments the PR
    /// author can mark resolved.
    #[value(name = "github-review")]
    GithubReview,
    /// JSON Lines for `POST /repos/.../check-runs` — one Check Run per
    /// `(scope, review)` pair so each shows up as its own check on the PR.
    #[value(name = "check-run")]
    CheckRun,
    /// Plain markdown summary, suitable for `gh pr comment --body-file -`
    /// or any chat/email integration.
    #[value(name = "github-summary")]
    GithubSummary,
}

#[derive(Debug, Clone)]
pub struct RenderContext<'a> {
    pub head_sha: Option<&'a str>,
    pub commit_sha: Option<&'a str>,
}

pub fn render(
    run_dir: &Path,
    format: Format,
    ctx: RenderContext<'_>,
) -> Result<String, BlickError> {
    let records = list_task_records(run_dir)?;
    match format {
        Format::GithubReview => render_github_review(&records, ctx),
        Format::CheckRun => render_check_runs(&records, ctx),
        Format::GithubSummary => Ok(render_github_summary(&records)),
    }
}

fn render_github_review(
    records: &[TaskRecord],
    ctx: RenderContext<'_>,
) -> Result<String, BlickError> {
    let commit_sha = ctx.commit_sha.or(ctx.head_sha).ok_or_else(|| {
        BlickError::Config("github-review requires --head-sha (the PR head commit)".to_owned())
    })?;

    let mut comments: Vec<Value> = Vec::new();
    let mut out_of_diff: Vec<&Finding> = Vec::new();
    let mut total_findings = 0usize;
    let mut summary_lines: Vec<String> = Vec::new();

    for record in records {
        total_findings += record.report.findings.len();
        let index = DiffLineIndex::from_unified(&record.diff);

        summary_lines.push(format!(
            "**{}/{}** — {} ({} finding{})",
            record.scope_label,
            record.review_name,
            record.report.summary,
            record.report.findings.len(),
            if record.report.findings.len() == 1 {
                ""
            } else {
                "s"
            }
        ));

        for finding in &record.report.findings {
            let body = format!(
                "**[{}]** {}\n\n{}\n\n_Reported by `{}/{}`._",
                finding.severity.as_str(),
                finding.title,
                finding.body,
                record.scope_label,
                record.review_name
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

    let body = build_review_body(records, total_findings, &summary_lines, &out_of_diff);
    let event = if total_findings == 0 {
        "COMMENT"
    } else if records
        .iter()
        .flat_map(|r| r.report.findings.iter())
        .any(|f| f.severity == Severity::High)
    {
        "REQUEST_CHANGES"
    } else {
        "COMMENT"
    };

    let payload = json!({
        "commit_id": commit_sha,
        "event": event,
        "body": body,
        "comments": comments,
    });
    Ok(serde_json::to_string_pretty(&payload).expect("serializable"))
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
                "- **[{}]** {} — {}\n",
                finding.severity.as_str(),
                location,
                finding.title
            ));
        }
    }

    body
}

fn render_check_runs(records: &[TaskRecord], ctx: RenderContext<'_>) -> Result<String, BlickError> {
    let head_sha = ctx
        .head_sha
        .or(ctx.commit_sha)
        .ok_or_else(|| BlickError::Config("check-run requires --head-sha".to_owned()))?;

    let mut lines: Vec<String> = Vec::new();
    for record in records {
        let index = DiffLineIndex::from_unified(&record.diff);
        let mut annotations: Vec<Value> = Vec::new();
        for finding in &record.report.findings {
            let Some(line) = finding.line else {
                continue;
            };
            if !index.contains(&finding.file, line) {
                continue;
            }
            annotations.push(json!({
                "path": finding.file,
                "start_line": line,
                "end_line": line,
                "annotation_level": severity_to_annotation_level(finding.severity),
                "title": finding.title,
                "message": finding.body,
            }));
        }

        let conclusion = conclusion_for(record);
        let summary = format!(
            "{} ({} finding{})",
            record.report.summary,
            record.report.findings.len(),
            if record.report.findings.len() == 1 {
                ""
            } else {
                "s"
            }
        );
        let payload = json!({
            "name": format!("blick / {}/{}", record.scope_label, record.review_name),
            "head_sha": head_sha,
            "status": "completed",
            "conclusion": conclusion,
            "output": {
                "title": format!("{} — {}", record.review_name, conclusion),
                "summary": summary,
                "annotations": annotations,
            },
        });
        lines.push(serde_json::to_string(&payload).expect("serializable"));
    }
    Ok(lines.join("\n"))
}

fn render_github_summary(records: &[TaskRecord]) -> String {
    let mut total = 0usize;
    let mut lines: Vec<String> = vec!["### Blick review".to_owned(), String::new()];

    for record in records {
        total += record.report.findings.len();
    }

    if total == 0 {
        lines.push("No findings.".to_owned());
    } else {
        lines.push(format!(
            "{} finding{} across {} review{}.",
            total,
            if total == 1 { "" } else { "s" },
            records.len(),
            if records.len() == 1 { "" } else { "s" }
        ));
    }

    for record in records {
        lines.push(String::new());
        lines.push(format!(
            "#### {}/{}",
            record.scope_label, record.review_name
        ));
        lines.push(record.report.summary.clone());
        if !record.report.findings.is_empty() {
            lines.push(String::new());
            lines.push("| Severity | File | Title |".to_owned());
            lines.push("| --- | --- | --- |".to_owned());
            for finding in &record.report.findings {
                let location = match finding.line {
                    Some(line) => format!("`{}:{line}`", finding.file),
                    None => format!("`{}`", finding.file),
                };
                lines.push(format!(
                    "| {} | {} | {} |",
                    finding.severity.as_str(),
                    location,
                    finding.title
                ));
            }
        }
    }

    lines.join("\n")
}

fn severity_to_annotation_level(severity: Severity) -> &'static str {
    match severity {
        Severity::High => "failure",
        Severity::Medium => "warning",
        Severity::Low => "notice",
    }
}

fn conclusion_for(record: &TaskRecord) -> &'static str {
    let has_high = record
        .report
        .findings
        .iter()
        .any(|f| f.severity == Severity::High);
    if has_high {
        "failure"
    } else if record.report.findings.is_empty() {
        "success"
    } else {
        "neutral"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Severity;
    use crate::review::{Finding, ReviewReport};

    fn fixture_record() -> TaskRecord {
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
                findings: vec![
                    Finding {
                        severity: Severity::Medium,
                        file: "src/main.rs".into(),
                        line: Some(2),
                        title: "Use safer print".into(),
                        body: "Consider using a structured logger.".into(),
                    },
                    Finding {
                        severity: Severity::Low,
                        file: "docs/old.md".into(),
                        line: Some(99),
                        title: "Out of diff".into(),
                        body: "This file is not in the PR diff.".into(),
                    },
                ],
            },
        }
    }

    #[test]
    fn renders_github_review_with_inline_comments_and_body_overflow() {
        let record = fixture_record();
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
        // Out-of-diff finding ends up in the review body.
        assert!(json.contains("Findings outside this PR"));
        assert!(json.contains("docs/old.md"));
    }

    #[test]
    fn renders_check_run_with_annotations() {
        let record = fixture_record();
        let ndjson = render_check_runs(
            &[record],
            RenderContext {
                head_sha: Some("deadbeef"),
                commit_sha: None,
            },
        )
        .unwrap();
        assert!(ndjson.contains("\"head_sha\":\"deadbeef\""));
        assert!(ndjson.contains("\"conclusion\":\"neutral\""));
        assert!(ndjson.contains("\"annotation_level\":\"warning\""));
        // Out-of-diff finding does not show up as an annotation.
        assert!(!ndjson.contains("docs/old.md"));
    }

    #[test]
    fn summary_handles_zero_findings() {
        let mut record = fixture_record();
        record.report.findings.clear();
        record.report.summary = "No findings.".into();
        let md = render_github_summary(&[record]);
        assert!(md.contains("No findings."));
    }
}
