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
            command.stdin(Stdio::piped());
            command.stdout(Stdio::piped());
            command.stderr(Stdio::piped());

            let mut child = command
                .spawn()
                .map_err(|err| BlickError::Api(format!("failed to spawn {binary}: {err}")))?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(prompt.as_bytes()).map_err(|err| {
                    BlickError::Api(format!("failed to write opencode stdin: {err}"))
                })?;
            }

            let raw = child
                .wait_with_output()
                .map_err(|err| BlickError::Api(format!("opencode wait failed: {err}")))?;

            let stdout = String::from_utf8_lossy(&raw.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&raw.stderr).into_owned();

            if !raw.status.success() {
                return Err(BlickError::Api(format!(
                    "opencode exited with {}: {}",
                    raw.status,
                    stderr.trim()
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
