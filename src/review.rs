use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::agent::{AgentRunner, RunOutput};
use crate::config::{ReviewEntry, ScopeConfig, Severity};
use crate::error::BlickError;
use crate::git::DiffBundle;
use crate::skill::{LoadedSkill, load as load_skill};

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

/// Outcome of a single `(scope, review)` execution.
#[derive(Debug, Clone)]
pub struct ReviewOutcome {
    pub report: ReviewReport,
    pub run: RunOutput,
    /// The assembled system prompt sent to the agent — persisted alongside
    /// the log so contributors can confirm skill bodies and overrides made
    /// it in.
    pub system_prompt: String,
}

/// Runs a single review (named bundle of skills) for one scope.
pub async fn run_review(
    runner: &dyn AgentRunner,
    scope: &ScopeConfig,
    review: &ReviewEntry,
    base: &str,
    diff: &DiffBundle,
) -> Result<ReviewOutcome, BlickError> {
    let skills = collect_skills(scope, review)?;
    let prompt_addendum = collect_prompt_addendum(scope, review)?;
    let system_prompt = build_system_prompt(&skills, prompt_addendum.as_deref());
    let user_prompt = build_user_prompt(base, diff);

    let run = runner.run(&system_prompt, &user_prompt).await?;
    let report = parse_report(&run.text)?;
    Ok(ReviewOutcome {
        report,
        run,
        system_prompt,
    })
}

fn collect_skills(
    scope: &ScopeConfig,
    review: &ReviewEntry,
) -> Result<Vec<LoadedSkill>, BlickError> {
    let mut loaded = Vec::with_capacity(review.skills.len());
    for name in &review.skills {
        let entry = scope.skills.get(name).ok_or_else(|| {
            BlickError::Config(format!(
                "review {} references unknown skill {name}",
                review.name
            ))
        })?;
        loaded.push(load_skill(entry)?);
    }
    Ok(loaded)
}

fn collect_prompt_addendum(
    scope: &ScopeConfig,
    review: &ReviewEntry,
) -> Result<Option<String>, BlickError> {
    let mut chunks = Vec::new();
    if let Some(inline) = &review.prompt {
        chunks.push(inline.trim().to_owned());
    }
    if let Some(prompt_path) = &review.prompt_file {
        let path: &Path = prompt_path.as_path();
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            scope.root.join(path)
        };
        let body = std::fs::read_to_string(&absolute).map_err(|err| {
            BlickError::Config(format!(
                "review {} prompt_file {}: {err}",
                review.name,
                absolute.display()
            ))
        })?;
        chunks.push(body.trim().to_owned());
    }
    if chunks.is_empty() {
        Ok(None)
    } else {
        Ok(Some(chunks.join("\n\n")))
    }
}

fn build_system_prompt(skills: &[LoadedSkill], addendum: Option<&str>) -> String {
    let mut parts = vec![BASE_SYSTEM_PROMPT.to_owned()];

    if !skills.is_empty() {
        parts.push("Apply the following skills (analyses) to the diff:".to_owned());
        for skill in skills {
            parts.push(format!(
                "--- skill: {} ---\n{}",
                skill.name,
                skill.body.trim()
            ));
        }
    }

    if let Some(extra) = addendum
        && !extra.trim().is_empty()
    {
        parts.push(extra.trim().to_owned());
    }

    parts.push(JSON_SCHEMA_INSTRUCTION.to_owned());
    parts.join("\n\n")
}

fn build_user_prompt(base: &str, diff: &DiffBundle) -> String {
    let truncated_note = if diff.truncated {
        "The diff was truncated to stay within the configured limit."
    } else {
        "The diff is complete."
    };
    let files = if diff.files.is_empty() {
        "(git diff did not report any tracked files)".to_owned()
    } else {
        diff.files.join("\n")
    };

    format!(
        "Base revision: {base}\n{truncated_note}\n\nChanged files:\n{files}\n\nUnified diff:\n{}\n",
        diff.diff
    )
}

