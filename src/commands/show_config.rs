//! `blick config` — print the effective configuration per scope, optionally
//! with provenance (`--explain`).

use std::path::{Path, PathBuf};

use crate::cli::ConfigArgs;
use crate::error::BlickError;
use crate::scope::load_scopes;

pub fn run(args: ConfigArgs) -> Result<(), BlickError> {
    let repo_root = args
        .repo
        .clone()
        .unwrap_or_else(|| PathBuf::from("."))
        .canonicalize()?;
    let scopes = load_scopes(&repo_root)?;

    if scopes.is_empty() {
        println!("No blick.toml found under {}", repo_root.display());
        return Ok(());
    }

    for scope in &scopes {
        println!("# scope: {}", display_path(&scope.root, &repo_root));
        println!("  agent.kind  = {}", scope.agent.kind.as_str());
        if let Some(model) = &scope.agent.model {
            println!("  agent.model = {}", model);
        }
        println!("  defaults.base           = {}", scope.defaults.base);
        println!(
            "  defaults.max_diff_bytes = {}",
            scope.defaults.max_diff_bytes
        );
        println!(
            "  defaults.fail_on        = {}",
            scope.defaults.fail_on.as_str()
        );

        if !scope.skills.is_empty() {
            println!("  skills:");
            for (name, resolved) in &scope.skills {
                println!(
                    "    - {name} ({}) [declared in {}]",
                    resolved.entry.source,
                    display_path(&resolved.declared_in, &repo_root)
                );
            }
        }

        if !scope.reviews.is_empty() {
            println!("  reviews:");
            for review_entry in &scope.reviews {
                println!(
                    "    - {} (skills: {})",
                    review_entry.name,
                    if review_entry.skills.is_empty() {
                        "<none>".to_owned()
                    } else {
                        review_entry.skills.join(", ")
                    }
                );
            }
        }

        if scope.reviews.is_empty() {
            println!("  ⚠ this scope defines no reviews; files here will be skipped.");
        }

        if args.explain {
            println!("  provenance:");
            for entry in &scope.provenance {
                println!(
                    "    {:<24} from {}",
                    entry.field,
                    display_path(&entry.source, &repo_root)
                );
            }
        }
        println!();
    }

    Ok(())
}

/// Render `path` as relative-to-`repo_root`, falling back to absolute when
/// the prefix doesn't match. The repo root itself becomes `.`.
pub(super) fn display_path(path: &Path, repo_root: &Path) -> String {
    path.strip_prefix(repo_root)
        .map(|p| {
            if p.as_os_str().is_empty() {
                ".".to_owned()
            } else {
                p.display().to_string()
            }
        })
        .unwrap_or_else(|_| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_path_uses_dot_for_repo_root() {
        let root = Path::new("/repo");
        assert_eq!(display_path(root, root), ".");
    }

    #[test]
    fn display_path_strips_repo_prefix() {
        assert_eq!(
            display_path(Path::new("/repo/apps/web"), Path::new("/repo")),
            "apps/web"
        );
    }

    #[test]
    fn display_path_falls_back_to_absolute_when_outside_repo() {
        assert_eq!(
            display_path(Path::new("/other/place"), Path::new("/repo")),
            "/other/place"
        );
    }
}
