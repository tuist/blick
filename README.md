# blick

`blick` is a Rust code review agent inspired by [getsentry/warden](https://github.com/getsentry/warden), but designed from the start around two things:

- a native Rust implementation
- configurable LLM provider and model selection

The first version focuses on local review of a Git diff. It loads a `blick.toml`, collects the diff against `HEAD` by default, sends a review prompt to the configured provider, and prints structured findings.

## Why this shape?

Warden is a great reference for developer experience: run locally, catch issues early, and keep the workflow repo-native. This project takes that same direction, but keeps the model layer open so you can choose OpenAI or Anthropic today and expand from there later.

## Tooling

- Rust `1.95.0`
- Bazel `9.0.2` via Bazelisk
- `mise` for local tool management

Install the toolchain:

```sh
mise install
```

Build and test:

```sh
cargo test
bazelisk test //...
```

## Configuration

Create a `blick.toml`:

```toml
[llm]
provider = "openai"
model = "gpt-5"

[review]
base = "HEAD"
max_diff_bytes = 120000
```

Provider defaults:

- `openai` reads `OPENAI_API_KEY`
- `anthropic` reads `ANTHROPIC_API_KEY`

You can also override the environment variable name or base URL:

```toml
[llm]
provider = "anthropic"
model = "claude-sonnet-4-5"
api_key_env = "MY_ANTHROPIC_TOKEN"
base_url = "https://api.anthropic.com/v1"
```

## Usage

Initialize a config file:

```sh
cargo run -- init
```

Review local changes:

```sh
cargo run -- review
```

Review against another base:

```sh
cargo run -- review --base origin/main
```

Emit JSON instead of terminal-friendly output:

```sh
cargo run -- review --json
```

## Current scope

This is an intentionally small MVP:

- local diff review
- configurable provider/model
- OpenAI Responses API support
- Anthropic Messages API support
- Bazel + Cargo working from the same source tree

Good next steps would be GitHub PR review, reusable skills or rules, autofix flows, and richer output adapters.
