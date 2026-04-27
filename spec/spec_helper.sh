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
    git -c init.defaultBranch=main init >/dev/null 2>&1
    git config user.name "Blick ShellSpec" >/dev/null 2>&1
    git config user.email "blick@example.com" >/dev/null 2>&1
    git config commit.gpgsign false >/dev/null 2>&1
    cat > src/main.rs <<'EOF'
fn main() {
    println!("hello");
}
EOF
    git add src/main.rs >/dev/null 2>&1
    git commit -m "initial commit" >/dev/null 2>&1
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
[agent]
kind = "codex"
model = "openai/gpt-5"

[defaults]
base = "HEAD"

[[reviews]]
name = "default"
EOF
}

write_workflow_config() {
  mkdir -p "${REPO}/skills/correctness"
  cat > "${REPO}/skills/correctness/SKILL.md" <<'EOF'
# correctness

Only report correctness issues.
Return JSON only.
EOF

  cat > "${REPO}/blick.toml" <<'EOF'
[agent]
kind = "codex"
model = "openai/gpt-5"

[defaults]
base = "HEAD"

[[skills]]
name = "correctness"
source = "./skills/correctness"

[[reviews]]
name = "default"
skills = ["correctness"]
prompt = """
Review base: HEAD
"""
EOF
}

write_multi_review_config() {
  cat > "${REPO}/blick.toml" <<'EOF'
[agent]
kind = "codex"
model = "openai/gpt-5"

[defaults]
base = "HEAD"

[[reviews]]
name = "security"

[[reviews]]
name = "technical"
EOF
}

write_multi_scope_config() {
  mkdir -p "${REPO}/apps/web/src" "${REPO}/apps/ios/src"
  cat > "${REPO}/blick.toml" <<'EOF'
[agent]
kind = "codex"
model = "openai/gpt-5"

[defaults]
base = "HEAD"

[[reviews]]
name = "root-review"
EOF
  cat > "${REPO}/apps/web/blick.toml" <<'EOF'
[[reviews]]
name = "web-review"
EOF
  cat > "${REPO}/apps/ios/blick.toml" <<'EOF'
[[reviews]]
name = "ios-review"
EOF

  (
    cd "${REPO}" || exit 1
    git add blick.toml apps/web/blick.toml apps/ios/blick.toml >/dev/null 2>&1
    echo 'fn web() {}' > apps/web/src/lib.rs
    echo 'fn ios() {}' > apps/ios/src/lib.rs
    git add apps/web/src/lib.rs apps/ios/src/lib.rs >/dev/null 2>&1
    git commit -m "add scopes" >/dev/null 2>&1
    echo 'fn web_changed() {}' > apps/web/src/lib.rs
    echo 'fn ios_changed() {}' > apps/ios/src/lib.rs
  )
}

run_review_and_dump() {
  (
    cd "${REPO}" || exit 1
    PATH="${FAKE_BIN}:/usr/bin:/bin" \
      PROMPT_LOG="${PROMPT_LOG}" \
      SYSTEM_LOG="${SYSTEM_LOG}" \
      "${BLICK_BIN}" review --json > "${WORKDIR}/review.out" 2>/dev/null
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

run_review_named() {
  name="${1:-}"
  (
    cd "${REPO}" || exit 1
    if [ -n "${name}" ]; then
      PATH="${FAKE_BIN}:/usr/bin:/bin" \
        PROMPT_LOG="${PROMPT_LOG}" \
        SYSTEM_LOG="${SYSTEM_LOG}" \
        "${BLICK_BIN}" review "${name}" --json > "${WORKDIR}/review.out" 2>/dev/null
    else
      PATH="${FAKE_BIN}:/usr/bin:/bin" \
        PROMPT_LOG="${PROMPT_LOG}" \
        SYSTEM_LOG="${SYSTEM_LOG}" \
        "${BLICK_BIN}" review --json > "${WORKDIR}/review.out" 2>/dev/null
    fi
  ) || return $?
  cat "${WORKDIR}/review.out"
}

run_review_with_env() {
  env_pairs="$1"
  (
    cd "${REPO}" || exit 1
    # shellcheck disable=SC2086
    env ${env_pairs} \
      PATH="${FAKE_BIN}:/usr/bin:/bin" \
      PROMPT_LOG="${PROMPT_LOG}" \
      SYSTEM_LOG="${SYSTEM_LOG}" \
      "${BLICK_BIN}" review --json 2>/dev/null
  )
}

run_config_explain() {
  (
    cd "${REPO}" || exit 1
    "${BLICK_BIN}" config --explain
  )
}

list_run_logs() {
  find "${REPO}/.blick/runs" -type f 2>/dev/null
}
