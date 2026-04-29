//! `[learn]` block — configures the periodic self-improvement pass.

use serde::{Deserialize, Serialize};

/// Configuration for `blick learn` — the periodic self-improvement pass that
/// inspects past blick reviews and proposes edits to the review setup.
///
/// Only the root scope's `[learn]` block is consulted; learn does not
/// participate in scope inheritance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnConfig {
    /// How many days of merged-PR history to inspect.
    #[serde(default = "default_lookback_days")]
    pub lookback_days: u32,
    /// Minimum number of supporting threads required before learn opens or
    /// updates a PR.
    #[serde(default = "default_min_signal")]
    pub min_signal: u32,
    /// GitHub logins assigned as PR reviewers.
    #[serde(default)]
    pub reviewers: Vec<String>,
    /// GitHub team slugs (`org/team`) assigned as PR reviewers.
    #[serde(default)]
    pub team_reviewers: Vec<String>,
    /// Labels applied to the PR.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Base branch for the PR.
    #[serde(default = "default_learn_base")]
    pub base: String,
    /// Open as a draft PR.
    #[serde(default = "default_true")]
    pub draft: bool,
    /// Branch name learn uses for its rolling PR.
    #[serde(default = "default_learn_branch")]
    pub branch: String,
}

impl Default for LearnConfig {
    fn default() -> Self {
        Self {
            lookback_days: default_lookback_days(),
            min_signal: default_min_signal(),
            reviewers: Vec::new(),
            team_reviewers: Vec::new(),
            labels: Vec::new(),
            base: default_learn_base(),
            draft: default_true(),
            branch: default_learn_branch(),
        }
    }
}

fn default_lookback_days() -> u32 {
    7
}
fn default_min_signal() -> u32 {
    3
}
fn default_learn_base() -> String {
    "main".to_owned()
}
fn default_learn_branch() -> String {
    "blick/learn".to_owned()
}
fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_lookback_is_one_week() {
        assert_eq!(LearnConfig::default().lookback_days, 7);
    }

    #[test]
    fn defaults_to_draft_pr_on_main() {
        let cfg = LearnConfig::default();
        assert!(cfg.draft);
        assert_eq!(cfg.base, "main");
        assert_eq!(cfg.branch, "blick/learn");
    }

    #[test]
    fn parses_minimal_block_using_defaults() {
        // Empty `[learn]` block must not error — every field has a default.
        let parsed: LearnConfig = toml::from_str("").unwrap();
        assert_eq!(parsed.lookback_days, 7);
        assert_eq!(parsed.min_signal, 3);
    }
}
