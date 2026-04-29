//! Scope discovery and inheritance.
//!
//! A "scope" is a directory containing a `blick.toml`. We walk the repo
//! tree to find every scope, then for each one compute its effective
//! configuration by merging the scope's own file with everything inherited
//! from ancestor scopes.
//!
//! Submodules:
//! - [`discover`] — find `blick.toml` files; auto-detect `.blick/skills/*`
//! - [`inherit`]  — merge ancestor chain into a single [`ScopeConfig`]
//! - [`owner`]    — map a path to the scope that owns it

mod discover;
mod inherit;
mod owner;

use std::path::Path;

use crate::config::{ReviewEntry, ScopeConfig, Severity};
use crate::error::BlickError;

pub use owner::owner_for;

/// Discover every `blick.toml` reachable from `repo_root` (including the
/// root itself if present), producing one resolved [`ScopeConfig`] per file.
///
/// Each scope inherits `[agent]` and `[[skills]]` from its ancestor scopes
/// (closest wins per-field for agent; skills union with closest-wins on
/// name collision). `[[reviews]]` are scope-local.
pub fn load_scopes(repo_root: &Path) -> Result<Vec<ScopeConfig>, BlickError> {
    let files = discover::collect_config_files(repo_root)?;

    let mut scopes = Vec::with_capacity(files.len());
    for (root, (_file, source_path)) in &files {
        let chain = inherit::ancestor_chain(repo_root, root, &files);
        let scope = inherit::build_scope(root.clone(), source_path, &chain)?;
        scopes.push(scope);
    }

    Ok(scopes)
}

/// Default fail-on threshold for a review, falling back through review →
/// scope defaults → high.
pub fn effective_fail_on(scope: &ScopeConfig, review: &ReviewEntry) -> Severity {
    review.fail_on.unwrap_or(scope.defaults.fail_on)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentKind;
    use std::fs;
    use std::path::Path;
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
    fn auto_discovers_local_blick_skills_directory() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "blick.toml",
            r#"
[[reviews]]
name = "default"
skills = ["foo"]
"#,
        );
        write(tmp.path(), ".blick/skills/foo/SKILL.md", "# foo\n");
        // Bare directory without SKILL.md/README.md should be ignored.
        std::fs::create_dir_all(tmp.path().join(".blick/skills/empty")).unwrap();

        let scopes = load_scopes(tmp.path()).unwrap();
        assert_eq!(scopes.len(), 1);
        let foo = scopes[0]
            .skills
            .get("foo")
            .expect(".blick/skills/foo should be auto-discovered");
        assert_eq!(foo.entry.source, "./.blick/skills/foo");
        assert!(!scopes[0].skills.contains_key("empty"));
    }

    #[test]
    fn explicit_skill_overrides_auto_discovered_one() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "blick.toml",
            r#"
[[skills]]
name = "foo"
source = "./custom/foo"

[[reviews]]
name = "default"
"#,
        );
        write(tmp.path(), ".blick/skills/foo/SKILL.md", "# auto\n");

        let scopes = load_scopes(tmp.path()).unwrap();
        assert_eq!(scopes[0].skills["foo"].entry.source, "./custom/foo");
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

    #[test]
    fn effective_fail_on_falls_back_to_scope_default() {
        use crate::config::ReviewEntry;

        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "blick.toml",
            r#"
[defaults]
fail_on = "medium"

[[reviews]]
name = "x"
"#,
        );
        let scopes = load_scopes(tmp.path()).unwrap();
        let scope = &scopes[0];
        let review = &scope.reviews[0];
        assert_eq!(effective_fail_on(scope, review), Severity::Medium);

        // Explicit review-level setting wins.
        let with_explicit = ReviewEntry {
            fail_on: Some(Severity::High),
            ..review.clone()
        };
        assert_eq!(effective_fail_on(scope, &with_explicit), Severity::High);
    }
}
