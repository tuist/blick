# blick

`blick` reviews the diff in your repository with the hosted model or local coding agent you already use.

## ✨ What You Can Do

- review local changes against `HEAD` or any git base
- choose a hosted provider like OpenAI or Anthropic
- use a local `claude` or `codex` binary when it is available
- customize the review workflow with deterministic prompt steps plus LLM review steps

## 🚀 Install

For development in this repository:

```sh
mise install
```

After a release is published, install the CLI from GitHub releases with Mise:

```sh
mise use -g github:tuist/blick
```

## 🛠️ Quick Start

Create a starter config:

```sh
blick init
```

Review the current diff:

```sh
blick review
```

Review against a different base:

```sh
blick review --base origin/main
```

Emit JSON for scripts or CI:

```sh
blick review --json
```

## 🤖 Provider Setup

Hosted providers:

```toml
[llm]
provider = "openai"
model = "gpt-5"
```

```toml
[llm]
provider = "anthropic"
model = "claude-sonnet-4-5"
```

Local agent CLIs:

```toml
[llm]
provider = "claude"
```

```toml
[llm]
provider = "codex"
```

Auto-discover a local CLI if one is present:

```toml
[llm]
provider = "auto"
```

You can also override the API key variable or base URL:

```toml
[llm]
provider = "anthropic"
model = "claude-sonnet-4-5"
api_key_env = "MY_ANTHROPIC_TOKEN"
base_url = "https://api.anthropic.com/v1"
```

## 🧠 Workflow DSL

`blick` ships with a default review workflow, but you can replace it in `blick.toml` when you want tighter instructions or a different review style.

```toml
[review]
base = "HEAD"
max_diff_bytes = 120000

[[review.workflow.steps]]
type = "prompt"
role = "system"
content = """
You are Blick.
Focus on correctness, regressions, and missing tests.
Return JSON only.
"""

[[review.workflow.steps]]
type = "prompt"
role = "user"
content = """
Base revision: {{base}}
{{truncated_note}}

Changed files:
{{files}}

Unified diff:
{{diff}}
"""

[[review.workflow.steps]]
type = "llm_review"
```

The current placeholders are `{{base}}`, `{{truncated_note}}`, `{{files}}`, and `{{diff}}`.

## 🧪 Development

Format the workspace:

```sh
mise run fmt
```

Run the test suites:

```sh
mise run test
```

That runs:

- temp-repo git integration coverage
- workspace build/test coverage
- ShellSpec end-to-end workflow checks with a fake local CLI

## 📦 Releases

Releases are driven by conventional commits and `git-cliff`.

- `mise run release:detect` checks whether `main` contains releasable changes
- `mise run release:changelog` regenerates `CHANGELOG.md`
- `.github/workflows/release.yml` packages release archives and publishes GitHub releases that Mise can install through the `github:` backend

## 🫶 Credits

- [getsentry/warden](https://github.com/getsentry/warden) shaped the local-review workflow and developer experience
- [:req_llm](https://hex.pm/packages/req_llm) pushed the design toward configurable providers instead of provider-specific code paths
