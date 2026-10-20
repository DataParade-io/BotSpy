Feature: botspy import
  One verb mines every registered adapter into the local store: the
  CDC pipeline the CLI fronts. The store lives at --db when given,
  otherwise at the default store path under the home (the same home
  the library derives). Incremental by default — a second import only
  mines what changed — with --full re-extracting from byte zero and
  --dry-run reporting what would land and writing nothing. Every run
  reports per outcome; skipped records never fail the run. The sources
  stay read-only, always.

  Background:
    Given a fixture home built from the synthetic corpus

  Scenario: Import mines every adapter into the default store
    When I run "botspy import" against the fixture home
    Then the exit code is 0
    And stdout reports "12 new sessions"
    And stdout reports "0 updated"
    And stdout reports "0 pruned"
    And the file at "{home}/.botspy/store.db" exists

  Scenario: A second import is incremental — nothing new, nothing re-mined
    When I run "botspy import" against the fixture home
    And I run "botspy import" against the fixture home
    Then the exit code is 0
    And stdout reports "0 new sessions"
    And stdout reports "12 unchanged"

  Scenario: --db places the store at a caller-given path
    When I run "botspy import --db {out_dir}/imported.db" against the fixture home
    Then the exit code is 0
    And stdout reports "12 new sessions"
    And the file at "{out_dir}/imported.db" exists

  Scenario: --dry-run reports what would land and writes nothing
    When I run "botspy import --dry-run" against the fixture home
    Then the exit code is 0
    And stdout reports "12 new sessions"
    And stdout reports "dry run"
    And the file at "{home}/.botspy/store.db" does not exist

  Scenario: --full re-extracts the corpus without changing anything
    When I run "botspy import --no-embed --db {out_dir}/full.db" against the fixture home
    And I run "botspy import --no-embed --full --db {out_dir}/full.db" against the fixture home
    Then the exit code is 0
    And stdout reports "0 new sessions"
    And stdout reports "12 unchanged"

  Scenario: An import after a transcript change reports the update
    When I run "botspy import --no-embed --db {out_dir}/delta.db" against the fixture home
    And the fixture home's solo transcript gains a message
    And I run "botspy import --no-embed --db {out_dir}/delta.db" against the fixture home
    Then the exit code is 0
    And stdout reports "1 updated"
    And stdout reports "11 unchanged"

  Scenario: An import after a transcript disappears prunes the session
    When I run "botspy import --no-embed --db {out_dir}/prune.db" against the fixture home
    And the fixture home's solo transcript is deleted
    And I run "botspy import --no-embed --db {out_dir}/prune.db" against the fixture home
    Then the exit code is 0
    And stdout reports "1 pruned"
    And stdout reports "11 unchanged"

  @scoped_pruning
  Scenario: A filtered import preserves sessions belonging to other sources
    When I run "botspy import --no-embed --db {out_dir}/selected.db" against the fixture home
    And I run "botspy import --source claude_code --no-embed --db {out_dir}/selected.db" against the fixture home
    Then the exit code is 0
    And stdout reports "0 pruned"
    When I run "botspy sessions --db {out_dir}/selected.db" against the fixture home
    Then the exit code is 0
    And stdout reports "12 sessions"

  @scoped_pruning
  Scenario: A filtered import prunes missing selected sessions and preserves all others
    When I run "botspy import --no-embed --db {out_dir}/selected-prune.db" against the fixture home
    And the fixture home's solo transcript is deleted
    And I run "botspy import --source claude_code --no-embed --dry-run --db {out_dir}/selected-prune.db" against the fixture home
    Then the exit code is 0
    And stdout reports "1 pruned"
    And stdout reports "dry run"
    When I run "botspy sessions --db {out_dir}/selected-prune.db" against the fixture home
    Then the exit code is 0
    And stdout reports "12 sessions"
    When I run "botspy import --source claude_code --no-embed --db {out_dir}/selected-prune.db" against the fixture home
    Then the exit code is 0
    And stdout reports "1 pruned"
    When I run "botspy sessions --db {out_dir}/selected-prune.db" against the fixture home
    Then the exit code is 0
    And stdout reports "11 sessions"

  Scenario: Sessions read from the store once it exists
    When I run "botspy import --db {out_dir}/imported.db" against the fixture home
    And I run "botspy sessions --db {out_dir}/imported.db" against an empty home
    Then the exit code is 0
    And stdout reports "12 sessions"

  Scenario: --limit caps the store-backed listing
    When I run "botspy import --db {out_dir}/imported.db" against the fixture home
    And I run "botspy sessions --db {out_dir}/imported.db --limit 2" against an empty home
    Then the exit code is 0
    And stdout reports "2 sessions"
