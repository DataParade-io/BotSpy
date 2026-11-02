Feature: botspy doctor
  Per-source diagnostics: each adapter's doctor report — its root or
  store, the counts it discovered, and its issues. Doctor reports; it
  does not gate: the exit code stays 0 even when a source reports
  issues, because skipped files and missing stores are data about the
  machine, not failures of the run.

  Background:
    Given a fixture home built from the synthetic corpus

  Scenario: Doctor reports every source without issues
    When I run "botspy doctor" against the fixture home
    Then the exit code is 0
    And stdout contains "claude_code", "cursor", "codex", "grok_bot" and "antigravity"
    And stdout contains "0 issues"

  Scenario: Doctor details one source
    When I run "botspy doctor --source cursor" against the fixture home
    Then the exit code is 0
    And stdout contains "cursor"
    And stdout does not contain "claude_code"

  Scenario: A missing source is reported, not fatal
    Given a fixture home with only the sources "claude_code, codex"
    When I run "botspy doctor" against the fixture home
    Then the exit code is 0
    And stdout contains "KV store file is missing"
    And stdout contains "persistence directory is missing"

  Scenario: JSON output carries the issues per source
    When I run "botspy doctor -o json" against the fixture home
    Then the exit code is 0
    And stdout parses as a JSON array of 6 entries
    And every JSON entry has an "issues" list
    And no JSON entry has any issues