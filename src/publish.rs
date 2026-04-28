use std::env;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::error::BlickError;
use crate::render::{self, Format, RenderContext};

#[derive(Debug, Default)]
pub struct PublishArgs {
    pub run: Option<String>,
    pub head_sha: Option<String>,
    pub repo: Option<String>,
    pub pr: Option<u64>,
}

pub struct PublishContext {
    pub repo: String,
    pub head_sha: String,
    pub pr: Option<u64>,
}

pub fn publish(repo_root: &Path, args: PublishArgs) -> Result<(), BlickError> {
    let ctx = resolve_context(args.head_sha.as_deref(), args.repo.as_deref(), args.pr)?;

    // If `blick review` failed before writing a run, there's nothing to
    // render — but in CI (`if: always()` publish step) we still want the PR
    // author to see *something* explaining the failure rather than a red
    // workflow box and silence on the PR. Post a one-line notice and exit
    // cleanly so the publish step doesn't double-fail the workflow.
    let runs_root = repo_root.join(".blick").join("runs");
    let probe = match args.run.as_deref() {
        None | Some("latest") => runs_root.join("latest"),
        Some(other) => runs_root.join(other),
    };
    if !probe.exists() {
        eprintln!(
            "ℹ no run directory at {} — `blick review` likely failed before writing a manifest",
            probe.display()
        );
        if let Some(pr) = ctx.pr {
            let body = build_review_failed_notice(&ctx);
            let payload = serde_json::json!({ "body": body }).to_string();
            let endpoint = format!("repos/{}/issues/{pr}/comments", ctx.repo);
            match gh_api_post(&endpoint, &payload) {
                Ok(()) => eprintln!("✓ posted review-failed notice on #{pr}"),
                Err(err) => eprintln!("⚠ failed to post review-failed notice: {err}"),
            }
        }
        return Ok(());
    }

    let run_dir = crate::run_record::resolve_run_dir(repo_root, args.run.as_deref())?;

    let render_ctx = RenderContext {
        head_sha: Some(&ctx.head_sha),
        commit_sha: Some(&ctx.head_sha),
    };

    let check_runs = render::render(&run_dir, Format::CheckRun, render_ctx.clone())?;
    let mut posted = 0usize;
    for line in check_runs.lines() {
        if line.trim().is_empty() {
            continue;
        }
        gh_api_post(&format!("repos/{}/check-runs", ctx.repo), line)?;
        posted += 1;
    }
    eprintln!(
        "✓ posted {posted} check run{}",
        if posted == 1 { "" } else { "s" }
    );

    if let Some(pr) = ctx.pr {
        // Don't fail the whole publish step on a counting error — the check
        // runs are already up. Surface the warning and skip just the review
        // post so the user still gets the per-review status.
        match render::total_findings(&run_dir) {
            Ok(0) => {
                eprintln!(
                    "ℹ no findings; skipping PR review post (check runs convey the pass status)"
                );
            }
            Ok(_) => {
                let review = render::render(&run_dir, Format::GithubReview, render_ctx)?;
                let endpoint = format!("repos/{}/pulls/{pr}/reviews", ctx.repo);
                match gh_api_post(&endpoint, &review) {
                    Ok(()) => eprintln!("✓ posted PR review on #{pr}"),
                    Err(BlickError::Api(msg)) if msg.contains("Line could not be resolved") => {
                        // GitHub rejected at least one inline comment
                        // because its line isn't in the PR diff as GitHub
                        // computes it (edge of context, `\ No newline`
                        // markers, or LLM-hallucinated lines). Retry without
                        // inline comments — those findings are folded into
                        // the body — so we still post *something* rather
                        // than dropping the review.
                        eprintln!("⚠ inline comments rejected ({msg}); retrying without them");
                        let fallback = strip_inline_comments(&review)?;
                        gh_api_post(&endpoint, &fallback)?;
                        eprintln!("✓ posted PR review on #{pr} (without inline comments)");
                    }
                    Err(err) => return Err(err),
                }
            }
            Err(err) => {
                eprintln!(
                    "⚠ could not count findings ({err}); skipping PR review post but check runs are already up"
                );
            }
        }
    } else {
        eprintln!("ℹ no PR context detected; skipped PR review post");
    }
    Ok(())
}

fn resolve_context(
    head_sha: Option<&str>,
    repo: Option<&str>,
    pr: Option<u64>,
) -> Result<PublishContext, BlickError> {
    let event = read_event_payload();

    let repo = repo
        .map(ToOwned::to_owned)
        .or_else(|| env::var("GITHUB_REPOSITORY").ok())
        .ok_or_else(|| {
            BlickError::Config(
                "could not determine repo (set GITHUB_REPOSITORY or pass --repo owner/repo)"
                    .to_owned(),
            )
        })?;

    let head_sha = head_sha
        .map(ToOwned::to_owned)
        .or_else(|| {
            event
                .as_ref()
                .and_then(|e| e.get("pull_request"))
                .and_then(|pr| pr.get("head"))
                .and_then(|h| h.get("sha"))
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned)
        })
        .or_else(|| env::var("GITHUB_SHA").ok())
        .ok_or_else(|| {
            BlickError::Config(
                "could not determine head SHA (pass --head-sha or run inside GitHub Actions)"
                    .to_owned(),
            )
        })?;

    let pr = pr.or_else(|| {
        event
            .as_ref()
            .and_then(|e| e.get("pull_request"))
            .and_then(|pr| pr.get("number"))
            .and_then(|v| v.as_u64())
    });

    Ok(PublishContext { repo, head_sha, pr })
}

