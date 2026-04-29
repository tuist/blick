//! `blick.toml` schema, defaults, and effective (merged) configuration.
//!
//! Submodules:
//! - [`severity`]   — the shared severity enum
//! - [`agent`]      — agent kind + `[agent]` block
//! - [`skill`]      — `[[skills]]` and resolved-with-provenance form
//! - [`review`]     — `[[reviews]]` + `[defaults]` table
//! - [`learn`]      — `[learn]` block
//! - [`effective`]  — types for the merged-across-ancestors view

mod agent;
mod effective;
mod learn;
mod review;
mod severity;
mod skill;

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::BlickError;

pub use agent::{AgentConfig, AgentKind};
pub use effective::{EffectiveDefaults, ProvenanceEntry, ScopeConfig};
pub use learn::LearnConfig;
pub use review::{ReviewDefaults, ReviewEntry};
pub use severity::Severity;
pub use skill::{ResolvedSkillEntry, SkillEntry};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learn: Option<LearnConfig>,
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

    /// A minimal `blick.toml` to drop into a new repo via `blick init`.
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
            learn: None,
        }
    }
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

    #[test]
    fn starter_with_explicit_model_overrides_default() {
        let config = ConfigFile::starter(AgentKind::Claude, Some("custom/model".into()));
        let rendered = config.to_toml().unwrap();
        assert!(rendered.contains("custom/model"));
        assert!(!rendered.contains("anthropic/claude-sonnet-4-5"));
    }

    #[test]
    fn load_surfaces_path_in_parse_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blick.toml");
        std::fs::write(&path, "this is not = valid = toml").unwrap();
        let err = ConfigFile::load(&path).unwrap_err();
        match err {
            BlickError::Config(msg) => {
                assert!(msg.contains(path.to_string_lossy().as_ref()));
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }
}
