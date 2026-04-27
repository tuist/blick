use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::config::{
    AgentConfig, AgentKind, ConfigFile, EffectiveDefaults, ProvenanceEntry, ResolvedSkillEntry,
    ReviewEntry, ScopeConfig, Severity,
};
use crate::error::BlickError;

const CONFIG_NAME: &str = "blick.toml";

/// Discover every `blick.toml` reachable from `repo_root` (including the
/// root itself if present), producing one resolved [`ScopeConfig`] per file.
///
/// Each scope inherits `[agent]` and `[[skills]]` from its ancestor scopes
/// (closest wins per-field for agent; skills union with closest-wins on
/// name collision). `[[reviews]]` are scope-local.
pub fn load_scopes(repo_root: &Path) -> Result<Vec<ScopeConfig>, BlickError> {
    let mut config_paths: Vec<PathBuf> = WalkDir::new(repo_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            // Don't descend into typical noise. Keep it simple — scope is
            // declared by the presence of a blick.toml, so we just need to
            // avoid wasted traversal.
            let name = entry.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                ".git" | "node_modules" | "target" | "dist" | "build" | ".venv"
            )
        })
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == CONFIG_NAME)
        .map(|entry| entry.into_path())
        .collect();

    config_paths.sort();

    let mut files: BTreeMap<PathBuf, (ConfigFile, PathBuf)> = BTreeMap::new();
    for path in &config_paths {
        let parent = path
            .parent()
            .ok_or_else(|| BlickError::Config(format!("{} has no parent", path.display())))?
            .to_path_buf();
        let file = ConfigFile::load(path)?;
        files.insert(parent.clone(), (file, path.clone()));
    }

    let mut scopes = Vec::with_capacity(files.len());
    for (root, (file, source_path)) in &files {
        let chain = ancestor_chain(repo_root, root, &files);
        let scope = build_scope(root.clone(), source_path, file, &chain)?;
        scopes.push(scope);
    }

    Ok(scopes)
}

/// Build the ancestor chain of (path, file) tuples for a scope, ordered from
/// the repo root inward (root first, scope itself last).
fn ancestor_chain<'a>(
    repo_root: &'a Path,
    scope_root: &'a Path,
    files: &'a BTreeMap<PathBuf, (ConfigFile, PathBuf)>,
) -> Vec<(&'a Path, &'a ConfigFile, &'a Path)> {
    let mut chain: Vec<(&'a Path, &'a ConfigFile, &'a Path)> = Vec::new();
    let mut current: Option<&Path> = Some(scope_root);

    while let Some(dir) = current {
        if let Some((file, source)) = files.get(dir) {
            // SAFETY-ish: keys in `files` outlive 'a; promote the borrow.
            let key = files.get_key_value(dir).map(|(k, _)| k.as_path()).unwrap();
            chain.push((key, file, source.as_path()));
        }
        if dir == repo_root {
            break;
        }
        current = dir.parent();
    }

    chain.reverse();
    chain
}

fn build_scope(
    root: PathBuf,
    source_path: &Path,
    _own: &ConfigFile,
    chain: &[(&Path, &ConfigFile, &Path)],
) -> Result<ScopeConfig, BlickError> {
    let mut agent: Option<AgentConfig> = None;
    let mut agent_source: Option<PathBuf> = None;
    let mut skills: BTreeMap<String, ResolvedSkillEntry> = BTreeMap::new();
    let mut defaults = EffectiveDefaults::default();
    let mut provenance: Vec<ProvenanceEntry> = Vec::new();

    for (dir, file, source) in chain {
        if let Some(file_agent) = &file.agent {
            // Closest wins: replace whole agent block (kind + model are tightly
            // coupled — partial inheritance would be confusing).
            agent = Some(file_agent.clone());
            agent_source = Some(source.to_path_buf());
        }
        for skill in &file.skills {
            skills.insert(
                skill.name.clone(),
                ResolvedSkillEntry {
                    entry: skill.clone(),
                    declared_in: dir.to_path_buf(),
                },
            );
        }
        if let Some(base) = &file.defaults.base {
            defaults.base = base.clone();
            provenance.push(ProvenanceEntry {
                field: "defaults.base".to_owned(),
                source: source.to_path_buf(),
            });
        }
        if let Some(max) = file.defaults.max_diff_bytes {
            defaults.max_diff_bytes = max;
            provenance.push(ProvenanceEntry {
                field: "defaults.max_diff_bytes".to_owned(),
                source: source.to_path_buf(),
            });
        }
        if let Some(fail_on) = file.defaults.fail_on {
            defaults.fail_on = fail_on;
            provenance.push(ProvenanceEntry {
                field: "defaults.fail_on".to_owned(),
                source: source.to_path_buf(),
            });
        }
        if let Some(cap) = file.defaults.max_concurrency {
            defaults.max_concurrency = cap.max(1);
            provenance.push(ProvenanceEntry {
                field: "defaults.max_concurrency".to_owned(),
                source: source.to_path_buf(),
            });
        }
    }

    let agent = agent.unwrap_or_else(|| AgentConfig {
        kind: AgentKind::Claude,
        model: AgentKind::Claude.default_model().map(ToOwned::to_owned),
        binary: None,
        args: Vec::new(),
    });

    if let Some(src) = agent_source {
        provenance.push(ProvenanceEntry {
            field: "agent".to_owned(),
            source: src,
        });
    }

    // Reviews are scope-local — only the scope's own file contributes.
    let own_file = &chain
        .last()
        .ok_or_else(|| BlickError::Config("empty scope chain".to_owned()))?
        .1;
    let reviews: Vec<ReviewEntry> = own_file.reviews.clone();

    provenance.push(ProvenanceEntry {
        field: "reviews".to_owned(),
        source: source_path.to_path_buf(),
    });

    Ok(ScopeConfig {
        root,
        agent,
        skills,
        reviews,
        defaults,
        provenance,
    })
}

/// Map each path (relative to repo_root) to its owning scope. The owning
/// scope is the nearest scope walking up from the path.
pub fn owner_for<'a>(scopes: &'a [ScopeConfig], path: &Path) -> Option<&'a ScopeConfig> {
    let mut best: Option<&ScopeConfig> = None;
    for scope in scopes {
        if path.starts_with(&scope.root) {
            match best {
                Some(current)
                    if current.root.components().count() >= scope.root.components().count() => {}
                _ => best = Some(scope),
            }
        }
    }
    best
}

