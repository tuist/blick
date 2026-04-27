pub mod diff_lines;

use std::path::Path;

use clap::ValueEnum;
use serde_json::{Value, json};

use crate::config::Severity;
use crate::error::BlickError;
use crate::review::Finding;
use crate::run_record::{TaskRecord, list_task_records};

use self::diff_lines::DiffLineIndex;

/// HTML-comment marker embedded in every PR review body so future runs can
/// look up the SHA we last reviewed and only re-review what has changed since.
pub const LAST_REVIEWED_MARKER_PREFIX: &str = "<!-- blick:last-reviewed=";
pub const LAST_REVIEWED_MARKER_SUFFIX: &str = " -->";

/// Extract the SHA encoded in a `blick:last-reviewed=<sha>` marker, if any.
pub fn parse_last_reviewed_marker(body: &str) -> Option<String> {
    let start = body.rfind(LAST_REVIEWED_MARKER_PREFIX)? + LAST_REVIEWED_MARKER_PREFIX.len();
    let rest = &body[start..];
    let end = rest.find(LAST_REVIEWED_MARKER_SUFFIX)?;
    let sha = rest[..end].trim();
    if sha.is_empty() {
        None
    } else {
        Some(sha.to_owned())
    }
}

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

/// Sum of findings across a slice of persisted task records.
fn count_findings(records: &[TaskRecord]) -> usize {
    records.iter().map(|r| r.report.findings.len()).sum()
}

/// Sum of findings across every persisted task in a run.
pub fn total_findings(run_dir: &Path) -> Result<usize, BlickError> {
    Ok(count_findings(&list_task_records(run_dir)?))
}

/// Human label for a `(scope, review)` pair. Drops the `./` prefix when
/// the scope is the repo root so output reads "default" instead of
/// "./default".
fn origin_label(scope_label: &str, review_name: &str) -> String {
    if scope_label == "." {
        review_name.to_owned()
    } else {
        format!("{scope_label}/{review_name}")
    }
}

/// Codex-style shields.io priority badge. `<sub><sub>` shrinks the badge so
/// it sits inline with text without dominating the line.
fn severity_badge(severity: Severity) -> String {
    let (priority, color) = match severity {
        Severity::High => ("P1", "red"),
        Severity::Medium => ("P2", "yellow"),
        Severity::Low => ("P3", "green"),
    };
    format!(
        "<sub><sub>![{priority} Badge](https://img.shields.io/badge/{priority}-{color}?style=flat)</sub></sub>"
    )
}

