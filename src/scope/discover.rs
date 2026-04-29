//! Discover `blick.toml` files and auto-declared local skills.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::config::{ConfigFile, SkillEntry};
use crate::error::BlickError;

pub(super) const CONFIG_NAME: &str = "blick.toml";
pub(super) const LOCAL_SKILLS_DIR: &str = ".blick/skills";

/// Walk `repo_root` collecting every `blick.toml` (skipping common build
/// directories), keyed by the directory containing the file.
pub(super) fn collect_config_files(
    repo_root: &Path,
) -> Result<BTreeMap<PathBuf, (ConfigFile, PathBuf)>, BlickError> {
    let mut config_paths: Vec<PathBuf> = WalkDir::new(repo_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_skipped_dir(entry.file_name().to_string_lossy().as_ref()))
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
        files.insert(parent, (file, path.clone()));
    }
    Ok(files)
}

/// Directories `walkdir` should never descend into. Keeping this as a
/// dedicated predicate makes it easy to test and to extend.
fn is_skipped_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | "target" | "dist" | "build" | ".venv"
    )
}

/// Find skills auto-declared by the `.blick/skills/<name>/` convention next
/// to a `blick.toml`. Each child directory containing `SKILL.md` (or
/// `README.md`) is exposed as a skill named after the directory, with a
/// synthetic local `source`.
pub(super) fn discover_local_skills(dir: &Path) -> Vec<SkillEntry> {
    let root = dir.join(LOCAL_SKILLS_DIR);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let has_body = ["SKILL.md", "skill.md", "README.md", "readme.md"]
            .iter()
            .any(|name| path.join(name).exists());
        if !has_body {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        found.push(SkillEntry {
            name: name.to_owned(),
            source: format!("./{LOCAL_SKILLS_DIR}/{name}"),
            r#ref: None,
            subpath: None,
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_dirs_match_known_build_artifacts() {
        for dir in [".git", "node_modules", "target", "dist", "build", ".venv"] {
            assert!(is_skipped_dir(dir), "{dir} should be skipped");
        }
        assert!(!is_skipped_dir("apps"));
        assert!(!is_skipped_dir("src"));
    }

    #[test]
    fn discover_returns_empty_for_missing_skills_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(discover_local_skills(tmp.path()).is_empty());
    }

    #[test]
    fn discover_skips_directories_without_a_body_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".blick/skills/empty")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".blick/skills/has_body")).unwrap();
        std::fs::write(
            tmp.path().join(".blick/skills/has_body/SKILL.md"),
            "# body\n",
        )
        .unwrap();
        let found = discover_local_skills(tmp.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "has_body");
    }

    #[test]
    fn discover_results_are_sorted_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["zeta", "alpha", "mu"] {
            let dir = tmp.path().join(".blick/skills").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), "x").unwrap();
        }
        let found = discover_local_skills(tmp.path());
        let names: Vec<_> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mu", "zeta"]);
    }
}
