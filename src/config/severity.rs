//! The shared severity ordering used across config, findings, and rendering.

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    #[value(name = "low")]
    Low,
    #[value(name = "medium")]
    Medium,
    #[value(name = "high")]
    High,
}

impl Severity {
    /// Lowercase tag matching the JSON serialization (`low` / `medium` /
    /// `high`). Use [`Severity::label`] for user-facing rendering.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Capitalized label suitable for human-facing output.
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ord_ranks_low_lt_medium_lt_high() {
        assert!(Severity::Low < Severity::Medium);
        assert!(Severity::Medium < Severity::High);
    }

    #[test]
    fn as_str_matches_serde_form() {
        // `as_str` should match what serde emits — round-trip a couple to be sure.
        for s in [Severity::Low, Severity::Medium, Severity::High] {
            let json = serde_json::to_string(&s).unwrap();
            assert_eq!(json, format!("\"{}\"", s.as_str()));
        }
    }

    #[test]
    fn label_capitalizes() {
        assert_eq!(Severity::Low.label(), "Low");
        assert_eq!(Severity::Medium.label(), "Medium");
        assert_eq!(Severity::High.label(), "High");
    }
}