/// Neutralize any literal `</details>` (case-insensitive) inside text that
/// will be embedded in a `<details>` block, so an LLM-generated finding body
/// can't accidentally close the wrapper early. We only touch this one
/// substring rather than full HTML-escaping so code samples with `<T>` or
/// `&` in finding bodies still render normally.
fn neutralize_details_close(text: &str) -> String {
    let needle = b"</details";
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut i = 0;
    // The needle is pure ASCII, so any byte that matches its first byte
    // (`<`, 0x3C) is guaranteed to sit on a UTF-8 char boundary — we can
    // safely slice `text[..]` at match positions and copy unmatched runs
    // as string slices, which preserves multi-byte content (emoji, CJK,
    // accents) exactly.
    while i + needle.len() <= bytes.len() {
        if bytes[i..i + needle.len()].eq_ignore_ascii_case(needle) {
            out.push_str(&text[last..i]);
            out.push('<');
            out.push('\\');
            out.push_str(&text[i + 1..i + needle.len()]);
            i += needle.len();
            last = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// Collapsible block embedded in every inline finding comment so an AI agent
/// asked to "address this comment" has a self-contained prompt — the
/// finding's title, body, and location are duplicated *inside* the block so
/// an agent that grabs only the `<details>` content still has everything it
/// needs. Hidden behind a summary by default so the human reviewer doesn't
/// see the duplication unless they expand it.
fn agent_instructions(finding: &Finding) -> String {
    let location = match finding.line {
        Some(line) => format!("`{}:{line}`", finding.file),
        None => format!("`{}`", finding.file),
    };
    format!(
        "<details>\n\
         <summary>🤖 Instructions for AI agents</summary>\n\n\
         You are an AI agent asked to address a code review finding. Treat this block as your prompt.\n\n\
         **Finding:** {title}\n\n\
         **Details:**\n\n\
         {body}\n\n\
         **Location:** {location}\n\n\
         **How to fix:**\n\n\
         1. Open {location} and read the surrounding code so you understand the context before changing anything.\n\
         2. Fix the underlying issue described in *Details* above — do not silence the symptom (e.g. by suppressing a warning, catching and discarding an error, or deleting the test that surfaces it).\n\
         3. Run the project's existing test and lint commands and confirm they pass before reporting the task as complete.\n\
         4. Keep the change minimal and focused on this finding; surface any unrelated concerns separately rather than bundling them in.\n\
         5. Once the fix is committed, if the `gh` CLI is available, mark this review thread as resolved so the human reviewer knows it's been addressed — use the GitHub GraphQL `resolveReviewThread` mutation via `gh api graphql` (look up the thread ID for this comment first).\n\n\
         </details>",
        title = neutralize_details_close(&finding.title),
        body = neutralize_details_close(&finding.body),
    )
}

const BLICK_FOOTER_LINK: &str = "[Blick](https://github.com/tuist/blick)";

fn render_github_review(
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
                "- {} {} - {}\n",
                severity_badge(finding.severity),
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
        let origin = origin_label(&record.scope_label, &record.review_name);
        let payload = json!({
            "name": format!("blick / {origin}"),
            "head_sha": head_sha,
            "status": "completed",
            "conclusion": conclusion,
            "output": {
                "title": format!("{origin} review · {conclusion}"),
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
        let origin = origin_label(&record.scope_label, &record.review_name);
        lines.push(String::new());
        lines.push(format!("#### {origin} review"));
        lines.push(record.report.summary.clone());
        if !record.report.findings.is_empty() {
            lines.push(String::new());
            lines.push("| Priority | File | Title |".to_owned());
            lines.push("| --- | --- | --- |".to_owned());
            for finding in &record.report.findings {
                let location = match finding.line {
                    Some(line) => format!("`{}:{line}`", finding.file),
                    None => format!("`{}`", finding.file),
                };
                lines.push(format!(
                    "| {} | {} | {} |",
                    severity_badge(finding.severity),
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
    fn neutralizes_details_close_tag_in_embedded_text() {
        // `</details>` in a finding body would close the wrapper early.
        assert_eq!(
            neutralize_details_close("see </details> here"),
            "see <\\/details> here"
        );
        // Case-insensitive — LLMs sometimes uppercase tags.
        assert_eq!(neutralize_details_close("</DETAILS>"), "<\\/DETAILS>");
        // Code samples with unrelated `<T>` / `&` are left alone so they
        // still render naturally inside the details block.
        assert_eq!(
            neutralize_details_close("use Vec<T> & avoid Box"),
            "use Vec<T> & avoid Box"
        );
        // Multi-byte UTF-8 (emoji, CJK, accents) must be preserved verbatim
        // alongside any neutralized close tag — earlier byte-by-byte
        // implementation corrupted these.
        assert_eq!(
            neutralize_details_close("héllo 日本語 🦀 </details> tail"),
            "héllo 日本語 🦀 <\\/details> tail"
        );
        assert_eq!(neutralize_details_close("日本語"), "日本語");
    }

    #[test]
    fn parses_last_reviewed_marker_from_body() {
        let body = "### Blick review\n\n…\n\n<!-- blick:last-reviewed=abc1234 -->";
        assert_eq!(parse_last_reviewed_marker(body).as_deref(), Some("abc1234"));
        assert!(parse_last_reviewed_marker("no marker here").is_none());
        // When multiple markers are present (e.g. from edits), take the last one.
        let body =
            "<!-- blick:last-reviewed=oldsha -->\nlater\n<!-- blick:last-reviewed=newsha -->";
        assert_eq!(parse_last_reviewed_marker(body).as_deref(), Some("newsha"));
    }

    #[test]
    fn origin_label_drops_root_dot() {
        assert_eq!(origin_label(".", "default"), "default");
        assert_eq!(origin_label("apps/web", "security"), "apps/web/security");
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
