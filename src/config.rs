use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::error::BlickError;

/// On-disk shape of a single `blick.toml` file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentConfig>,
    #[serde(default, rename = "skills", skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillEntry>,
    #[serde(default, rename = "reviews", skip_serializing_if = "Vec::is_empty")]
    pub reviews: Vec<ReviewEntry>,
    #[serde(default, skip_serializing_if = "ReviewDefaults::is_empty")]
    pub defaults: ReviewDefaults,
}

impl ConfigFile {
    pub fn load(path: &Path) -> Result<Self, BlickError> {
        let raw = fs::read_to_string(path)?;
        toml::from_str(&raw).map_err(|error| {
            BlickError::Config(format!("failed to parse {}: {error}", path.display()))
        })
    }

    pub fn to_toml(&self) -> Result<String, BlickError> {
        toml::to_string_pretty(self)
            .map_err(|error| BlickError::Config(format!("failed to serialize config: {error}")))
    }

    pub fn starter(kind: AgentKind, model: Option<String>) -> Self {
        Self {
            agent: Some(AgentConfig {
                kind,
                model: model.or_else(|| kind.default_model().map(ToOwned::to_owned)),
                binary: None,
                args: Vec::new(),
            }),
            skills: Vec::new(),
            reviews: vec![ReviewEntry {
                name: "default".to_owned(),
                skills: Vec::new(),
                fail_on: None,
                prompt: None,
                prompt_file: None,
            }],
            defaults: ReviewDefaults::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[value(name = "claude")]
    Claude,
    #[value(name = "codex")]
    Codex,
    #[value(name = "opencode")]
    Opencode,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
        }
    }

    pub fn default_model(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("anthropic/claude-sonnet-4-5"),
            Self::Codex => Some("openai/gpt-5"),
            Self::Opencode => Some("anthropic/claude-sonnet-4-5"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub kind: AgentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_on: Option<Severity>,
    /// Inline prompt addendum (appended after skill content).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Path (relative to the defining `blick.toml`) to a markdown file
    /// containing prompt content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReviewDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_diff_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_on: Option<Severity>,
    /// Maximum number of `(scope, review)` tasks to run concurrently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<usize>,
}

impl ReviewDefaults {
    pub fn is_empty(&self) -> bool {
        self.base.is_none()
            && self.max_diff_bytes.is_none()
            && self.fail_on.is_none()
            && self.max_concurrency.is_none()
    }
}

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

#[derive(Debug, Clone)]
pub struct ResolvedSkillEntry {
    pub entry: SkillEntry,
    /// Directory of the `blick.toml` that declared this skill — used to
    /// resolve relative `source` paths.
    pub declared_in: PathBuf,
}

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

#[derive(Debug, Clone)]
pub struct ProvenanceEntry {
    pub field: String,
    pub source: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_starter_config() {
        let config = ConfigFile::starter(AgentKind::Claude, None);
        let rendered = config.to_toml().expect("config should serialize");
        assert!(rendered.contains("kind = \"claude\""));
        assert!(rendered.contains("anthropic/claude-sonnet-4-5"));
    }

    #[test]
    fn parses_agent_skills_reviews() {
        let raw = r#"
[agent]
kind = "codex"
model = "openai/gpt-5"

[[skills]]
name = "owasp"
source = "tuist/blick-skills"

[[reviews]]
name = "security"
skills = ["owasp"]
fail_on = "high"
"#;
        let parsed: ConfigFile = toml::from_str(raw).expect("should parse");
        assert_eq!(parsed.agent.as_ref().unwrap().kind, AgentKind::Codex);
        assert_eq!(parsed.skills.len(), 1);
        assert_eq!(parsed.reviews[0].name, "security");
        assert_eq!(parsed.reviews[0].fail_on, Some(Severity::High));
    }
}
