//! Compute the *effective* configuration for a single scope by walking the
//! ancestor chain of `blick.toml` files. Closest-wins for `[agent]`; skills
//! union with closest-wins on name collision; `[[reviews]]` are scope-local.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::{
    AgentConfig, AgentKind, ConfigFile, EffectiveDefaults, ProvenanceEntry, ResolvedSkillEntry,
    ReviewEntry, ScopeConfig,
};
use crate::error::BlickError;

use super::discover::discover_local_skills;

/// Build the ancestor chain of `(path, file, source)` tuples for a scope,
/// ordered from the repo root inward (root first, scope itself last).
pub(super) fn ancestor_chain<'a>(
    repo_root: &'a Path,
    scope_root: &'a Path,
    files: &'a BTreeMap<PathBuf, (ConfigFile, PathBuf)>,
) -> Vec<(&'a Path, &'a ConfigFile, &'a Path)> {
    let mut chain: Vec<(&'a Path, &'a ConfigFile, &'a Path)> = Vec::new();
    let mut current: Option<&Path> = Some(scope_root);

    while let Some(dir) = current {
        if let Some((file, source)) = files.get(dir) {
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

/// Merge the scope's own config with everything inherited from ancestors.
pub(super) fn build_scope(
    root: PathBuf,
    source_path: &Path,
    chain: &[(&Path, &ConfigFile, &Path)],
) -> Result<ScopeConfig, BlickError> {
    let mut agent: Option<AgentConfig> = None;
    let mut agent_source: Option<PathBuf> = None;
    let mut skills: BTreeMap<String, ResolvedSkillEntry> = BTreeMap::new();
    let mut defaults = EffectiveDefaults::default();
    let mut provenance: Vec<ProvenanceEntry> = Vec::new();

    for (dir, file, source) in chain {
        merge_agent(file, source, &mut agent, &mut agent_source);
        merge_skills(dir, file, &mut skills);
        merge_defaults(file, source, &mut defaults, &mut provenance);
    }

    let agent = agent.unwrap_or_else(default_agent);
    if let Some(src) = agent_source {
        provenance.push(ProvenanceEntry {
            field: "agent".to_owned(),
            source: src,
        });
    }

    // Reviews are scope-local — only the scope's own file contributes.
    let own_file = chain
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

fn merge_agent(
    file: &ConfigFile,
    source: &Path,
    agent: &mut Option<AgentConfig>,
    agent_source: &mut Option<PathBuf>,
) {
    // Closest wins: replace the whole agent block. Kind + model are tightly
    // coupled — partial inheritance would be confusing.
    if let Some(file_agent) = &file.agent {
        *agent = Some(file_agent.clone());
        *agent_source = Some(source.to_path_buf());
    }
}

fn merge_skills(
    dir: &Path,
    file: &ConfigFile,
    skills: &mut BTreeMap<String, ResolvedSkillEntry>,
) {
    // Auto-discovered local skills come first so an explicit [[skills]]
    // entry of the same name in the same scope overrides them.
    for entry in discover_local_skills(dir) {
        skills.insert(
            entry.name.clone(),
            ResolvedSkillEntry {
                entry,
                declared_in: dir.to_path_buf(),
            },
        );
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
}

fn merge_defaults(
    file: &ConfigFile,
    source: &Path,
    defaults: &mut EffectiveDefaults,
    provenance: &mut Vec<ProvenanceEntry>,
) {
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

fn default_agent() -> AgentConfig {
    AgentConfig {
        kind: AgentKind::Claude,
        model: AgentKind::Claude.default_model().map(ToOwned::to_owned),
        binary: None,
        args: Vec::new(),
    }
}
