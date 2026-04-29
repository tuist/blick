//! Apply CLI / env-var overrides for the agent and model after scopes have
//! been loaded but before tasks are dispatched.

use std::env;

use crate::config::{AgentConfig, AgentKind, ScopeConfig};

/// Override every scope's agent (kind / model) when the user passed `--agent`
/// / `--model` or set `BLICK_AGENT_KIND` / `BLICK_AGENT_MODEL`. Overrides
/// preserve the scope's existing `binary` and `args`.
pub(super) fn apply_agent_overrides(
    scopes: &mut [ScopeConfig],
    agent: Option<AgentKind>,
    model: Option<&str>,
) {
    let env_agent = env::var("BLICK_AGENT_KIND").ok().and_then(parse_agent_kind);
    let env_model = env::var("BLICK_AGENT_MODEL").ok();

    let final_agent = agent.or(env_agent);
    let final_model: Option<String> = model.map(ToOwned::to_owned).or(env_model);

    if final_agent.is_none() && final_model.is_none() {
        return;
    }

    for scope in scopes.iter_mut() {
        apply_to_one(scope, final_agent, final_model.as_deref());
    }
}

fn parse_agent_kind(raw: String) -> Option<AgentKind> {
    match raw.as_str() {
        "claude" => Some(AgentKind::Claude),
        "codex" => Some(AgentKind::Codex),
        "opencode" => Some(AgentKind::Opencode),
        _ => None,
    }
}

fn apply_to_one(scope: &mut ScopeConfig, agent: Option<AgentKind>, model: Option<&str>) {
    if let Some(kind) = agent {
        scope.agent = AgentConfig {
            kind,
            model: model
                .map(ToOwned::to_owned)
                .or_else(|| kind.default_model().map(ToOwned::to_owned)),
            binary: scope.agent.binary.clone(),
            args: scope.agent.args.clone(),
        };
    } else if let Some(model_value) = model {
        scope.agent.model = Some(model_value.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EffectiveDefaults;
    use std::path::PathBuf;

    fn scope() -> ScopeConfig {
        ScopeConfig {
            root: PathBuf::from("/repo"),
            agent: AgentConfig {
                kind: AgentKind::Claude,
                model: Some("anthropic/claude-sonnet-4-5".into()),
                binary: Some("/custom/claude".into()),
                args: vec!["--flag".into()],
            },
            skills: Default::default(),
            reviews: Vec::new(),
            defaults: EffectiveDefaults::default(),
            provenance: Vec::new(),
        }
    }

    #[test]
    fn parse_agent_kind_accepts_known_names() {
        assert!(matches!(
            parse_agent_kind("claude".into()),
            Some(AgentKind::Claude)
        ));
        assert!(matches!(
            parse_agent_kind("codex".into()),
            Some(AgentKind::Codex)
        ));
        assert!(matches!(
            parse_agent_kind("opencode".into()),
            Some(AgentKind::Opencode)
        ));
    }

    #[test]
    fn parse_agent_kind_rejects_unknown_names() {
        assert!(parse_agent_kind("gpt".into()).is_none());
        assert!(parse_agent_kind("".into()).is_none());
    }

    #[test]
    fn apply_to_one_replaces_kind_and_picks_default_model() {
        let mut s = scope();
        apply_to_one(&mut s, Some(AgentKind::Codex), None);
        assert_eq!(s.agent.kind, AgentKind::Codex);
        assert_eq!(
            s.agent.model.as_deref(),
            AgentKind::Codex.default_model()
        );
        // `binary` and `args` are preserved when only the kind changes.
        assert_eq!(s.agent.binary.as_deref(), Some("/custom/claude"));
        assert_eq!(s.agent.args, vec!["--flag".to_owned()]);
    }

    #[test]
    fn apply_to_one_uses_explicit_model_when_provided() {
        let mut s = scope();
        apply_to_one(&mut s, Some(AgentKind::Codex), Some("openai/o4-mini"));
        assert_eq!(s.agent.model.as_deref(), Some("openai/o4-mini"));
    }

    #[test]
    fn apply_to_one_with_only_model_keeps_existing_kind() {
        let mut s = scope();
        apply_to_one(&mut s, None, Some("anthropic/claude-opus-4-5"));
        assert_eq!(s.agent.kind, AgentKind::Claude);
        assert_eq!(s.agent.model.as_deref(), Some("anthropic/claude-opus-4-5"));
    }
}
