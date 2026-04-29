//! Fetch the set of merged PRs to consider this learn pass.

use serde_json::Value;

use crate::error::BlickError;

use super::gh::gh_api_get_with_retry;

#[derive(Debug, Clone)]
pub(super) struct PrSummary {
    pub(super) number: u64,
}

/// Fetch numbers of every PR merged in `gh_repo` since `cutoff`, using the
/// search API.
///
/// We deliberately use `search/issues` with `is:merged merged:>=DATE` rather
/// than `repos/.../pulls?state=closed&sort=updated`: the pulls endpoint
/// orders by `updated_at` (no `merged_at` sort exists), which means a PR
/// merged before the cutoff but commented on yesterday appears before a
/// recently-merged PR with no fresh activity, so any "stop when all results
/// on this page are older" optimization can stop too early. The search API
/// filters server-side by merge date so we can paginate without that
/// heuristic.
pub(super) async fn fetch_recent_merged_prs(
    gh_repo: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<PrSummary>, BlickError> {
    let cutoff_date = cutoff.format("%Y-%m-%d").to_string();
    let query = format!("repo:{gh_repo} is:pr is:merged merged:>={cutoff_date}");
    let mut out = Vec::new();
    let mut page = 1u32;
    loop {
        let path = format!(
            "search/issues?q={}&per_page=100&page={page}",
            percent_encode_query(&query),
        );
        let raw = gh_api_get_with_retry(&path).await?;
        let response: Value = serde_json::from_str(&raw)
            .map_err(|err| BlickError::Api(format!("failed to parse search response: {err}")))?;
        let items = response
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let len = items.len();
        for item in items {
            if let Some(number) = item.get("number").and_then(Value::as_u64) {
                out.push(PrSummary { number });
            }
        }
        // The search API caps at 1000 results (10 pages of 100). Stop early
        // when we drain a partial page or hit the cap.
        if len < 100 || page >= 10 {
            break;
        }
        page += 1;
    }
    Ok(out)
}

/// Percent-encode an entire query string for the `q=` parameter of the
/// search API. Only alphanumerics and a small set of unreserved characters
/// pass through untouched; everything else (including spaces and `:`) is
/// `%XX`-encoded so `gh api` doesn't have to interpret it.
fn percent_encode_query(input: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            // `write!` against a `String` is infallible — the `_ =` is a
            // visible nudge that we're intentionally not propagating an
            // error here. Avoids the per-byte `format!` allocation that an
            // earlier version of this function did for every escaped char.
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_separators_and_keeps_unreserved() {
        // Spaces, colons, and `>` (used by `merged:>=DATE`) must be encoded
        // so `gh api` doesn't have to interpret them.
        assert_eq!(
            percent_encode_query("repo:tuist/blick is:pr is:merged merged:>=2026-04-22"),
            "repo%3Atuist%2Fblick%20is%3Apr%20is%3Amerged%20merged%3A%3E%3D2026-04-22"
        );
        // Unreserved characters per RFC 3986 pass through untouched.
        assert_eq!(percent_encode_query("a-Z_0.9~"), "a-Z_0.9~");
    }

    #[test]
    fn encodes_high_bytes_as_uppercase_hex() {
        assert_eq!(percent_encode_query(" "), "%20");
        assert_eq!(percent_encode_query("/"), "%2F");
        assert_eq!(percent_encode_query("é"), "%C3%A9");
    }
}
