Feature: botspy stats
  One glance at what the store holds: sessions bucketed by source, by
  project, or by the day of last activity, biggest bucket first. Stats
  read the store (--db), never the live sources; an empty store is a
  clean zero, not an error.

  Background:
    Given a fixture home built from the synthetic corpus
    And the fixture home is imported into the text-only store

  Scenario: Stats bucket the sessions by source
    When I run "botspy stats --db {out_dir}/store.db" against the fixture home
    Then the exit code is 0
    And stdout reports "13 sessions"
    And stdout contains "claude_code" "4"
    And stdout contains "antigravity" "2"
    And stdout contains "grok_bot" "2"

  Scenario: --by project buckets by project id
    When I run "botspy stats --by project --db {out_dir}/store.db" against the fixture home
    Then the exit code is 0
    And stdout reports "13 sessions"
    And stdout contains "extra-demo" "1"
    And stdout contains "/workspace/alpha" "2"
    And stdout contains "no project" "2"

  Scenario: --by day buckets by the day of last activity
    When I run "botspy stats --by day --db {out_dir}/store.db" against the fixture home
    Then the exit code is 0
    And stdout reports "13 sessions"
    And stdout contains "2026-10-01" "7"
    And stdout contains "no activity" "5"

  Scenario: Stats serialize as JSON
    When I run "botspy stats --db {out_dir}/store.db -o json" against the fixture home
    Then the exit code is 0
    And stdout parses as a JSON array of 5 entries

  Scenario: Stats over an empty store are a clean zero
    When I run "botspy stats --db {out_dir}/empty.db" against an empty home
    Then the exit code is 0
    And stdout reports "0 sessions"