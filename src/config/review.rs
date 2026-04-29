//! `[[reviews]]` entries and their `[defaults]` sibling.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::severity::Severity;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_is_empty_when_all_none() {
        assert!(ReviewDefaults::default().is_empty());
    }

    #[test]
    fn defaults_is_not_empty_when_any_field_set() {
        assert!(
            !ReviewDefaults {
                base: Some("main".into()),
                ..Default::default()
            }
            .is_empty()
        );
        assert!(
            !ReviewDefaults {
                max_diff_bytes: Some(1),
                ..Default::default()
            }
            .is_empty()
        );
        assert!(
            !ReviewDefaults {
                fail_on: Some(Severity::High),
                ..Default::default()
            }
            .is_empty()
        );
        assert!(
            !ReviewDefaults {
                max_concurrency: Some(1),
                ..Default::default()
            }
            .is_empty()
        );
    }
}
