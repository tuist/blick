#!/bin/sh

ensure_blick_binary() {
  REPO_ROOT="$(pwd)"
  BLICK_BIN="${REPO_ROOT}/target/debug/blick"

  if [ ! -x "${BLICK_BIN}" ]; then
    cargo build --quiet
  fi
}

setup_review_fixture() {
  WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/blick-shellspec.XXXXXX")"
  REPO="${WORKDIR}/repo"
  FAKE_BIN="${WORKDIR}/bin"
  PROMPT_LOG="${WORKDIR}/prompt.log"
  SYSTEM_LOG="${WORKDIR}/system.log"

  mkdir -p "${REPO}" "${FAKE_BIN}" "${REPO}/src"

  cat > "${FAKE_BIN}/codex" <<'EOF'
#!/bin/sh
set -eu

prompt_log="${PROMPT_LOG:?}"
system_log="${SYSTEM_LOG:?}"

task=""
if [ "${1:-}" = "exec" ]; then
  shift
  task="${1:-}"
fi

printf '%s' "${task}" > "${prompt_log}"

if [ -n "${CODEX_HOME:-}" ] && [ -f "${CODEX_HOME}/config.toml" ]; then
  cp "${CODEX_HOME}/config.toml" "${system_log}"
fi

cat <<'JSON'
{"type":"thread.started","thread_id":"shellspec-thread"}
{"type":"item.completed","item":{"id":"msg-1","type":"agent_message","text":"{\"summary\":\"One issue found.\",\"findings\":[{\"severity\":\"medium\",\"file\":\"src/main.rs\",\"line\":1,\"title\":\"demo finding\",\"body\":\"Please add a regression test.\"}]}" }}
{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5}}
JSON
EOF
  chmod +x "${FAKE_BIN}/codex"

  (
    cd "${REPO}" || exit 1
    git init >/dev/null
    git config user.name "Blick ShellSpec" >/dev/null
    git config user.email "blick@example.com" >/dev/null
    cat > src/main.rs <<'EOF'
fn main() {
    println!("hello");
}
EOF
    git add src/main.rs
    git commit -m "initial commit" >/dev/null
    cat > src/main.rs <<'EOF'
fn main() {
    println!("hello, blick");
}
EOF
    cat > notes.txt <<'EOF'
remember the shellspec suite
EOF
  )
}

cleanup_review_fixture() {
  rm -rf "${WORKDIR}"
}

write_codex_config() {
  cat > "${REPO}/blick.toml" <<'EOF'
[llm]
provider = "codex"

[review]
base = "HEAD"
EOF
}

write_workflow_config() {
  cat > "${REPO}/blick.toml" <<'EOF'
[llm]
provider = "codex"

[review]
base = "HEAD"

[[review.workflow.steps]]
type = "prompt"
role = "system"
content = """
Only report correctness issues.
Return JSON only.
"""

[[review.workflow.steps]]
type = "prompt"
role = "user"
content = """
Review base: {{base}}

Files:
{{files}}

Patch:
{{diff}}
"""

[[review.workflow.steps]]
type = "llm_review"
EOF
}

run_review_and_dump() {
  (
    cd "${REPO}" || exit 1
    PATH="${FAKE_BIN}:/usr/bin:/bin" \
      PROMPT_LOG="${PROMPT_LOG}" \
      SYSTEM_LOG="${SYSTEM_LOG}" \
      "${BLICK_BIN}" review --config blick.toml --json > "${WORKDIR}/review.out"
  ) || return $?

  printf '=== review ===\n'
  cat "${WORKDIR}/review.out"
  printf '\n=== prompt ===\n'
  cat "${PROMPT_LOG}"
  if [ -f "${SYSTEM_LOG}" ]; then
    printf '\n=== system ===\n'
    cat "${SYSTEM_LOG}"
  fi
}
