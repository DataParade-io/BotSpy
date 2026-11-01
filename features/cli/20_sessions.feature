Feature: botspy sessions
  One unified listing across every registered source. Filters select by
  source, project, and activity window; --limit caps the listing. The
  output modes are human (truncated, pipe-tolerant), json, and ndjson —
  machine modes never truncate.

  Background:
    Given a fixture home built from the synthetic corpus

  Scenario: Sessions from every source are listed together
    When I run "botspy sessions" against the fixture home
    Then the exit code is 0
    And stdout reports "14 sessions"

  Scenario: JSON output is the session summaries serialized
    When I run "botspy sessions -o json" against the fixture home
    Then the exit code is 0
    And stdout parses as a JSON array of 14 entries
    And every JSON entry has "id", "agent" and "project_id"

  Scenario: NDJSON streams one summary per line
    When I run "botspy sessions -o ndjson" against the fixture home
    Then the exit code is 0
    And stdout parses as NDJSON with 14 lines

  Scenario: --source filters to one source
    When I run "botspy sessions --source claude_code" against the fixture home
    Then the exit code is 0
    And stdout reports "4 sessions"

  Scenario: --source can be repeated
    When I run "botspy sessions --source claude_code --source codex" against the fixture home
    Then the exit code is 0
    And stdout reports "6 sessions"

  Scenario: --project filters by project id
    When I run "botspy sessions --project extra-demo --no-truncate" against the fixture home
    Then the exit code is 0
    And stdout reports "1 session"
    And stdout contains "solo-abc123"

  Scenario: --limit caps the listing
    When I run "botspy sessions --limit 5" against the fixture home
    Then the exit code is 0
    And stdout reports "5 sessions"

  Scenario: --since excludes sessions with no activity that recent
    When I run "botspy sessions --since 2030-01-01T00:00:00Z" against the fixture home
    Then the exit code is 0
    And stdout reports "0 sessions"

  Scenario: Human output truncates session ids
    When I run "botspy sessions" against the fixture home
    Then the exit code is 0
    And stdout does not contain "00000000-0000-4000-8000-000000000001"
    And stdout contains "00000000"

  Scenario: Summaries carry real activity from the transcripts
    When I run "botspy sessions --source claude_code -o json" against the fixture home
    Then the exit code is 0
    And the JSON session "00000000-0000-4000-8000-000000000001" has "message_count" 4
    And the JSON session "00000000-0000-4000-8000-000000000001" has "started_at" "2026-10-01T09:00:00Z"
    And the JSON session "00000000-0000-4000-8000-000000000001" has "last_activity_at" "2026-10-01T09:00:15Z"
    And the JSON session "solo-abc123" has "message_count" 2
    And the JSON session "solo-abc123" has "started_at" "2026-09-15T12:00:00Z"
    And the JSON session "solo-abc123" has "last_activity_at" "2026-09-15T12:05:00Z"

  Scenario: Human output shows the recorded session title
    When I run "botspy sessions --source claude_code" against the fixture home
    Then the exit code is 0
    And stdout contains "Synthetic parser session"

  Scenario: --since keeps only sessions active at or after the timestamp
    When I run "botspy sessions --source claude_code --since 2026-10-01T09:05:00Z" against the fixture home
    Then the exit code is 0
    And stdout reports "2 sessions"

  Scenario: An invalid --since value is rejected
    When I run "botspy sessions --since bogus" against the fixture home
    Then the exit code is 2
    And stderr contains "--since" "bogus"

  Scenario: An invalid --until value is rejected
    When I run "botspy sessions --until not-a-time" against the fixture home
    Then the exit code is 2
    And stderr contains "--until" "not-a-time"

  Scenario: Sessions are ordered most recent activity first
    When I run "botspy sessions --source claude_code -o json" against the fixture home
    Then the exit code is 0
    And the JSON sessions are ordered "00000000-0000-4000-8000-000000000003,00000000-0000-4000-8000-000000000002,00000000-0000-4000-8000-000000000001,solo-abc123"

  Scenario: The same session id under two projects lists twice
    Given a fixture home with the same session id in two projects
    When I run "botspy sessions --source claude_code" against the fixture home
    Then the exit code is 0
    And stdout reports "5 sessions"
    And stdout contains "extra-demo" "dupe-demo"

  Scenario: Human output keeps the table aligned with long project ids
    When I run "botspy sessions" against the fixture home
    Then the exit code is 0
    And the sessions table aligns the LAST ACTIVITY column

  Scenario: --no-truncate keeps the table aligned with full values
    When I run "botspy sessions --no-truncate" against the fixture home
    Then the exit code is 0
    And the sessions table aligns the LAST ACTIVITY column

  Scenario: An unknown source filter is rejected
    When I run "botspy sessions --source notepad"
    Then the exit code is 1
    And stderr contains "notepad"