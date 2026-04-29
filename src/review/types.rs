//! Data types describing a review's output.

use serde::{Deserialize, Serialize};

use crate::agent::RunOutput;
use crate::config::Severity;

/// A review's overall verdict plus the individual findings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewReport {
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

impl ReviewReport {
    /// Convenience constructor for the "review didn't actually do anything"
    /// case (no diff, no matching reviews, etc.). Surfaces a summary string
    /// for human output without forcing callers to allocate an empty vec.
    pub fn empty(summary: String) -> Self {
        Self {
            summary,
            findings: Vec::new(),
        }
    }
}

/// A single issue raised by a review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub file: String,
    #[serde(default)]
    pub line: Option<u64>,
    pub title: String,
    pub body: String,
}

/// Outcome of a single `(scope, review)` execution.
#[derive(Debug, Clone)]
pub struct ReviewOutcome {
    pub report: ReviewReport,
    pub run: RunOutput,
    /// The assembled system prompt sent to the agent — persisted alongside
    /// the log so contributors can confirm skill bodies and overrides made
    /// it in.
    pub system_prompt: String,
}
