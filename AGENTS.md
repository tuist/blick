# Notes for AI agents working in this repo

This file is the entry point for AI agents (Claude, Codex, opencode, Cursor, etc.) working on `blick`. `CLAUDE.md` is a symlink to this file so any tool that follows the legacy name still finds it.

The README is the authoritative documentation for what blick does and how it's used; this file only lists the conventions an agent needs to know to land changes safely.

## Pull request titles

Pull request titles must follow the [Conventional Commits](https://www.conventionalcommits.org/) format:

```
<type>(<optional scope>): <short summary>
```

Common types:

- `feat:` - a new user-visible capability (triggers a minor release)
- `fix:` - a bug fix (triggers a patch release)
- `docs:` - documentation-only changes
- `refactor:`, `perf:`, `test:`, `style:` - non-functional improvements
- `chore:`, `ci:` - tooling and infrastructure (do **not** appear in the changelog or trigger a release)

Squash merges use the PR title verbatim as the commit subject, and `cliff.toml` parses that subject to decide whether to cut a release. A PR titled `feat: add X` will produce a `feat: add X (#NN)` commit on `main` and the release workflow will pick it up. A PR titled `wip: tweaks` will not.

If the substantive change in a `chore:` or `ci:` PR really should produce a release, follow up with a tiny empty commit on `main` whose subject is the right `feat:` / `fix:` line so the release pipeline notices.

## Other things worth knowing

- `mise install` provisions the pinned toolchain (Rust, Bazelisk, git-cliff, shellspec, opencode).
- `mise run test` runs the full local matrix: `cargo test`, `bazelisk test //...`, `shellspec`. Run it before pushing.
- `mise run fmt` formats; CI's `Format` job rejects unformatted Rust.
- This repo dogfoods itself: PRs are reviewed by the [`Blick Review`](.github/workflows/blick-review.yml) workflow, which runs the local `blick` binary against the diff using opencode + an LLM. Don't be surprised if your PR gets a check run named `blick / ...`.
- `blick.toml` at the repo root configures the dogfood review. Changing the agent or model here changes what reviews this repo's PRs.
