//! Map a path to the scope that owns it (the nearest scope walking up).

use std::path::Path;

use crate::config::ScopeConfig;

/// Find the scope whose root is the deepest ancestor of `path`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentConfig, AgentKind, EffectiveDefaults};
    use std::path::PathBuf;

    fn make_scope(root: &str) -> ScopeConfig {
        ScopeConfig {
            root: PathBuf::from(root),
            agent: AgentConfig {
                kind: AgentKind::Claude,
                model: None,
                binary: None,
                args: Vec::new(),
            },
            skills: Default::default(),
            reviews: Vec::new(),
            defaults: EffectiveDefaults::default(),
            provenance: Vec::new(),
        }
    }

    #[test]
    fn picks_the_deepest_ancestor() {
        let scopes = vec![make_scope("/repo"), make_scope("/repo/apps/web")];
        let owner = owner_for(&scopes, Path::new("/repo/apps/web/src/main.rs")).unwrap();
        assert_eq!(owner.root, PathBuf::from("/repo/apps/web"));
    }

    #[test]
    fn falls_back_to_root_when_no_nested_scope_matches() {
        let scopes = vec![make_scope("/repo"), make_scope("/repo/apps/web")];
        let owner = owner_for(&scopes, Path::new("/repo/docs/intro.md")).unwrap();
        assert_eq!(owner.root, PathBuf::from("/repo"));
    }

    #[test]
    fn returns_none_when_no_scope_owns_the_path() {
        let scopes = vec![make_scope("/repo")];
        assert!(owner_for(&scopes, Path::new("/elsewhere/x")).is_none());
    }

    #[test]
    fn order_of_scopes_does_not_matter() {
        let scopes_a = vec![make_scope("/repo"), make_scope("/repo/a")];
        let scopes_b = vec![make_scope("/repo/a"), make_scope("/repo")];
        let p = Path::new("/repo/a/x");
        assert_eq!(
            owner_for(&scopes_a, p).unwrap().root,
            owner_for(&scopes_b, p).unwrap().root
        );
    }
}
