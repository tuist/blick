Describe 'blick review'
  BeforeAll 'ensure_blick_binary'
  BeforeEach 'setup_review_fixture'
  AfterEach 'cleanup_review_fixture'

  It 'reviews a repository through the codex cli'
    write_codex_config

    When run run_review_and_dump
    The status should equal 0
    The output should include '"summary": "One issue found."'
    The output should include '"file": "src/main.rs"'
    The output should include 'notes.txt'
    The output should include 'Full PR diff (context):'
  End

  It 'attaches skill content and review prompt to the agent invocation'
    write_workflow_config

    When run run_review_and_dump
    The status should equal 0
    The output should include '"summary": "One issue found."'
    The output should include 'Base revision: HEAD'
  End

  It 'writes a per-task log file under .blick/runs'
    write_codex_config

    When call run_review_named ""
    The status should equal 0
    The output should include '"summary": "One issue found."'
    The path "${REPO}/.blick/runs" should be directory
  End

  It 'runs only the named review when one is requested'
    write_multi_review_config

    When call run_review_named "security"
    The status should equal 0
    The output should include '"summary": "One issue found."'
    The output should not include 'technical'
  End

  It 'runs all reviews when no name is provided and combines findings'
    write_multi_review_config

    When call run_review_named ""
    The status should equal 0
    The output should include 'security'
    The output should include 'technical'
  End

  It 'partitions changes across nested blick.toml scopes'
    write_multi_scope_config

    When call run_review_named ""
    The status should equal 0
    The output should include 'web-review'
    The output should include 'ios-review'
  End

  It 'honors the BLICK_AGENT_KIND override'
    write_codex_config

    When call run_review_with_env "BLICK_AGENT_KIND=codex BLICK_AGENT_MODEL=openai/gpt-5"
    The status should equal 0
    The output should include '"summary": "One issue found."'
  End
End

Describe 'blick config --explain'
  BeforeAll 'ensure_blick_binary'
  BeforeEach 'setup_review_fixture'
  AfterEach 'cleanup_review_fixture'

  It 'prints provenance for each scope'
    write_multi_scope_config

    When call run_config_explain
    The status should equal 0
    The output should include 'scope: .'
    The output should include 'scope: apps/web'
    The output should include 'scope: apps/ios'
    The output should include 'provenance:'
  End
End
