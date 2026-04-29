//! Plain-markdown rollup suitable for `gh pr comment --body-file -` or any
//! chat/email integration. Unlike `github-review`, this format never emits
//! JSON and doesn't need a commit SHA.

use crate::run_record::TaskRecord;

use super::badges::severity_badge;
use super::origin::origin_label;

pub(super) fn render_github_summary(records: &[TaskRecord]) -> String {
    let total: usize = records.iter().map(|r| r.report.findings.len()).sum();
    let mut lines: Vec<String> = vec!["### Blick review".to_owned(), String::new()];

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Severity;
    use crate::review::{Finding, ReviewReport};

    fn empty_record(scope: &str, name: &str) -> TaskRecord {
        TaskRecord {
            run_id: "test".into(),
            scope_label: scope.into(),
            review_name: name.into(),
            base: "origin/main".into(),
            files: vec![],
            diff: String::new(),
            report: ReviewReport {
                summary: "No findings.".into(),
                findings: vec![],
            },
        }
    }

    #[test]
    fn says_no_findings_when_empty() {
        let md = render_github_summary(&[empty_record(".", "default")]);
        assert!(md.contains("No findings."));
    }

    #[test]
    fn pluralizes_findings_and_reviews() {
        let mut r1 = empty_record("a", "x");
        r1.report.findings.push(Finding {
            severity: Severity::Low,
            file: "f.rs".into(),
            line: Some(1),
            title: "t".into(),
            body: "b".into(),
        });
        let mut r2 = empty_record("b", "y");
        r2.report.findings.push(Finding {
            severity: Severity::Medium,
            file: "g.rs".into(),
            line: Some(2),
            title: "t2".into(),
            body: "b".into(),
        });
        let md = render_github_summary(&[r1, r2]);
        assert!(md.contains("2 findings across 2 reviews."));
    }

    #[test]
    fn uses_singular_for_one_finding_and_one_review() {
        let mut r = empty_record(".", "default");
        r.report.findings.push(Finding {
            severity: Severity::Low,
            file: "f.rs".into(),
            line: None,
            title: "t".into(),
            body: "b".into(),
        });
        let md = render_github_summary(&[r]);
        assert!(md.contains("1 finding across 1 review."));
    }

    #[test]
    fn omits_findings_table_when_review_has_none() {
        let md = render_github_summary(&[empty_record(".", "default")]);
        assert!(!md.contains("| Priority |"));
    }
}
