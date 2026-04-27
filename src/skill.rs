use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::ResolvedSkillEntry;
use crate::error::BlickError;

/// A resolved skill — concrete on-disk path plus its loaded markdown body.
#[derive(Debug, Clone)]
pub struct LoadedSkill {
    pub name: String,
    pub path: PathBuf,
    pub body: String,
}

pub fn load(skill: &ResolvedSkillEntry) -> Result<LoadedSkill, BlickError> {
    let dir = resolve_source(skill)?;
    let dir = if let Some(sub) = &skill.entry.subpath {
        dir.join(sub)
    } else {
        dir
    };

    let body = read_skill_body(&dir).map_err(|err| {
        BlickError::Config(format!(
            "skill {} at {}: {err}",
            skill.entry.name,
            dir.display()
        ))
    })?;

    Ok(LoadedSkill {
        name: skill.entry.name.clone(),
        path: dir,
        body,
    })
}

fn resolve_source(skill: &ResolvedSkillEntry) -> Result<PathBuf, BlickError> {
    let source = &skill.entry.source;

    if is_local_path(source) {
        let resolved = if Path::new(source).is_absolute() {
            PathBuf::from(source)
        } else {
            skill.declared_in.join(source)
        };
        return Ok(resolved);
    }

    // Treat as `owner/repo` (or `owner/repo/path/within`) GitHub shorthand.
    let parts: Vec<&str> = source.splitn(3, '/').collect();
    if parts.len() < 2 {
        return Err(BlickError::Config(format!(
            "skill source {source} is not a local path or owner/repo"
        )));
    }
    let owner = parts[0];
    let repo = parts[1];
    let inner_subpath = parts.get(2).map(|s| s.to_string());
    let git_ref = skill.entry.r#ref.as_deref().unwrap_or("HEAD");

    let cache_dir = cache_root()?.join("skills").join(owner).join(format!(
        "{}@{}",
        repo,
        git_ref.replace('/', "_")
    ));
    if !cache_dir.exists() {
        clone(owner, repo, git_ref, &cache_dir)?;
    }

    Ok(if let Some(sub) = inner_subpath {
        cache_dir.join(sub)
    } else {
        cache_dir
    })
}

fn is_local_path(source: &str) -> bool {
    source.starts_with("./")
        || source.starts_with("../")
        || source.starts_with('/')
        || source.starts_with("~")
}

fn cache_root() -> Result<PathBuf, BlickError> {
    if let Ok(custom) = std::env::var("BLICK_CACHE_DIR") {
        return Ok(PathBuf::from(custom));
    }
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(xdg).join("blick"));
    }
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home).join(".cache").join("blick"));
    }
    Ok(std::env::temp_dir().join("blick-cache"))
}

fn clone(owner: &str, repo: &str, git_ref: &str, dest: &Path) -> Result<(), BlickError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let url = format!("https://github.com/{owner}/{repo}.git");
    let output = Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            "--branch",
            git_ref,
            &url,
            dest.to_string_lossy().as_ref(),
        ])
        .output()
        .map_err(|err| BlickError::Config(format!("failed to invoke git clone: {err}")))?;

    if output.status.success() {
        return Ok(());
    }

    // Some refs (commit SHAs, default branch under another name) don't work
    // with `--branch`; fall back to a full shallow clone + checkout.
    let _ = fs::remove_dir_all(dest);
    let output = Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            &url,
            dest.to_string_lossy().as_ref(),
        ])
        .output()
        .map_err(|err| BlickError::Config(format!("failed to invoke git clone: {err}")))?;

    if !output.status.success() {
        return Err(BlickError::Config(format!(
            "git clone {url} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn read_skill_body(dir: &Path) -> Result<String, std::io::Error> {
    // Prefer SKILL.md (Anthropic skills format); fall back to README.md.
    for candidate in ["SKILL.md", "skill.md", "README.md", "readme.md"] {
        let path = dir.join(candidate);
        if path.exists() {
            return fs::read_to_string(path);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no SKILL.md or README.md found in skill directory",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SkillEntry;
    use tempfile::TempDir;

    #[test]
    fn loads_local_skill_from_relative_path() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skills/my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "# my-skill\n\nReview body content.\n",
        )
        .unwrap();

        let entry = ResolvedSkillEntry {
            entry: SkillEntry {
                name: "my-skill".to_owned(),
                source: "./skills/my-skill".to_owned(),
                r#ref: None,
                subpath: None,
            },
            declared_in: tmp.path().to_path_buf(),
        };

        let loaded = load(&entry).expect("local skill should load");
        assert_eq!(loaded.name, "my-skill");
        assert!(loaded.body.contains("Review body content"));
    }

    #[test]
    fn returns_error_on_missing_skill_directory() {
        let tmp = TempDir::new().unwrap();
        let entry = ResolvedSkillEntry {
            entry: SkillEntry {
                name: "missing".to_owned(),
                source: "./does-not-exist".to_owned(),
                r#ref: None,
                subpath: None,
            },
            declared_in: tmp.path().to_path_buf(),
        };
        assert!(load(&entry).is_err());
    }
}
