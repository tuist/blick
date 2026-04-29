//! Building the system + user prompts sent to the agent.

use std::path::Path;

use crate::config::{ReviewEntry, ScopeConfig};
use crate::error::BlickError;
use crate::git::DiffBundle;
use crate::skill::{LoadedSkill, load as load_skill};

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

/// Resolve every skill referenced by `review` against the scope's skill table.
pub(super) fn collect_skills(
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

/// Concatenate the inline `prompt` and the contents of `prompt_file` (if any).
pub(super) fn collect_prompt_addendum(
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

/// Assemble the system prompt: base instructions + skill bodies +
/// review-specific addendum + JSON schema instruction.
pub(super) fn build_system_prompt(skills: &[LoadedSkill], addendum: Option<&str>) -> String {
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

/// Render the user-prompt payload describing the diff to review.
pub(super) fn build_user_prompt(base: &str, diff: &DiffBundle) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(name: &str, body: &str) -> LoadedSkill {
        LoadedSkill {
            name: name.into(),
            path: std::path::PathBuf::from(format!("/skills/{name}.md")),
            body: body.into(),
        }
    }

    #[test]
    fn system_prompt_includes_base_and_schema() {
        let prompt = build_system_prompt(&[], None);
        assert!(prompt.contains("You are Blick"));
        assert!(prompt.contains("\"summary\": \"short summary\""));
    }

    #[test]
    fn system_prompt_inlines_skill_bodies_with_separators() {
        let prompt = build_system_prompt(&[loaded("security", "look for unsafe blocks")], None);
        assert!(prompt.contains("--- skill: security ---"));
        assert!(prompt.contains("look for unsafe blocks"));
    }

    #[test]
    fn system_prompt_drops_blank_addendum() {
        let prompt = build_system_prompt(&[], Some("   \n\n  "));
        // Blank addendum shouldn't add an extra section before the schema.
        assert!(!prompt.contains("\n\n\n\n"));
    }

    #[test]
    fn system_prompt_includes_non_blank_addendum() {
        let prompt = build_system_prompt(&[], Some("focus on security"));
        assert!(prompt.contains("focus on security"));
    }

    #[test]
    fn user_prompt_marks_truncation() {
        let diff = DiffBundle {
            diff: "diff --git a b".into(),
            files: vec!["a".into()],
            truncated: true,
        };
        let prompt = build_user_prompt("origin/main", &diff);
        assert!(prompt.contains("Base revision: origin/main"));
        assert!(prompt.contains("diff was truncated"));
        assert!(prompt.contains("a"));
    }

    #[test]
    fn user_prompt_says_no_files_when_list_is_empty() {
        let diff = DiffBundle {
            diff: String::new(),
            files: vec![],
            truncated: false,
        };
        let prompt = build_user_prompt("HEAD~1", &diff);
        assert!(prompt.contains("git diff did not report any tracked files"));
        assert!(prompt.contains("The diff is complete."));
    }
}
