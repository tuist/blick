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

            let stdin_write_err = {
                let mut stdin = child
                    .stdin
                    .take()
                    .ok_or_else(|| BlickError::Api(format!("failed to open stdin for {binary}")))?;
                stdin.write_all(prompt.as_bytes()).err()
            };

            // If writing the prompt failed (e.g. opencode exited early on
            // bad flags / auth and closed its stdin → BrokenPipe), don't `?`
            // out before reaping: that drops `Child` unwaited (zombie risk on
            // long-running review jobs) and loses the real cause sitting in
            // stderr. Wait the process out, then build an error that includes
            // whatever it printed.
            let raw = child
                .wait_with_output()
                .map_err(|err| BlickError::Api(format!("failed to wait for {binary}: {err}")))?;

            if let Some(err) = stdin_write_err {
                let stderr = String::from_utf8_lossy(&raw.stderr);
                let detail = extract_opencode_error(&stderr).unwrap_or_else(|| {
                    let trimmed = stderr.trim();
                    if trimmed.is_empty() {
                        format!("exit {}", raw.status)
                    } else {
                        format!("exit {}: {trimmed}", raw.status)
                    }
                });
                return Err(BlickError::Api(format!(
                    "failed to write prompt to {binary} stdin ({err}); {binary} {detail}"
                )));
            }

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
                // opencode 1.x exits 0 even on auth/billing/config failures
                // and writes its `Error: ...` message to stderr. When stdout
                // is empty, prefer that line over a generic "empty output"
                // so the surface message names the real cause.
                let detail = if let Some(err) = extract_opencode_error(&stderr) {
                    err
                } else if stderr.trim().is_empty() {
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

/// Pull the last `Error:` message out of opencode's stderr.
///
/// opencode renders progress and errors with ANSI color codes and prints
/// errors as `\x1b[91m\x1b[1mError:\x1b[0m <message>` (sometimes with the
/// space eaten by adjacent escapes). We strip the escapes, find the last
/// `Error:` token, and return the rest of that line — which is what a human
/// would actually want to read.
fn extract_opencode_error(stderr: &str) -> Option<String> {
    let plain = strip_ansi(stderr);
    let idx = plain.rfind("Error:")?;
    let after = plain[idx + "Error:".len()..].trim_start();
    let line = after.lines().next()?.trim();
    if line.is_empty() {
        None
    } else {
        Some(line.to_owned())
    }
}

/// Minimal ANSI CSI stripper. Removes `ESC [ <params> <final>` sequences
/// (final byte 0x40..=0x7E) and bare `ESC` characters. Sufficient for
/// opencode's color output; not a general-purpose terminal emulator.
fn strip_ansi(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            i += 1;
            if i < bytes.len() && bytes[i] == b'[' {
                i += 1;
                while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            continue;
        }
        // Safe: we only advance by full UTF-8 codepoints when not in escape.
        let ch_len = utf8_char_len(bytes[i]);
        if let Ok(s) = std::str::from_utf8(&bytes[i..i + ch_len]) {
            out.push_str(s);
        }
        i += ch_len;
    }
    out
}

fn utf8_char_len(b: u8) -> usize {
    // ASCII (0x00..=0x7f) and stray continuation bytes (0x80..=0xbf) both
    // advance one byte; multi-byte leads encode their length in the top
    // bits.
    if b < 0xc0 {
        1
    } else if b < 0xe0 {
        2
    } else if b < 0xf0 {
        3
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_error_from_ansi_colored_stderr() {
        let stderr = "Performing one time database migration...\n\u{1b}[0m\n> build · accounts/fireworks/models/kimi-k2p5\n\u{1b}[0m\n\u{1b}[91m\u{1b}[1mError:\u{1b}[0m Account tuist is suspended, possibly due to reaching the monthly spending limit or failure to pay past invoices. Please go to https://fireworks.ai/account/billing for more information.\n";
        let extracted = extract_opencode_error(stderr).unwrap();
        assert!(extracted.starts_with("Account tuist is suspended"));
        assert!(extracted.contains("fireworks.ai/account/billing"));
    }

    #[test]
    fn returns_none_when_no_error_line() {
        assert!(extract_opencode_error("just some progress text\n").is_none());
    }
}
