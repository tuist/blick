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
        let review = render::render(&run_dir, Format::GithubReview, render_ctx)?;
        gh_api_post(&format!("repos/{}/pulls/{pr}/reviews", ctx.repo), &review)?;
        eprintln!("✓ posted PR review on #{pr}");
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

fn read_event_payload() -> Option<Value> {
    let path = env::var("GITHUB_EVENT_PATH").ok()?;
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
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
