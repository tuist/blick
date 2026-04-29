//! Skill declarations from `blick.toml` and their resolved (provenance-aware)
//! form.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A single `[[skills]]` entry in `blick.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntry {
    pub name: String,
    /// Either a local path (relative to the defining `blick.toml`) or a
    /// `owner/repo` shorthand resolved over GitHub.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
}

/// A skill resolved against the scope it was declared in. We keep the
/// declaring directory so relative `source` paths can be resolved later.
#[derive(Debug, Clone)]
pub struct ResolvedSkillEntry {
    pub entry: SkillEntry,
    /// Directory of the `blick.toml` that declared this skill — used to
    /// resolve relative `source` paths.
    pub declared_in: PathBuf,
}
