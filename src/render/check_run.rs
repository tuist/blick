//! `POST /repos/.../check-runs` payloads (one per `(scope, review)` pair).
//!
//! Each check run carries its in-diff findings as annotations and folds
//! the rest into the `output.summary` markdown so out-of-diff findings
//! still surface somewhere — the PR review body no longer carries them.

use serde_json::{Value, json};

use crate::config::Severity;
use crate::error::BlickError;
use crate::review::Finding;
use crate::run_record::TaskRecord;

use super::RenderContext;
use super::badges::{severity_badge, severity_to_annotation_level};
use super::diff_lines::DiffLineIndex;
use super::origin::origin_label;

pub(super) fn render_check_runs(
    records: &[TaskRecord],
    ctx: RenderContext<'_>,
) -> Result<String, BlickError> {
    let head_sha = ctx
        .head_sha
        .or(ctx.commit_sha)
        .ok_or_else(|| BlickError::Config("check-run requires --head-sha".to_owned()))?;

    let mut lines: Vec<String> = Vec::new();
    for record in records {
        lines.push(render_one(record, head_sha));
    }
    Ok(lines.join("\n"))
}

fn render_one(record: &TaskRecord, head_sha: &str) -> String {
    let index = DiffLineIndex::from_unified(&record.diff);
    let mut annotations: Vec<Value> = Vec::new();
    let mut out_of_diff: Vec<&Finding> = Vec::new();
    for finding in &record.report.findings {
        match finding.line {
            Some(line) if index.contains(&finding.file, line) => {
                annotations.push(json!({
                    "path": finding.file,
                    "start_line": line,
                    "end_line": line,
                    "annotation_level": severity_to_annotation_level(finding.severity),
                    "title": finding.title,
                    "message": finding.body,
                }));
            }
            _ => out_of_diff.push(finding),
        }
    }

    let conclusion = conclusion_for(record);
    let summary = build_summary(record, &out_of_diff);
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
    serde_json::to_string(&payload).expect("serializable")
}

fn build_summary(record: &TaskRecord, out_of_diff: &[&Finding]) -> String {
    let total = record.report.findings.len();
    let mut summary = format!(
        "{} ({} finding{})",
        record.report.summary,
        total,
        if total == 1 { "" } else { "s" }
    );

    if !out_of_diff.is_empty() {
        summary.push_str("\n\n#### Findings outside this PR's diff\n");
        for finding in out_of_diff {
            let location = match finding.line {
                Some(line) => format!("`{}:{line}`", finding.file),
                None => format!("`{}`", finding.file),
            };
            summary.push_str(&format!(
                "- {} {} - {}\n",
                severity_badge(finding.severity),
                location,
                finding.title
            ));
        }
    }
    summary
}

/// `failure` if any high-severity finding, `success` if no findings,
/// otherwise `neutral` — non-high findings shouldn't fail the check.
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
    use crate::review::{Finding, ReviewReport};

    fn record(findings: Vec<Finding>) -> TaskRecord {
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
                summary: "summary".into(),
                findings,
            },
        }
    }

    #[test]
    fn renders_annotations_only_for_in_diff_findings() {
        let record = record(vec![
            Finding {
                severity: Severity::Medium,
                file: "src/main.rs".into(),
                line: Some(2),
                title: "in diff".into(),
                body: "body".into(),
            },
            Finding {
                severity: Severity::Low,
                file: "docs/old.md".into(),
                line: Some(99),
                title: "out of diff".into(),
                body: "body".into(),
            },
        ]);
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
        // The out-of-diff finding doesn't appear as an annotation (rejected
        // by the API) but is listed in the summary so reviewers still see it.
        let value: Value = serde_json::from_str(&ndjson).unwrap();
        let annotations = value["output"]["annotations"].as_array().unwrap();
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0]["path"], "src/main.rs");
        let summary = value["output"]["summary"].as_str().unwrap();
        assert!(summary.contains("Findings outside this PR's diff"));
        assert!(summary.contains("docs/old.md:99"));
        assert!(summary.contains("out of diff"));
    }

    #[test]
    fn summary_omits_out_of_diff_section_when_all_findings_are_in_diff() {
        let record = record(vec![Finding {
            severity: Severity::Medium,
            file: "src/main.rs".into(),
            line: Some(2),
            title: "in diff".into(),
            body: "body".into(),
        }]);
        let ndjson = render_check_runs(
            &[record],
            RenderContext {
                head_sha: Some("deadbeef"),
                commit_sha: None,
            },
        )
        .unwrap();
        let value: Value = serde_json::from_str(&ndjson).unwrap();
        let summary = value["output"]["summary"].as_str().unwrap();
        assert!(!summary.contains("Findings outside this PR's diff"));
    }

    #[test]
    fn findings_without_a_line_are_listed_in_the_summary() {
        let record = record(vec![Finding {
            severity: Severity::Medium,
            file: "src/main.rs".into(),
            line: None,
            title: "no line".into(),
            body: "body".into(),
        }]);
        let ndjson = render_check_runs(
            &[record],
            RenderContext {
                head_sha: Some("deadbeef"),
                commit_sha: None,
            },
        )
        .unwrap();
        let value: Value = serde_json::from_str(&ndjson).unwrap();
        // No annotations — without a line GitHub can't anchor one.
        assert!(
            value["output"]["annotations"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        // ...but the finding still shows in the summary.
        let summary = value["output"]["summary"].as_str().unwrap();
        assert!(summary.contains("no line"));
        assert!(summary.contains("`src/main.rs`"));
    }

    #[test]
    fn conclusion_failure_when_any_high_severity() {
        let r = record(vec![Finding {
            severity: Severity::High,
            file: "x".into(),
            line: None,
            title: "t".into(),
            body: "b".into(),
        }]);
        assert_eq!(conclusion_for(&r), "failure");
    }

    #[test]
    fn conclusion_success_when_no_findings() {
        let r = record(vec![]);
        assert_eq!(conclusion_for(&r), "success");
    }

    #[test]
    fn conclusion_neutral_for_low_or_medium_only() {
        let r = record(vec![Finding {
            severity: Severity::Low,
            file: "x".into(),
            line: None,
            title: "t".into(),
            body: "b".into(),
        }]);
        assert_eq!(conclusion_for(&r), "neutral");
    }

    #[test]
    fn missing_head_sha_is_a_config_error() {
        let err = render_check_runs(
            &[record(vec![])],
            RenderContext {
                head_sha: None,
                commit_sha: None,
            },
        )
        .unwrap_err();
        assert!(matches!(err, BlickError::Config(_)));
    }
}
