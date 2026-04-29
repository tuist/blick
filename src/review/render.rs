//! Human-facing rendering of a [`ReviewReport`] for terminal output (the
//! GitHub-targeted formats live in `crate::render`).

use crate::error::BlickError;

use super::types::ReviewReport;

/// Render `report` for terminal display, or as pretty JSON when `as_json`.
pub fn render_report(report: &ReviewReport, as_json: bool) -> Result<String, BlickError> {
    if as_json {
        return serde_json::to_string_pretty(report)
            .map_err(|error| BlickError::Api(format!("failed to render JSON output: {error}")));
    }

    let mut lines = vec![format!("Summary: {}", report.summary)];

    if report.findings.is_empty() {
        lines.push("No findings.".to_owned());
        return Ok(lines.join("\n"));
    }

    for (index, finding) in report.findings.iter().enumerate() {
        let location = finding
            .line
            .map(|line| format!("{}:{line}", finding.file))
            .unwrap_or_else(|| finding.file.clone());

        lines.push(format!(
            "{}. [{}] {} - {}",
            index + 1,
            finding.severity.label(),
            location,
            finding.title
        ));
        lines.push(format!("   {}", finding.body));
    }

    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Severity;
    use crate::review::types::Finding;

    #[test]
    fn renders_human_output() {
        let report = ReviewReport {
            summary: "One issue found.".to_owned(),
            findings: vec![Finding {
                severity: Severity::Medium,
                file: "src/main.rs".to_owned(),
                line: Some(8),
                title: "Missing error context".to_owned(),
                body: "Bubble up the agent name so failures are easier to diagnose.".to_owned(),
            }],
        };
        let rendered = render_report(&report, false).expect("human output should render");
        assert!(rendered.contains("[Medium] src/main.rs:8"));
    }

    #[test]
    fn renders_no_findings_message_for_empty_report() {
        let report = ReviewReport::empty("All clear.".into());
        let rendered = render_report(&report, false).unwrap();
        assert!(rendered.contains("Summary: All clear."));
        assert!(rendered.contains("No findings."));
    }

    #[test]
    fn json_mode_returns_serialized_report() {
        let report = ReviewReport::empty("ok".into());
        let rendered = render_report(&report, true).unwrap();
        assert!(rendered.contains("\"summary\": \"ok\""));
        assert!(rendered.contains("\"findings\": []"));
    }

    #[test]
    fn human_output_omits_line_when_finding_has_none() {
        let report = ReviewReport {
            summary: "x".into(),
            findings: vec![Finding {
                severity: Severity::Low,
                file: "src/foo.rs".into(),
                line: None,
                title: "t".into(),
                body: "b".into(),
            }],
        };
        let rendered = render_report(&report, false).unwrap();
        assert!(rendered.contains("src/foo.rs - t"));
        assert!(!rendered.contains("src/foo.rs:"));
    }
}
