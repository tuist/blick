//! Helpers for embedding LLM-generated text safely inside `<details>` blocks
//! in PR comments, and for building the agent-facing instruction block we
//! attach to every inline finding.

use crate::review::Finding;

/// Neutralize any literal `</details>` (case-insensitive) inside text that
/// will be embedded in a `<details>` block, so an LLM-generated finding body
/// can't accidentally close the wrapper early. We only touch this one
/// substring rather than full HTML-escaping so code samples with `<T>` or
/// `&` in finding bodies still render normally.
pub fn neutralize_details_close(text: &str) -> String {
    let needle = b"</details";
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut i = 0;
    // The needle is pure ASCII, so any byte that matches its first byte
    // (`<`, 0x3C) is guaranteed to sit on a UTF-8 char boundary — we can
    // safely slice `text[..]` at match positions and copy unmatched runs
    // as string slices, which preserves multi-byte content (emoji, CJK,
    // accents) exactly.
    while i + needle.len() <= bytes.len() {
        if bytes[i..i + needle.len()].eq_ignore_ascii_case(needle) {
            out.push_str(&text[last..i]);
            out.push('<');
            out.push('\\');
            out.push_str(&text[i + 1..i + needle.len()]);
            i += needle.len();
            last = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// Collapsible block embedded in every inline finding comment so an AI agent
/// asked to "address this comment" has a self-contained prompt — the
/// finding's title, body, and location are duplicated *inside* the block so
/// an agent that grabs only the `<details>` content still has everything it
/// needs. Hidden behind a summary by default so the human reviewer doesn't
/// see the duplication unless they expand it.
pub fn agent_instructions(finding: &Finding) -> String {
    let location = match finding.line {
        Some(line) => format!("`{}:{line}`", finding.file),
        None => format!("`{}`", finding.file),
    };
    format!(
        "<details>\n\
         <summary>🤖 Instructions for AI agents</summary>\n\n\
         You are an AI agent asked to address a code review finding. Treat this block as your prompt.\n\n\
         **Finding:** {title}\n\n\
         **Details:**\n\n\
         {body}\n\n\
         **Location:** {location}\n\n\
         **How to fix:**\n\n\
         1. Open {location} and read the surrounding code so you understand the context before changing anything.\n\
         2. Fix the underlying issue described in *Details* above — do not silence the symptom (e.g. by suppressing a warning, catching and discarding an error, or deleting the test that surfaces it).\n\
         3. Run the project's existing test and lint commands and confirm they pass before reporting the task as complete.\n\
         4. Keep the change minimal and focused on this finding; surface any unrelated concerns separately rather than bundling them in.\n\
         5. Once the fix is committed, if the `gh` CLI is available, mark this review thread as resolved so the human reviewer knows it's been addressed — use the GitHub GraphQL `resolveReviewThread` mutation via `gh api graphql` (look up the thread ID for this comment first).\n\n\
         </details>",
        title = neutralize_details_close(&finding.title),
        body = neutralize_details_close(&finding.body),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Severity;

    #[test]
    fn neutralizes_literal_close_tag() {
        // `</details>` in a finding body would close the wrapper early.
        assert_eq!(
            neutralize_details_close("see </details> here"),
            "see <\\/details> here"
        );
    }

    #[test]
    fn neutralization_is_case_insensitive() {
        // LLMs sometimes uppercase tags.
        assert_eq!(neutralize_details_close("</DETAILS>"), "<\\/DETAILS>");
    }

    #[test]
    fn leaves_unrelated_angle_brackets_alone() {
        // Code samples with `<T>` / `&` should render naturally inside the
        // details block — only the close tag is dangerous.
        assert_eq!(
            neutralize_details_close("use Vec<T> & avoid Box"),
            "use Vec<T> & avoid Box"
        );
    }

    #[test]
    fn preserves_multibyte_utf8() {
        // An earlier byte-by-byte implementation corrupted multi-byte chars.
        assert_eq!(
            neutralize_details_close("héllo 日本語 🦀 </details> tail"),
            "héllo 日本語 🦀 <\\/details> tail"
        );
        assert_eq!(neutralize_details_close("日本語"), "日本語");
    }

    #[test]
    fn agent_instructions_embed_finding_location_with_line() {
        let f = Finding {
            severity: Severity::Medium,
            file: "src/foo.rs".into(),
            line: Some(42),
            title: "Title".into(),
            body: "Body".into(),
        };
        let block = agent_instructions(&f);
        assert!(block.contains("`src/foo.rs:42`"));
        assert!(block.contains("Title"));
        assert!(block.contains("Body"));
    }

    #[test]
    fn agent_instructions_omit_line_when_absent() {
        let f = Finding {
            severity: Severity::Low,
            file: "src/foo.rs".into(),
            line: None,
            title: "Title".into(),
            body: "Body".into(),
        };
        let block = agent_instructions(&f);
        assert!(block.contains("`src/foo.rs`"));
        assert!(!block.contains("`src/foo.rs:"));
    }

    #[test]
    fn agent_instructions_neutralize_close_tag_in_body() {
        let f = Finding {
            severity: Severity::High,
            file: "x".into(),
            line: None,
            title: "ok".into(),
            body: "bad </details> here".into(),
        };
        let block = agent_instructions(&f);
        assert!(block.contains("<\\/details>"));
        // The wrapping `<details>` and final `</details>` should still be present.
        assert!(block.starts_with("<details>"));
        assert!(block.ends_with("</details>"));
    }
}
