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
    The output should include 'Unified diff:'
  End

  It 'supports the workflow DSL on the local cli path'
    write_workflow_config

    When run run_review_and_dump
    The status should equal 0
    The output should include '"summary": "One issue found."'
    The output should include 'Review base: HEAD'
    The output should include 'Only report correctness issues.'
  End
End
