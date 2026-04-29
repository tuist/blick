//! Run the configured agent against the bucketed thread snapshot and parse
//! its proposal back out.

use serde_json::json;

use crate::agent::runner_for;
use crate::config::ScopeConfig;
use crate::error::BlickError;

use super::allowlist::AllowlistFile;
use super::buckets::ThreadBuckets;
use super::proposal::{LearnProposal, parse_proposal};

const LEARN_SYSTEM_PROMPT: &str = r#"You are Blick's reflection pass. Your job is to inspect how Blick's past code reviews landed with human reviewers and propose improvements to Blick's own review setup (skills, prompts, agent instructions).

You will be given:
- A bucketed list of review threads from recent merged PRs. Each thread carries the full sequence of comments (opening comment first, then replies), with each comment's author and whether it was authored by a Bot or User:
  - blick_resolved: thread opened by Blick, then resolved by a human (suggestion accepted/addressed). Replies on these threads can carry nuance — a "this is wrong because…" reply means the human resolved it dismissively, not approvingly.
  - blick_unresolved: thread opened by Blick, still open on a merged PR (suggestion likely ignored or rejected). Replies often contain the human's reason.
  - blick_outdated: thread opened by Blick that GitHub flagged as out of date (ambiguous; weight low).
  - human_authored: thread opened by a human reviewer. This is the strongest signal of what Blick MISSED — categories of feedback humans care about that the current skills don't surface.
- A snapshot of every file you are allowed to edit (blick.toml, AGENTS.md / CLAUDE.md, and anything under .blick/skills/).

What to look for:
- Recurring blick suggestion types that are repeatedly *ignored* (blick_unresolved, or blick_resolved with a dismissive human reply) — relax or remove the corresponding rule.
- Recurring blick suggestion types that are repeatedly *accepted* (blick_resolved without a pushback reply) but phrased poorly (verbose, repetitive, off-tone) — tighten the relevant skill.
- Recurring themes in human_authored threads that Blick is missing entirely — propose a new local skill under .blick/skills/<name>/SKILL.md and reference it from a review in blick.toml.
- Disagreements voiced in replies — a clear "no, X is fine because Y" from a human is strong evidence to weaken or scope down the rule that triggered it.

Skill design constraints (very important):
- Any new or edited skill must be GENERIC and reusable across PRs, not a fix for one specific incident. If you can't articulate the rule without naming a particular file, function, or PR, the pattern is not yet generalizable — drop it instead of encoding a one-off.
- New skills must be COHESIVE with the existing setup. Read the snapshot of the current skills and review prompts before proposing anything. Prefer extending an existing skill over adding a new one; only create a new skill when the topic is genuinely orthogonal to what's already there. Match the tone, structure, and granularity of the skills already in place.
- A theme needs at least two independent supporting threads (across different PRs) before it justifies a skill change. One-off feedback is not a pattern.

What NOT to do:
- Do not propose edits to source code, tests, CI, or anything outside the allowlist.
- Do not propose speculative edits without clear thread evidence — if a theme has fewer than two supporting threads, drop it.
- Do not rewrite files end-to-end when a small, surgical change is sufficient. Always return the FULL final contents of any file you change, but keep the actual diff minimal.
- Do not invent thread URLs. Only cite URLs that appear in the input.

Output ONLY a JSON object of this shape, no markdown fences:
{
  "themes": [
    {
      "title": "short label",
      "rationale": "1-3 sentences explaining the pattern and what change you propose",
      "evidence": ["https://github.com/.../pull/.../#discussion_r123", "..."]
    }
  ],
  "edits": [
    {
      "path": "blick.toml",
      "contents": "<full new file contents>"
    },
    {
      "path": ".blick/skills/<name>/SKILL.md",
      "contents": "<full file contents>"
    }
  ]
}

If there is not enough signal to justify any change, return {"themes":[],"edits":[]}.
"#;

pub(super) async fn run_agent_pass(
    scope: &ScopeConfig,
    gh_repo: &str,
    buckets: &ThreadBuckets,
    allowlist: &[AllowlistFile],
    lookback_days: u32,
) -> Result<LearnProposal, BlickError> {
    let runner = runner_for(&scope.agent)?;
    let system_prompt = LEARN_SYSTEM_PROMPT.to_owned();
    let user_prompt = build_learn_user_prompt(gh_repo, buckets, allowlist, lookback_days);
    let run = runner.run(&system_prompt, &user_prompt).await?;
    parse_proposal(&run.text)
}

fn build_learn_user_prompt(
    gh_repo: &str,
    buckets: &ThreadBuckets,
    allowlist: &[AllowlistFile],
    lookback_days: u32,
) -> String {
    let bucketed = json!({
        "repo": gh_repo,
        "lookback_days": lookback_days,
        "blick_resolved": buckets.blick_resolved,
        "blick_unresolved": buckets.blick_unresolved,
        "blick_outdated": buckets.blick_outdated,
        "human_authored": buckets.human_authored,
    });
    let mut out = String::new();
    out.push_str("Past blick review threads (bucketed):\n\n");
    out.push_str(&serde_json::to_string_pretty(&bucketed).unwrap_or_default());
    out.push_str("\n\nCurrent allowlist files:\n\n");
    for file in allowlist {
        out.push_str(&format!("--- path: {} ---\n", file.path));
        out.push_str(&file.contents);
        if !file.contents.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("--- end ---\n\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_prompt_includes_repo_lookback_and_allowlist_paths() {
        let buckets = ThreadBuckets::default();
        let files = vec![
            AllowlistFile {
                path: "blick.toml".into(),
                contents: "x = 1\n".into(),
            },
            AllowlistFile {
                path: ".blick/skills/foo/SKILL.md".into(),
                contents: "# foo".into(),
            },
        ];
        let prompt = build_learn_user_prompt("tuist/blick", &buckets, &files, 7);
        assert!(prompt.contains("tuist/blick"));
        assert!(prompt.contains("\"lookback_days\": 7"));
        assert!(prompt.contains("--- path: blick.toml ---"));
        assert!(prompt.contains(".blick/skills/foo/SKILL.md"));
    }

    #[test]
    fn user_prompt_appends_newline_when_file_lacks_one() {
        let buckets = ThreadBuckets::default();
        let files = vec![AllowlistFile {
            path: "x".into(),
            contents: "no trailing newline".into(),
        }];
        let prompt = build_learn_user_prompt("o/r", &buckets, &files, 1);
        // Expect the synthetic newline before `--- end ---`.
        assert!(prompt.contains("no trailing newline\n--- end ---"));
    }
}