fn build_review_failed_notice(ctx: &PublishContext) -> String {
    let workflow_link = workflow_run_url();
    let mut body = String::from(
        "### Blick review didn't run\n\n\
         The `blick review` step failed before producing a manifest, so there's no \
         review to post on this PR. This usually means the agent (opencode) couldn't \
         start — common causes are an expired or suspended model API key, a missing \
         secret, or the workflow timing out.\n",
    );
    if let Some(url) = workflow_link {
        body.push_str(&format!("\nSee the workflow run for details: {url}\n",));
    } else {
        body.push_str("\nCheck the workflow logs for the underlying error.\n");
    }
    body.push_str(&format!("\n_Commit: `{}`_\n", ctx.head_sha));
    body
}

fn workflow_run_url() -> Option<String> {
    let server = env::var("GITHUB_SERVER_URL").ok()?;
    let repo = env::var("GITHUB_REPOSITORY").ok()?;
    let run_id = env::var("GITHUB_RUN_ID").ok()?;
    Some(format!("{server}/{repo}/actions/runs/{run_id}"))
}

fn read_event_payload() -> Option<Value> {
    let path = env::var("GITHUB_EVENT_PATH").ok()?;
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Rewrite a `github-review` payload with its inline `comments` array
/// emptied and the rejected findings appended to the review `body`.
fn strip_inline_comments(payload: &str) -> Result<String, BlickError> {
    let mut value: Value = serde_json::from_str(payload)
        .map_err(|err| BlickError::Api(format!("review payload is not valid JSON: {err}")))?;

    let comments = value
        .get("comments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if !comments.is_empty() {
        let body = value
            .get("body")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_default();

        let mut appended = body;
        appended.push_str("\n\n#### Inline comments not posted\n");
        appended
            .push_str("_GitHub rejected one or more line comments; reproducing them here._\n\n");
        for comment in &comments {
            let path = comment.get("path").and_then(Value::as_str).unwrap_or("?");
            let line = comment.get("line").and_then(Value::as_u64);
            let body = comment.get("body").and_then(Value::as_str).unwrap_or("");
            match line {
                Some(line) => appended.push_str(&format!("**`{path}:{line}`**\n\n{body}\n\n")),
                None => appended.push_str(&format!("**`{path}`**\n\n{body}\n\n")),
            }
        }
        value["body"] = Value::String(appended);
    }

    value["comments"] = Value::Array(Vec::new());
    Ok(serde_json::to_string(&value).expect("serializable"))
}

fn gh_api_post(api_path: &str, body: &str) -> Result<(), BlickError> {
    let mut child = Command::new("gh")
        .args([
            "api",
            api_path,
            "-X",
            "POST",
            "-H",
            "Accept: application/vnd.github+json",
            "--input",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            BlickError::Api(format!(
                "failed to invoke gh (is it installed and on PATH?): {err}"
            ))
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(body.as_bytes())
            .map_err(|err| BlickError::Api(format!("writing to gh stdin failed: {err}")))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|err| BlickError::Api(format!("waiting on gh failed: {err}")))?;
    if !output.status.success() {
        // `gh` writes the API response body to stdout (even on 4xx) and a
        // short status line to stderr. Surface both so 422s tell us *why*.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(BlickError::Api(format!(
            "gh api {api_path} failed: {} {}",
            stderr.trim(),
            stdout.trim(),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strip_inline_comments_folds_comments_into_body() {
        let payload = json!({
            "commit_id": "deadbeef",
            "event": "COMMENT",
            "body": "### Blick review\n\n1 finding.",
            "comments": [
                {
                    "path": "src/foo.rs",
                    "line": 42,
                    "side": "RIGHT",
                    "body": "**[low]** Title\n\nDetails."
                }
            ]
        })
        .to_string();

        let stripped = strip_inline_comments(&payload).unwrap();
        let value: Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(value["comments"].as_array().unwrap().len(), 0);
        let body = value["body"].as_str().unwrap();
        assert!(body.contains("Inline comments not posted"));
        assert!(body.contains("`src/foo.rs:42`"));
        assert!(body.contains("Details."));
    }

    #[test]
    fn strip_inline_comments_is_a_noop_when_there_are_none() {
        let payload = json!({
            "commit_id": "deadbeef",
            "event": "COMMENT",
            "body": "### Blick review\n\nNo findings.",
            "comments": []
        })
        .to_string();

        let stripped = strip_inline_comments(&payload).unwrap();
        let value: Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(value["body"], "### Blick review\n\nNo findings.");
        assert_eq!(value["comments"].as_array().unwrap().len(), 0);
    }
}
