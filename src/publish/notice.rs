//! The "Blick review didn't run" notice posted when `blick review` failed
//! before producing a manifest. Kept separate from the happy-path flow so
//! the notice text and its tests are easy to find.

use super::context::PublishContext;

/// Build the markdown body posted as an issue comment when
/// `.blick/runs/latest` is missing — i.e. the agent crashed before writing
/// any output. Includes a link to the workflow run when available.
///
/// `workflow_run_url` is injected so this function stays pure and testable
/// without poking at environment variables.
pub(super) fn build_review_failed_notice(
    ctx: &PublishContext,
    workflow_run_url: Option<&str>,
) -> String {
    let mut body = String::from(
        "### Blick review didn't run\n\n\
         The `blick review` step failed before producing a manifest, so there's no \
         review to post on this PR. This usually means the agent (opencode) couldn't \
         start — common causes are an expired or suspended model API key, a missing \
         secret, or the workflow timing out.\n",
    );
    if let Some(url) = workflow_run_url {
        body.push_str(&format!("\nSee the workflow run for details: {url}\n",));
    } else {
        body.push_str("\nCheck the workflow logs for the underlying error.\n");
    }
    body.push_str(&format!("\n_Commit: `{}`_\n", ctx.head_sha));
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> PublishContext {
        PublishContext {
            repo: "tuist/blick".into(),
            head_sha: "abc1234".into(),
            pr: Some(42),
        }
    }

    #[test]
    fn includes_head_sha_for_traceability() {
        let body = build_review_failed_notice(&ctx(), None);
        assert!(body.contains("abc1234"));
        assert!(body.contains("Blick review didn't run"));
    }

    #[test]
    fn falls_back_when_workflow_run_url_is_unavailable() {
        let body = build_review_failed_notice(&ctx(), None);
        assert!(body.contains("Check the workflow logs"));
        assert!(!body.contains("See the workflow run for details"));
    }

    #[test]
    fn includes_workflow_run_link_when_provided() {
        let body =
            build_review_failed_notice(&ctx(), Some("https://github.com/o/r/actions/runs/123"));
        assert!(body.contains("https://github.com/o/r/actions/runs/123"));
        assert!(body.contains("See the workflow run for details"));
    }
}
