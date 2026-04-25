use serde::{Deserialize, Serialize};

use crate::error::BlickError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewReport {
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

impl ReviewReport {
    pub fn empty(summary: String) -> Self {
        Self {
            summary,
            findings: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub file: String,
    #[serde(default)]
    pub line: Option<u64>,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    High,
    Medium,
    Low,
}

impl Severity {
    fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

pub fn parse_report(raw: &str) -> Result<ReviewReport, BlickError> {
    if let Ok(report) = serde_json::from_str::<ReviewReport>(raw) {
        return Ok(report);
    }

    if let Some(json) = extract_json_block(raw) {
        if let Ok(report) = serde_json::from_str::<ReviewReport>(&json) {
            return Ok(report);
        }
    }

    Err(BlickError::Api(format!(
        "model response was not valid review JSON: {}",
        raw.trim()
    )))
}

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
            finding.severity.as_str(),
            location,
            finding.title
        ));
        lines.push(format!("   {}", finding.body));
    }

    Ok(lines.join("\n"))
}

fn extract_json_block(raw: &str) -> Option<String> {
    if let Some(stripped) = raw.strip_prefix("```") {
        let stripped = stripped
            .split_once('\n')
            .map(|(_, remainder)| remainder)
            .unwrap_or(stripped);
        if let Some((json, _)) = stripped.split_once("\n```") {
            return Some(json.trim().to_owned());
        }
    }

    let start = raw.find('{')?;
    let mut depth = 0usize;

    for (offset, ch) in raw[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = start + offset + ch.len_utf8();
                    return Some(raw[start..end].to_owned());
                }
            }
            _ => {}
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{Finding, ReviewReport, Severity, parse_report, render_report};

    #[test]
    fn parses_plain_json() {
        let report = parse_report(
            r#"{"summary":"Looks risky.","findings":[{"severity":"high","file":"src/lib.rs","line":12,"title":"panic path","body":"This can panic on empty input."}]}"#,
        )
        .expect("plain json should parse");

        assert_eq!(report.findings.len(), 1);
    }

    #[test]
    fn parses_fenced_json() {
        let report = parse_report(
            r#"```json
{"summary":"No findings.","findings":[]}
```"#,
        )
        .expect("fenced json should parse");

        assert!(report.findings.is_empty());
    }

    #[test]
    fn renders_human_output() {
        let report = ReviewReport {
            summary: "One issue found.".to_owned(),
            findings: vec![Finding {
                severity: Severity::Medium,
                file: "src/main.rs".to_owned(),
                line: Some(8),
                title: "Missing error context".to_owned(),
                body: "Bubble up the provider name so failures are easier to diagnose.".to_owned(),
            }],
        };

        let rendered = render_report(&report, false).expect("human output should render");
        assert!(rendered.contains("[medium] src/main.rs:8"));
    }
}
