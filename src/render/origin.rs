//! Display label for a `(scope, review)` pair.

/// Human label for a `(scope, review)` pair. Drops the `./` prefix when
/// the scope is the repo root so output reads "default" instead of
/// "./default".
pub fn origin_label(scope_label: &str, review_name: &str) -> String {
    if scope_label == "." {
        review_name.to_owned()
    } else {
        format!("{scope_label}/{review_name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_root_dot_for_repo_root_scope() {
        assert_eq!(origin_label(".", "default"), "default");
    }

    #[test]
    fn joins_nested_scope_with_review() {
        assert_eq!(origin_label("apps/web", "security"), "apps/web/security");
    }
}
