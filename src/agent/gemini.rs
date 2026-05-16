use std::io::Write;
use std::process::{Command, Stdio};

use async_trait::async_trait;

use crate::agent::{AgentRunner, RunOutput, strip_provider_prefix};
use crate::config::AgentConfig;
use crate::error::BlickError;

pub struct GeminiRunner {
    binary: String,
    model: Option<String>,
    extra_args: Vec<String>,
}

impl GeminiRunner {
    pub fn new(config: &AgentConfig) -> Self {
        Self {
            binary: config.binary.clone().unwrap_or_else(|| "gemini".to_owned()),
            model: config
                .model
                .as_deref()
                .map(strip_provider_prefix)
                .map(str::to_owned),
            extra_args: config.args.clone(),
        }
    }
}

#[async_trait]
impl AgentRunner for GeminiRunner {
    async fn run(&self, system_prompt: &str, user_prompt: &str) -> Result<RunOutput, BlickError> {
        let binary = self.binary.clone();
        let model = self.model.clone();
        let extra = self.extra_args.clone();
        // gemini-cli has no system-prompt flag; concatenate the same way the
        // opencode adapter does.
        let prompt = format!("{system_prompt}\n\n{user_prompt}");

        let output = tokio::task::spawn_blocking(move || -> Result<RunOutput, BlickError> {
            let mut command = Command::new(&binary);
            if let Some(model) = &model {
                command.args(["-m", model.as_str()]);
            }
            // Skip the interactive tool-approval prompt — same intent as
            // opencode's auto-run and claude's --dangerously-skip-permissions.
            command.arg("--approval-mode=yolo");
            // Bypass the workspace-trust gate; gemini-cli otherwise refuses
            // to run headless in any directory the user hasn't trusted
            // interactively, which makes blick unusable in fresh checkouts.
            command.arg("--skip-trust");
            for arg in &extra {
                command.arg(arg);
            }
            // Pipe the prompt via stdin to dodge the OS `ARG_MAX` ceiling on
            // large diffs. gemini-cli reads stdin when no `-p` is supplied.
            command.stdin(Stdio::piped());
            command.stdout(Stdio::piped());
            command.stderr(Stdio::piped());

            let mut child = command
                .spawn()
                .map_err(|err| BlickError::Api(format!("failed to run {binary}: {err}")))?;

            let stdin_write_err = {
                let mut stdin = child
                    .stdin
                    .take()
                    .ok_or_else(|| BlickError::Api(format!("failed to open stdin for {binary}")))?;
                stdin.write_all(prompt.as_bytes()).err()
            };

            let raw = child
                .wait_with_output()
                .map_err(|err| BlickError::Api(format!("failed to wait for {binary}: {err}")))?;

            if let Some(err) = stdin_write_err {
                let stderr = String::from_utf8_lossy(&raw.stderr);
                let trimmed = stderr.trim();
                let detail = if trimmed.is_empty() {
                    format!("exit {}", raw.status)
                } else {
                    format!("exit {}: {trimmed}", raw.status)
                };
                return Err(BlickError::Api(format!(
                    "failed to write prompt to {binary} stdin ({err}); {binary} {detail}"
                )));
            }

            let stdout = String::from_utf8_lossy(&raw.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&raw.stderr).into_owned();

            if !raw.status.success() {
                return Err(BlickError::Api(format!(
                    "gemini exited with {}: {}",
                    raw.status,
                    stderr.trim()
                )));
            }

            if stdout.trim().is_empty() {
                let detail = if stderr.trim().is_empty() {
                    "no stdout and no stderr — is GEMINI_API_KEY set?".to_owned()
                } else {
                    format!("no stdout. stderr: {}", stderr.trim())
                };
                return Err(BlickError::Api(format!(
                    "gemini produced empty output: {detail}"
                )));
            }

            Ok(RunOutput {
                text: stdout.clone(),
                stdout,
                stderr,
            })
        })
        .await
        .map_err(|err| BlickError::Api(format!("gemini join failed: {err}")))??;

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentKind;

    #[test]
    fn strips_provider_prefix_from_model() {
        let runner = GeminiRunner::new(&AgentConfig {
            kind: AgentKind::Gemini,
            model: Some("google/gemini-2.5-pro".to_owned()),
            binary: None,
            args: Vec::new(),
        });
        assert_eq!(runner.model.as_deref(), Some("gemini-2.5-pro"));
    }

    #[test]
    fn defaults_binary_to_gemini() {
        let runner = GeminiRunner::new(&AgentConfig {
            kind: AgentKind::Gemini,
            model: None,
            binary: None,
            args: Vec::new(),
        });
        assert_eq!(runner.binary, "gemini");
        assert!(runner.model.is_none());
    }

    #[test]
    fn honours_binary_and_args_overrides() {
        let runner = GeminiRunner::new(&AgentConfig {
            kind: AgentKind::Gemini,
            model: None,
            binary: Some("/usr/local/bin/gemini-rc".to_owned()),
            args: vec!["--debug".to_owned()],
        });
        assert_eq!(runner.binary, "/usr/local/bin/gemini-rc");
        assert_eq!(runner.extra_args, vec!["--debug".to_owned()]);
    }
}