const BASE_SYSTEM_PROMPT: &str = r#"You are Blick, a careful code review agent.

Review the provided git diff for correctness issues, regressions, security problems, maintainability risks, and meaningful testing gaps.
Only use the diff and file list provided. Do not assume access to the repository, filesystem, tools, or test results."#;

const JSON_SCHEMA_INSTRUCTION: &str = r#"Return only valid JSON with this shape:
{
  "summary": "short summary",
  "findings": [
    {
      "severity": "high|medium|low",
      "file": "path/to/file",
      "line": 123,
      "title": "short issue title",
      "body": "why this matters and what should change"
    }
  ]
}

If there are no meaningful findings, return:
{"summary":"No findings.","findings":[]}

Do not wrap the JSON in markdown fences."#;

pub fn parse_report(raw: &str) -> Result<ReviewReport, BlickError> {
    if let Ok(report) = serde_json::from_str::<ReviewReport>(raw) {
        return Ok(report);
    }

    if let Some(json) = extract_json_block(raw)
        && let Ok(report) = serde_json::from_str::<ReviewReport>(&json)
    {
        return Ok(report);
    }

    Err(BlickError::Api(format!(
        "agent response was not valid review JSON: {}",
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
            finding.severity.label(),
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

    // Try every `{` in the raw string as a candidate start: walk forward
    // tracking brace depth (with awareness of JSON string literals so a `{`
    // or `}` inside `"..."` doesn't throw the counter off) and return the
    // first balanced object that parses as a `ReviewReport`. This is
    // deliberately more permissive than a single-pass walk so prose with
    // stray braces ahead of the real JSON can't drop the whole report.
    let bytes = raw.as_bytes();
    for (start, _) in raw.match_indices('{') {
        if let Some(end) = balanced_object_end(&bytes[start..])
            && let Some(slice) = raw.get(start..start + end)
            && serde_json::from_str::<ReviewReport>(slice).is_ok()
        {
            return Some(slice.to_owned());
        }
    }
    None
}

/// Find the byte offset, relative to `bytes`, just past the closing `}` of
/// the first balanced JSON object, treating `"..."` (with `\\` and `\"`
/// escapes) as opaque so braces inside string literals don't perturb the
/// depth counter. Returns `None` if `bytes` doesn't start with `{` or never
/// closes.
fn balanced_object_end(bytes: &[u8]) -> Option<usize> {
    if bytes.first() != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json() {
        let report = parse_report(
            r#"{"summary":"Looks risky.","findings":[{"severity":"high","file":"src/lib.rs","line":12,"title":"panic path","body":"This can panic on empty input."}]}"#,
        )
        .expect("plain json should parse");
        assert_eq!(report.findings.len(), 1);
    }

    #[test]
    fn parses_prose_prefixed_json_with_curly_placeholders_in_body() {
        // Reproduces the CI failure: the agent prefixed its output with
        // free-form prose, and a finding body contained string literals
        // with `{}` and `{err}` formatting placeholders. The previous
        // brace-counter walked into those without tracking string
        // boundaries and never found a balanced close.
        let raw = "I'll analyze this diff…\n\
            Let me reason out loud first.\n\n\
            {\"summary\":\"x\",\"findings\":[\
            {\"severity\":\"low\",\"file\":\"src/learn.rs\",\"line\":89,\
            \"title\":\"silent error\",\
            \"body\":\"`eprintln!(\\\"  ⚠ skipping PR #{}: {err}\\\", pr.number);` — comment\"}\
            ]}\n\nLet me know if you want me to elaborate.";
        let report = parse_report(raw).expect("prose-prefixed JSON should parse");
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].title, "silent error");
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
                body: "Bubble up the agent name so failures are easier to diagnose.".to_owned(),
            }],
        };
        let rendered = render_report(&report, false).expect("human output should render");
        assert!(rendered.contains("[Medium] src/main.rs:8"));
    }
}
