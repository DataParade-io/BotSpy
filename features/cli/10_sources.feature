Feature: botspy sources
  The source registry as a command: every known source name, the root
  BotSpy resolved for it (after --root/--home overrides), how many
  sessions discovery finds there, and the source's status. Sources whose
  files are missing are listed as missing, never hidden.

  Background:
    Given a fixture home built from the synthetic corpus

  Scenario: The registry lists every known source with its resolved root
    When I run "botspy sources --no-truncate" against the fixture home
    Then the exit code is 0
    And stdout contains "claude_code", "cursor", "cursor_cli", "codex", "grok_bot" and "antigravity"
    And stdout contains ".claude" and "projects"
    And stdout contains "sand-client-persistence"

  Scenario: JSON output carries name, root, status and session count
    When I run "botspy sources -o json" against the fixture home
    Then the exit code is 0
    And stdout parses as a JSON array of 6 entries
    And the JSON entry "claude_code" has "sessions" 4
    And the JSON entry "codex" has "sessions" 2
    And the JSON entry "cursor" has "sessions" 2
    And the JSON entry "cursor_cli" has "sessions" 1
    And the JSON entry "grok_bot" has "sessions" 3
    And the JSON entry "antigravity" has "sessions" 2
    And every JSON entry has a non-empty "root"
    And every JSON entry has "status" "ok"

  Scenario: --source details one source
    When I run "botspy sources --source cursor -o json" against the fixture home
    Then the exit code is 0
    And stdout parses as a JSON array of 1 entry
    And the JSON entry "cursor" has "sessions" 2

  Scenario: --root overrides the derived root
    When I run "botspy sources --source claude_code --root {claude_projects_root} -o json" against the fixture corpus
    Then the exit code is 0
    And stdout parses as a JSON array of 1 entry
    And the JSON entry "claude_code" has "root" "{claude_projects_root}"
    And the JSON entry "claude_code" has "sessions" 3

  Scenario: A source whose files are missing is listed as missing
    Given a fixture home with only the sources "claude_code, codex"
    When I run "botspy sources -o json" against the fixture home
    Then the exit code is 0
    And the JSON entry "grok_bot" has "status" "missing"
    And the JSON entry "grok_bot" has "sessions" 0
    And the JSON entry "cursor" has "status" "missing"

  Scenario: An unknown source name is rejected with the known names
    When I run "botspy sources --source notepad"
    Then the exit code is 1
    And stderr contains "notepad", "claude_code" and "antigravity"

  Scenario: Human output truncates long paths until --no-truncate
    When I run "botspy sources" against the fixture home
    Then the exit code is 0
    And stdout does not contain the full claude projects path
    When I run "botspy sources --no-truncate" against the fixture home
    Then stdout contains the full claude projects path