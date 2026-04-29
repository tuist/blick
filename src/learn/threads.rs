//! Fetch the review-thread sequence for a single PR via GraphQL.

use std::process::Command;

use serde::Serialize;
use serde_json::Value;

use crate::error::BlickError;

#[derive(Debug, Clone, Serialize)]
pub(super) struct ReviewThread {
    pub(super) pr_number: u64,
    pub(super) is_resolved: bool,
    pub(super) is_outdated: bool,
    pub(super) path: Option<String>,
    pub(super) line: Option<u64>,
    pub(super) url: Option<String>,
    /// All comments on the thread in chronological order. The first entry is
    /// the comment that opened the thread; later entries are replies.
    pub(super) comments: Vec<ThreadComment>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ThreadComment {
    pub(super) author: String,
    /// `"Bot"` or `"User"`, as reported by the GraphQL API. Useful when we
    /// can't pin down the bot login but still want to distinguish the two.
    pub(super) author_type: String,
    pub(super) body: String,
}

impl ReviewThread {
    pub(super) fn first_body(&self) -> &str {
        self.comments.first().map(|c| c.body.as_str()).unwrap_or("")
    }

    pub(super) fn first_author_type(&self) -> &str {
        self.comments
            .first()
            .map(|c| c.author_type.as_str())
            .unwrap_or("")
    }
}

const QUERY: &str = r#"
query($owner: String!, $name: String!, $pr: Int!, $cursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $pr) {
      reviewThreads(first: 50, after: $cursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          isResolved
          isOutdated
          path
          line
          comments(first: 20) {
            pageInfo { hasNextPage }
            nodes { url body author { login __typename } }
          }
        }
      }
    }
  }
}
"#;

pub(super) fn fetch_review_threads(
    gh_repo: &str,
    pr: u64,
) -> Result<Vec<ReviewThread>, BlickError> {
    let (owner, name) = gh_repo
        .split_once('/')
        .ok_or_else(|| BlickError::Config(format!("expected owner/repo, got {gh_repo}")))?;

    let mut threads = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut args = vec![
            "api".to_owned(),
            "graphql".to_owned(),
            "-f".to_owned(),
            format!("query={QUERY}"),
            "-F".to_owned(),
            format!("owner={owner}"),
            "-F".to_owned(),
            format!("name={name}"),
            "-F".to_owned(),
            format!("pr={pr}"),
        ];
        if let Some(c) = &cursor {
            args.push("-F".to_owned());
            args.push(format!("cursor={c}"));
        }
        let output = Command::new("gh")
            .args(&args)
            .output()
            .map_err(|err| BlickError::Api(format!("failed to run gh: {err}")))?;
        if !output.status.success() {
            return Err(BlickError::Api(format!(
                "gh api graphql failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let value: Value = serde_json::from_str(&String::from_utf8_lossy(&output.stdout))
            .map_err(|err| BlickError::Api(format!("graphql response not JSON: {err}")))?;
        let root = value
            .pointer("/data/repository/pullRequest/reviewThreads")
            .ok_or_else(|| {
                BlickError::Api(format!("graphql response missing reviewThreads: {value}"))
            })?;

        if let Some(nodes) = root.get("nodes").and_then(Value::as_array) {
            for node in nodes {
                if let Some(thread) = parse_thread_node(node, pr) {
                    threads.push(thread);
                }
            }
        }

        let page_info = root.get("pageInfo");
        let has_next = page_info
            .and_then(|p| p.get("hasNextPage"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !has_next {
            break;
        }
        cursor = page_info
            .and_then(|p| p.get("endCursor"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    Ok(threads)
}

/// Parse a single `reviewThreads.nodes[*]` GraphQL value into a
/// [`ReviewThread`]. Returns `None` for threads with zero comments (the
/// API has been observed to occasionally emit these for deleted comments).
fn parse_thread_node(node: &Value, pr: u64) -> Option<ReviewThread> {
    let comment_nodes = node
        .pointer("/comments/nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if comment_nodes.is_empty() {
        return None;
    }
    if node
        .pointer("/comments/pageInfo/hasNextPage")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        eprintln!(
            "  ⚠ PR #{pr} thread has more than {} comments; truncating (later replies are not in the agent's input)",
            comment_nodes.len()
        );
    }

    let mut comments: Vec<ThreadComment> = Vec::with_capacity(comment_nodes.len());
    for c in &comment_nodes {
        comments.push(ThreadComment {
            author: c
                .pointer("/author/login")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            author_type: c
                .pointer("/author/__typename")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            body: c
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        });
    }
    let url = comment_nodes
        .first()
        .and_then(|c| c.get("url"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    Some(ReviewThread {
        pr_number: pr,
        is_resolved: node
            .get("isResolved")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        is_outdated: node
            .get("isOutdated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        path: node
            .get("path")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        line: node.get("line").and_then(Value::as_u64),
        url,
        comments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_thread_node_skips_empty_comments_array() {
        let node = json!({
            "isResolved": false,
            "isOutdated": false,
            "path": "x",
            "line": 1,
            "comments": { "nodes": [], "pageInfo": { "hasNextPage": false } }
        });
        assert!(parse_thread_node(&node, 1).is_none());
    }

    #[test]
    fn parse_thread_node_extracts_url_from_first_comment() {
        let node = json!({
            "isResolved": true,
            "isOutdated": false,
            "path": "src/foo.rs",
            "line": 7,
            "comments": {
                "nodes": [
                    {
                        "url": "https://example/r/1",
                        "body": "x",
                        "author": { "login": "a", "__typename": "User" }
                    }
                ],
                "pageInfo": { "hasNextPage": false }
            }
        });
        let parsed = parse_thread_node(&node, 99).unwrap();
        assert_eq!(parsed.pr_number, 99);
        assert!(parsed.is_resolved);
        assert_eq!(parsed.url.as_deref(), Some("https://example/r/1"));
        assert_eq!(parsed.comments.len(), 1);
        assert_eq!(parsed.comments[0].author_type, "User");
    }

    #[test]
    fn parse_thread_node_treats_missing_optional_fields_as_defaults() {
        let node = json!({
            "comments": {
                "nodes": [
                    {
                        "body": "ok",
                        "author": { "login": "x", "__typename": "Bot" }
                    }
                ],
                "pageInfo": { "hasNextPage": false }
            }
        });
        let parsed = parse_thread_node(&node, 1).unwrap();
        assert!(!parsed.is_resolved);
        assert!(!parsed.is_outdated);
        assert!(parsed.path.is_none());
        assert!(parsed.line.is_none());
        assert!(parsed.url.is_none());
    }
}
