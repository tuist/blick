use std::io::Write;
use std::process::{Command, Stdio};

use async_trait::async_trait;

use crate::agent::{AgentRunner, RunOutput};
use crate::config::AgentConfig;
use crate::error::BlickError;

pub struct OpencodeRunner {
    binary: String,
    model: Option<String>,
    extra_args: Vec<String>,
}

impl OpencodeRunner {
    pub fn new(config: &AgentConfig) -> Self {
        Self {
            binary: config
                .binary
                .clone()
                .unwrap_or_else(|| "opencode".to_owned()),
            model: config.model.clone(),
            extra_args: config.args.clone(),
        }
    }
}

#[async_trait]
impl AgentRunner for OpencodeRunner {
    async fn run(&self, system_prompt: &str, user_prompt: &str) -> Result<RunOutput, BlickError> {
        let binary = self.binary.clone();
        let model = self.model.clone();
        let extra = self.extra_args.clone();
        let prompt = format!("{system_prompt}\n\n{user_prompt}");

        let output = tokio::task::spawn_blocking(move || -> Result<RunOutput, BlickError> {
            let mut command = Command::new(&binary);
            command.arg("run");
            if let Some(model) = &model {
                command.args(["--model", model.as_str()]);
            }
            for arg in &extra {
                command.arg(arg);
            }
            // Pipe the prompt via stdin instead of passing it as a positional
            // argument. Large prompts (e.g. PR diffs in big repos) blow past
            // the OS `ARG_MAX` limit and `execve` returns E2BIG before
            // opencode can start. opencode `run` consumes stdin when no
            // positional message is provided.
            command.stdin(Stdio::piped());
            command.stdout(Stdio::piped());
            command.stderr(Stdio::piped());

            let mut child = command
                .spawn()
                .map_err(|err| BlickError::Api(format!("failed to run {binary}: {err}")))?;

            {
                let mut stdin = child
                    .stdin
                    .take()
                    .ok_or_else(|| BlickError::Api(format!("failed to open stdin for {binary}")))?;
                stdin.write_all(prompt.as_bytes()).map_err(|err| {
                    BlickError::Api(format!("failed to write prompt to {binary} stdin: {err}"))
                })?;
            }

            let raw = child
                .wait_with_output()
                .map_err(|err| BlickError::Api(format!("failed to wait for {binary}: {err}")))?;

            let stdout = String::from_utf8_lossy(&raw.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&raw.stderr).into_owned();

            if !raw.status.success() {
                return Err(BlickError::Api(format!(
                    "opencode exited with {}: {}",
                    raw.status,
                    stderr.trim()
                )));
            }

            if stdout.trim().is_empty() {
                let detail = if stderr.trim().is_empty() {
                    "no stdout and no stderr — is the model authenticated?".to_owned()
                } else {
                    format!("no stdout. stderr: {}", stderr.trim())
                };
                return Err(BlickError::Api(format!(
                    "opencode produced empty output: {detail}"
                )));
            }

            Ok(RunOutput {
                text: stdout.clone(),
                stdout,
                stderr,
            })
        })
        .await
        .map_err(|err| BlickError::Api(format!("opencode join failed: {err}")))??;

        Ok(output)
    }
}
