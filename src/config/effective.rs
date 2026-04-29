//! Effective (post-merge) configuration types — what a single scope sees
//! after walking up its ancestor `blick.toml` chain.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::agent::AgentConfig;
use super::review::ReviewEntry;
use super::severity::Severity;
use super::skill::ResolvedSkillEntry;

/// Effective configuration for a single scope (a single `blick.toml`),
/// computed by merging the scope's file with its ancestor scopes.
#[derive(Debug, Clone)]
pub struct ScopeConfig {
    /// Directory containing the scope's `blick.toml`.
    pub root: PathBuf,
    pub agent: AgentConfig,
    /// Skills available to this scope, keyed by name. Closest-wins on conflict.
    pub skills: BTreeMap<String, ResolvedSkillEntry>,
    /// Reviews defined in *this* scope (no inheritance).
    pub reviews: Vec<ReviewEntry>,
    pub defaults: EffectiveDefaults,
    /// Per-source provenance (which file each piece of config came from)
    /// for `--explain`.
    pub provenance: Vec<ProvenanceEntry>,
}

/// Resolved values for the `[defaults]` table — every field has a concrete
/// value (no `Option`s) because we've already merged ancestors and applied
/// fallback defaults.
#[derive(Debug, Clone)]
pub struct EffectiveDefaults {
    pub base: String,
    pub max_diff_bytes: usize,
    pub fail_on: Severity,
    pub max_concurrency: usize,
}

impl Default for EffectiveDefaults {
    fn default() -> Self {
        Self {
            base: "HEAD".to_owned(),
            max_diff_bytes: 120_000,
            fail_on: Severity::High,
            max_concurrency: 4,
        }
    }
}

/// Tracks where each piece of effective config came from. Used by
/// `blick config --explain`.
#[derive(Debug, Clone)]
pub struct ProvenanceEntry {
    pub field: String,
    pub source: PathBuf,
}