/// Default fail-on threshold for a review, falling back through review →
/// scope defaults → high.
pub fn effective_fail_on(scope: &ScopeConfig, review: &ReviewEntry) -> Severity {
    review.fail_on.unwrap_or(scope.defaults.fail_on)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn root_only_scope_resolves_with_defaults() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "blick.toml",
            r#"
[agent]
kind = "claude"

[[reviews]]
name = "default"
"#,
        );

        let scopes = load_scopes(tmp.path()).unwrap();
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].agent.kind, AgentKind::Claude);
        assert_eq!(scopes[0].reviews[0].name, "default");
    }

    #[test]
    fn child_scope_inherits_agent_and_skills_but_not_reviews() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "blick.toml",
            r#"
[agent]
kind = "claude"
model = "anthropic/claude-sonnet-4-5"

[[skills]]
name = "owasp"
source = "tuist/blick-skills"

[[reviews]]
name = "root-review"
"#,
        );
        write(
            tmp.path(),
            "apps/web/blick.toml",
            r#"
[[skills]]
name = "react"
source = "vercel-labs/agent-skills"

[[reviews]]
name = "web-review"
skills = ["owasp", "react"]
"#,
        );

        let scopes = load_scopes(tmp.path()).unwrap();
        let web = scopes
            .iter()
            .find(|s| s.root.ends_with("apps/web"))
            .expect("web scope");
        assert_eq!(web.agent.kind, AgentKind::Claude);
        assert!(web.skills.contains_key("owasp"));
        assert!(web.skills.contains_key("react"));
        assert_eq!(web.reviews.len(), 1);
        assert_eq!(web.reviews[0].name, "web-review");
    }

    #[test]
    fn closest_skill_wins_on_name_collision() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "blick.toml",
            r#"
[[skills]]
name = "lint"
source = "owner/root-lint"

[[reviews]]
name = "x"
"#,
        );
        write(
            tmp.path(),
            "child/blick.toml",
            r#"
[[skills]]
name = "lint"
source = "owner/child-lint"

[[reviews]]
name = "y"
"#,
        );

        let scopes = load_scopes(tmp.path()).unwrap();
        let child = scopes
            .iter()
            .find(|s| s.root.ends_with("child"))
            .expect("child scope");
        assert_eq!(child.skills["lint"].entry.source, "owner/child-lint");
    }
}
