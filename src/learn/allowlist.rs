//! Files learn is allowed to read into the agent's prompt and write back
//! to disk, plus the path-validation logic that protects the rest of the
//! repository from a malicious or buggy agent response.

use std::fs;
use std::path::Path;

use crate::error::BlickError;

use super::proposal::ProposedEdit;

/// Files learn is allowed to write. Anything outside this list is rejected
/// before the commit stage so the agent can't modify source code.
pub(super) const EDIT_ALLOWLIST: &[&str] =
    &["blick.toml", "AGENTS.md", "CLAUDE.md", ".blick/skills/"];

#[derive(Debug, Clone)]
pub(super) struct AllowlistFile {
    pub(super) path: String,
    pub(super) contents: String,
}

pub(super) fn collect_allowlist_files(repo_root: &Path) -> Result<Vec<AllowlistFile>, BlickError> {
    let mut out = Vec::new();
    for explicit in ["blick.toml", "AGENTS.md", "CLAUDE.md"] {
        let p = repo_root.join(explicit);
        if p.exists()
            && p.is_file()
            && let Ok(body) = fs::read_to_string(&p)
        {
            out.push(AllowlistFile {
                path: explicit.to_owned(),
                contents: body,
            });
        }
    }
    let skills_root = repo_root.join(".blick/skills");
    if skills_root.is_dir() {
        for entry in walkdir::WalkDir::new(&skills_root)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(repo_root) else {
                continue;
            };
            if let Ok(body) = fs::read_to_string(entry.path()) {
                out.push(AllowlistFile {
                    path: rel.to_string_lossy().into_owned(),
                    contents: body,
                });
            }
        }
    }
    Ok(out)
}

pub(super) fn validate_edits(edits: &[ProposedEdit]) -> Result<(), BlickError> {
    for edit in edits {
        validate_edit_path(&edit.path)?;
    }
    Ok(())
}

/// Reject anything other than a relative path containing only `Normal`
/// components, expressed in git-canonical form (forward slashes only).
///
/// Catches absolute paths (`/etc/passwd`), Windows-drive roots (`C:\foo`),
/// parent traversal (`../foo`), bare `.` segments, embedded NUL bytes, and
/// backslash separators — the last one is critical because the allowlist
/// is matched on string prefixes with `/` and `Path::components()` on
/// Windows happily splits `\` segments, so without this rejection a value
/// like `.blick\skills\..\..\secret` could slip past the allowlist on
/// Windows even though it would be rejected here.
pub(super) fn validate_edit_path(raw: &str) -> Result<(), BlickError> {
    if raw.is_empty() {
        return Err(BlickError::Config("edit path is empty".to_owned()));
    }
    if raw.as_bytes().contains(&0) {
        return Err(BlickError::Config(format!(
            "edit path contains a NUL byte: {raw:?}"
        )));
    }
    if raw.contains('\\') {
        return Err(BlickError::Config(format!(
            "edit path must use `/` separators, not `\\`: {raw}"
        )));
    }

    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(BlickError::Config(format!(
            "edit path must be relative: {raw}"
        )));
    }
    for component in path.components() {
        match component {
            std::path::Component::Normal(_) => {}
            std::path::Component::CurDir
            | std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err(BlickError::Config(format!(
                    "edit path has a disallowed component ({component:?}): {raw}"
                )));
            }
        }
    }

    if !path_in_allowlist(raw) {
        return Err(BlickError::Config(format!(
            "agent proposed edit outside the allowlist: {raw}"
        )));
    }

    Ok(())
}

/// Match `path` against the static [`EDIT_ALLOWLIST`].
///
/// Callers MUST run [`validate_edit_path`] first; we rely on that step to
/// reject `\` separators, traversal segments, and absolute paths. Given
/// those preconditions, plain string matching against `/`-terminated
/// prefixes is sufficient — and refusing to do it any other way means the
/// allowlist contract stays trivial to read.
pub(super) fn path_in_allowlist(path: &str) -> bool {
    EDIT_ALLOWLIST.iter().any(|allowed| {
        if allowed.ends_with('/') {
            path.starts_with(allowed)
        } else {
            path == *allowed
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_accepts_skill_subpaths_and_blick_toml() {
        assert!(path_in_allowlist("blick.toml"));
        assert!(path_in_allowlist("AGENTS.md"));
        assert!(path_in_allowlist(".blick/skills/foo/SKILL.md"));
        assert!(!path_in_allowlist("src/lib.rs"));
        assert!(!path_in_allowlist("blick.toml.bak"));
        assert!(!path_in_allowlist("blickXtoml"));
    }

    #[test]
    fn validate_edits_rejects_traversal_and_outside_paths() {
        let bad = vec![ProposedEdit {
            path: "src/main.rs".into(),
            contents: String::new(),
        }];
        assert!(validate_edits(&bad).is_err());
        let traversal = vec![ProposedEdit {
            path: ".blick/skills/../../etc/passwd".into(),
            contents: String::new(),
        }];
        assert!(validate_edits(&traversal).is_err());
        let ok = vec![ProposedEdit {
            path: ".blick/skills/foo/SKILL.md".into(),
            contents: "# foo\n".into(),
        }];
        assert!(validate_edits(&ok).is_ok());
    }

    #[test]
    fn validate_edit_path_rejects_backslash_separators() {
        // Windows-style separators are rejected outright so `..\..` style
        // traversal can't sneak past `path_in_allowlist`'s `/`-anchored
        // string match on platforms where `Path::components()` recognises
        // `\`. This matches blick's git-canonical edit path contract.
        assert!(validate_edit_path(".blick\\skills\\foo\\SKILL.md").is_err());
        assert!(validate_edit_path(".blick/skills\\..\\..\\secret").is_err());
    }

    #[test]
    fn validate_edit_path_rejects_absolute_and_special_components() {
        // Absolute path → rejected even though the suffix would be in the
        // allowlist if it were relative.
        assert!(validate_edit_path("/etc/passwd").is_err());
        // Parent-traversal segments are rejected ahead of the allowlist
        // check so they can't smuggle a path out of `.blick/skills/` past
        // `path_in_allowlist`.
        assert!(validate_edit_path(".blick/skills/foo/../../bar").is_err());
        // Leading `./` produces a `CurDir` component, which we reject so
        // the path stays in a canonical form.
        assert!(validate_edit_path("./blick.toml").is_err());
        // Empty path and NUL bytes are not file paths.
        assert!(validate_edit_path("").is_err());
        assert!(validate_edit_path(".blick/skills/foo/SKILL.md\0evil").is_err());
        // Sanity check that a clean relative path still passes. Mid-path
        // `./` is normalized away by `Path::components()` and is fine.
        assert!(validate_edit_path(".blick/skills/foo/SKILL.md").is_ok());
        assert!(validate_edit_path(".blick/skills/./foo/SKILL.md").is_ok());
        assert!(validate_edit_path("blick.toml").is_ok());
    }

    #[test]
    fn collect_allowlist_files_returns_empty_for_clean_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let files = collect_allowlist_files(tmp.path()).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn collect_allowlist_files_picks_up_blick_toml_and_skills() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("blick.toml"), "x = 1\n").unwrap();
        std::fs::create_dir_all(tmp.path().join(".blick/skills/foo")).unwrap();
        std::fs::write(tmp.path().join(".blick/skills/foo/SKILL.md"), "# foo\n").unwrap();
        let files = collect_allowlist_files(tmp.path()).unwrap();
        let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"blick.toml"));
        assert!(paths.iter().any(|p| p.ends_with("foo/SKILL.md")));
    }
}
