//! `POST /repos/.../check-runs` payloads (one per `(scope, review)` pair).

use serde_json::{Value, json};

use crate::config::Severity;
use crate::error::BlickError;
use crate::run_record::TaskRecord;

use super::badges::severity_to_annotation_level;
use super::diff_lines::DiffLineIndex;
use super::origin::origin_label;
use super::RenderContext;

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
    serde_json::to_string(&payload).expect("serializable")
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
        assert!(!ndjson.contains("docs/old.md"));
    }

    #[test]
    fn skips_findings_without_a_line() {
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
        // No annotations rendered.
        assert!(!ndjson.contains("annotation_level"));
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
