use serde::{Deserialize, Serialize};

use crate::error::BlickError;
use crate::git::DiffBundle;
use crate::llm::ReviewClient;
use crate::review::{ReviewReport, parse_report};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewWorkflow {
    #[serde(default = "default_steps")]
    pub steps: Vec<WorkflowStep>,
}

impl ReviewWorkflow {
    pub fn is_default(&self) -> bool {
        self.steps == default_steps()
    }
}

impl Default for ReviewWorkflow {
    fn default() -> Self {
        Self {
            steps: default_steps(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkflowStep {
    Prompt { role: PromptRole, content: String },
    LlmReview,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PromptRole {
    System,
    User,
}

struct WorkflowContext<'a> {
    base: &'a str,
    diff: &'a DiffBundle,
}

impl WorkflowContext<'_> {
    fn template_value(&self, name: &str) -> Option<String> {
        match name {
            "base" => Some(self.base.to_owned()),
            "diff" => Some(self.diff.diff.clone()),
            "files" => Some(if self.diff.files.is_empty() {
                "(git diff did not report any tracked files)".to_owned()
            } else {
                self.diff.files.join("\n")
            }),
            "truncated_note" => Some(if self.diff.truncated {
                "The diff was truncated to stay within the configured limit.".to_owned()
            } else {
                "The diff is complete.".to_owned()
            }),
            _ => None,
        }
    }
}

pub async fn run_review_workflow(
    client: &dyn ReviewClient,
    workflow: &ReviewWorkflow,
    base: &str,
    diff: &DiffBundle,
) -> Result<ReviewReport, BlickError> {
    let context = WorkflowContext { base, diff };
    let mut system_prompt = String::new();
    let mut user_prompt = String::new();
    let mut report = None;

    for step in &workflow.steps {
        match step {
            WorkflowStep::Prompt { role, content } => {
                let rendered = render_template(content, &context);
                match role {
                    PromptRole::System => append_prompt(&mut system_prompt, &rendered),
                    PromptRole::User => append_prompt(&mut user_prompt, &rendered),
                }
            }
            WorkflowStep::LlmReview => {
                let raw = client.review(&system_prompt, &user_prompt).await?;
                report = Some(parse_report(&raw)?);
            }
        }
    }

    report.ok_or_else(|| {
        BlickError::Config("review workflow must contain at least one llm_review step".to_owned())
    })
}

fn append_prompt(buffer: &mut String, chunk: &str) {
    if chunk.trim().is_empty() {
        return;
    }

    if !buffer.is_empty() {
        buffer.push_str("\n\n");
    }
    buffer.push_str(chunk.trim());
}

fn render_template(template: &str, context: &WorkflowContext<'_>) -> String {
    let mut rendered = template.to_owned();
    for name in ["base", "truncated_note", "files", "diff"] {
        let placeholder = format!("{{{{{name}}}}}");
        if let Some(value) = context.template_value(name) {
            rendered = rendered.replace(&placeholder, &value);
        }
    }
    rendered
}

fn default_steps() -> Vec<WorkflowStep> {
    vec![
        WorkflowStep::Prompt {
            role: PromptRole::System,
            content: r#"You are Blick, a careful code review agent.

Review the provided git diff for correctness issues, regressions, security problems, maintainability risks, and meaningful testing gaps.
Only use the diff and file list provided below.
Do not assume access to the repository, filesystem, tools, or test results.
Do not claim to have run commands, opened files, or inspected code beyond the supplied diff.

Return only valid JSON with this shape:
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

Do not wrap the JSON in markdown fences."#
                .to_owned(),
        },
        WorkflowStep::Prompt {
            role: PromptRole::User,
            content: r#"Base revision: {{base}}
{{truncated_note}}

Changed files:
{{files}}

Unified diff:
{{diff}}
"#
            .to_owned(),
        },
        WorkflowStep::LlmReview,
    ]
}

#[cfg(test)]
mod tests {
    use super::{PromptRole, ReviewWorkflow, WorkflowStep, run_review_workflow};
    use crate::error::BlickError;
    use crate::git::DiffBundle;
    use crate::llm::ReviewClient;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    struct StubClient {
        response: String,
        captures: Arc<Mutex<Vec<(String, String)>>>,
    }

    #[async_trait]
    impl ReviewClient for StubClient {
        async fn review(
            &self,
            system_prompt: &str,
            user_prompt: &str,
        ) -> Result<String, BlickError> {
            self.captures
                .lock()
                .expect("captures mutex should not be poisoned")
                .push((system_prompt.to_owned(), user_prompt.to_owned()));
            Ok(self.response.clone())
        }
    }

    #[tokio::test]
    async fn default_workflow_renders_diff_context() {
        let captures = Arc::new(Mutex::new(Vec::new()));
        let client = StubClient {
            response: r#"{"summary":"No findings.","findings":[]}"#.to_owned(),
            captures: captures.clone(),
        };
        let diff = DiffBundle {
            files: vec!["src/main.rs".to_owned(), "README.md".to_owned()],
            diff: "diff --git a/src/main.rs b/src/main.rs".to_owned(),
            truncated: false,
        };

        let report = run_review_workflow(&client, &ReviewWorkflow::default(), "HEAD", &diff)
            .await
            .expect("workflow should succeed");

        assert_eq!(report.summary, "No findings.");

        let captures = captures
            .lock()
            .expect("captures mutex should not be poisoned");
        let (system_prompt, user_prompt) = &captures[0];
        assert!(system_prompt.contains("You are Blick"));
        assert!(user_prompt.contains("Base revision: HEAD"));
        assert!(user_prompt.contains("src/main.rs"));
        assert!(user_prompt.contains("diff --git a/src/main.rs"));
    }

    #[tokio::test]
    async fn custom_workflow_supports_multiple_deterministic_steps() {
        let captures = Arc::new(Mutex::new(Vec::new()));
        let client = StubClient {
            response: r#"{"summary":"No findings.","findings":[]}"#.to_owned(),
            captures: captures.clone(),
        };
        let workflow = ReviewWorkflow {
            steps: vec![
                WorkflowStep::Prompt {
                    role: PromptRole::System,
                    content: "System step A".to_owned(),
                },
                WorkflowStep::Prompt {
                    role: PromptRole::System,
                    content: "System step B".to_owned(),
                },
                WorkflowStep::Prompt {
                    role: PromptRole::User,
                    content: "Files:\n{{files}}".to_owned(),
                },
                WorkflowStep::LlmReview,
            ],
        };
        let diff = DiffBundle {
            files: vec!["src/lib.rs".to_owned()],
            diff: String::new(),
            truncated: false,
        };

        run_review_workflow(&client, &workflow, "HEAD", &diff)
            .await
            .expect("workflow should succeed");

        let captures = captures
            .lock()
            .expect("captures mutex should not be poisoned");
        let (system_prompt, user_prompt) = &captures[0];
        assert!(system_prompt.contains("System step A"));
        assert!(system_prompt.contains("System step B"));
        assert!(user_prompt.contains("src/lib.rs"));
    }

    #[tokio::test]
    async fn workflow_requires_at_least_one_llm_step() {
        let client = StubClient {
            response: String::new(),
            captures: Arc::new(Mutex::new(Vec::new())),
        };
        let workflow = ReviewWorkflow {
            steps: vec![WorkflowStep::Prompt {
                role: PromptRole::User,
                content: "hello".to_owned(),
            }],
        };
        let diff = DiffBundle {
            files: Vec::new(),
            diff: String::new(),
            truncated: false,
        };

        let error = run_review_workflow(&client, &workflow, "HEAD", &diff)
            .await
            .expect_err("workflow should fail");

        assert_eq!(
            error.to_string(),
            "review workflow must contain at least one llm_review step"
        );
    }
}
