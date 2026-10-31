Feature: Incremental refresh
  Refresh keeps the store in step with its sources without re-mining
  everything: a pass opens only the sessions whose summaries changed,
  leaves unchanged sessions untouched, and prunes sessions the source no
  longer reports. Running it again changes nothing.
  Evidence: BOTSPY-c7795b Local Store Initiative; transcripts exceed
  100 MB (some trees hold 1.4 GB and 6 GB stores), so re-mining every
  session on every pass does not scale.

  Scenario: A second pass ingests only what is new
    Given fixture sessions "f1" from "claude_code" and "f2" from "cursor"
    And a store at "refresh-new/store.db"
    When I ingest the registered adapters into the store
    And a fixture session "f3" from "codex" is registered
    And I refresh the store
    Then the refresh reports 1 new session, 0 updated, and 0 pruned
    When I iterate the sessions
    Then the iteration yields sessions "f1", "f2", and "f3"

  Scenario: A changed session is re-ingested and queries return the new content
    Given fixture sessions "f4" from "claude_code" and "f5" from "cursor"
    And a store at "refresh-changed/store.db"
    When I ingest the registered adapters into the store
    And session "f4" gets another message at "2026-10-01T10:00:00Z"
    And I refresh the store
    Then the refresh reports 0 new sessions, 1 updated, and 0 pruned
    When I iterate the messages of session "f4"
    Then the iteration yields the texts "hello from f4", "and one more" in that order

  Scenario: A session gone from the source is pruned
    Given fixture sessions "f6" from "claude_code" and "f7" from "cursor"
    And a store at "refresh-prune/store.db"
    When I ingest the registered adapters into the store
    And session "f7" is removed from its adapter
    And I refresh the store
    Then the refresh reports 0 new sessions, 0 updated, and 1 pruned
    When I iterate the sessions
    Then the iteration yields sessions "f6"

  @scoped_pruning
  Scenario: Refreshing one source prunes only that source's missing sessions
    Given fixture sessions "selected-gone" from "claude_code" and "other-kept" from "codex"
    And a store at "refresh-selected/store.db"
    When I ingest the registered adapters into the store
    And session "selected-gone" is removed from its adapter
    And I refresh only the "claude_code" adapter
    Then the refresh reports 0 new sessions, 0 updated, and 1 pruned
    When I iterate the sessions
    Then the iteration yields sessions "other-kept"

  @scoped_pruning
  Scenario: A selected-source dry run reports the same pruning without deleting anything
    Given fixture sessions "selected-dry-gone" from "claude_code" and "other-dry-kept" from "codex"
    And a store at "refresh-selected-dryrun/store.db"
    When I ingest the registered adapters into the store
    And session "selected-dry-gone" is removed from its adapter
    And I refresh only the "claude_code" adapter without writing
    Then the refresh reports 0 new sessions, 0 updated, and 1 pruned
    When I iterate the store's sessions
    Then the store iteration yields 2 sessions
    When I refresh only the "claude_code" adapter
    Then the refresh reports 0 new sessions, 0 updated, and 1 pruned
    When I iterate the sessions
    Then the iteration yields sessions "other-dry-kept"

  @scoped_pruning
  Scenario: Refreshing no adapters leaves stored history intact
    Given fixture sessions "unselected-one" from "claude_code" and "unselected-two" from "codex"
    And a store at "refresh-no-adapters/store.db"
    When I ingest the registered adapters into the store
    And I refresh with no adapters
    Then the refresh reports 0 new sessions, 0 updated, and 0 pruned
    When I iterate the store's sessions
    Then the store iteration yields 2 sessions

  Scenario: Refresh is idempotent
    Given fixture sessions "f8" from "claude_code" and "f9" from "cursor" and "g0" from "codex"
    And a store at "refresh-idempotent/store.db"
    When I ingest the registered adapters into the store
    And I refresh the store
    Then the refresh reports 0 new sessions, 0 updated, and 0 pruned
    When I iterate the sessions
    Then the iteration yields sessions "f8", "f9", and "g0"

  Scenario: Unchanged sessions are not re-opened from their adapters
    Given fixture sessions "f11" from "claude_code" and "f12" from "cursor"
    And a store at "refresh-open-count/store.db"
    When I ingest the registered adapters into the store
    And I snapshot the adapters' open count
    And I refresh the store
    And I snapshot the adapters' open count again
    Then the refresh reports 0 new sessions, 0 updated, and 0 pruned
    And the adapters served no session opens during the refresh

  Scenario: A cutoff refresh picks up only what is new and counts the rest skipped
    Given fixture sessions "r1" last active at "2026-10-01T09:00:00Z" and "r2" last active at "2026-09-01T09:00:00Z" and "r3" last active at "2026-10-01T09:30:00Z"
    And a store at "refresh-cutoff/store.db"
    When I ingest the registered adapters into the store
    And a fixture session "r4" last active at "2026-10-02T09:00:00Z" is registered
    And a fixture session "r5" last active at "2026-08-01T09:00:00Z" is registered
    When I refresh the store with a cutoff "2026-10-01T00:00:00Z"
    Then the refresh reports 1 new session, 0 updated, 0 pruned, and 2 skipped sessions
    When I iterate the sessions
    Then the iteration yields sessions "r4", "r3", "r1", and "r2"

  Scenario: A cutoff refresh never prunes the sessions it skipped
    Given fixture sessions "r7" last active at "2026-10-01T09:00:00Z" and "r8" last active at "2026-09-01T09:00:00Z" and "r9" last active at "2026-09-02T09:00:00Z"
    And a store at "refresh-cutoff-noprune/store.db"
    When I ingest the registered adapters into the store
    And I refresh the store with a cutoff "2026-10-01T00:00:00Z"
    Then the refresh reports 0 new sessions, 0 updated, 0 pruned, and 2 skipped sessions
    When I iterate the sessions
    Then the iteration yields sessions "r7", "r9", and "r8"

  Scenario: A refresh pass can be told not to prune
    Given fixture sessions "f13" from "claude_code" and "f14" from "cursor"
    And a store at "refresh-noprune/store.db"
    When I ingest the registered adapters into the store
    And session "f14" is removed from its adapter
    And I refresh the store without pruning
    Then the refresh reports 0 new sessions, 0 updated, and 0 pruned
    When I iterate the sessions
    Then the iteration yields sessions "f13" and "f14"

  Scenario: A dry-run refresh reports what would change and writes nothing
    Given fixture sessions "f15" from "claude_code" and "f16" from "cursor"
    And a store at "refresh-dryrun/store.db"
    When I ingest the registered adapters into the store
    And session "f15" gets another message at "2026-10-01T10:00:00Z"
    And I refresh the store without writing
    Then the refresh reports 0 new sessions, 1 updated, and 0 pruned
    When I iterate the messages of session "f15"
    Then the iteration yields the texts "hello from f15" in that order
