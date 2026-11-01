Feature: botspy scan
  botspy scan mines the adapters into the local store — the store's
  ingest/CDC pass, on the command line. By default every registered
  source is scanned into the one store at $BOTSPY_HOME/.botspy/store.db.
  The pass adds new sessions and updates changed ones; it never prunes:
  an adapter that reports nothing must never delete stored history, so a
  source yielding zero sessions (the Codex zero-sessions bug BOTSPY-81143b)
  stays visible without being destructive. Exit codes: 0 success
  (including adapters that report nothing), 1 unknown source, 2 usage
  error, 3 store failure.

  A scan runs one full pass to start, then keeps running: it re-scans
  every `--interval` seconds (default 60) until it is interrupted. The
  interval is measured from the end of the previous pass, so passes
  never overlap. An interrupt (SIGINT/SIGTERM) lets the current pass
  finish and commit, then stops the loop with exit code 130. Errors in
  one pass are reported in that pass's output and never kill the loop.
  `--once` runs exactly one pass and exits — the shape scripts and CI
  want. The loop is an ordinary foreground process the user can
  background with their own tooling; there is no daemon.

  Background:
    Given a fixture home built from the synthetic corpus

  Scenario: A default scan fills the store from every source
    When I run "botspy scan --once" with BOTSPY_HOME at the fixture home
    Then the exit code is 0
    And the fixture store holds 13 sessions
    And stdout contains "claude_code", "cursor", "codex", "grok_bot" and "antigravity"
    And stdout reports "13 added"

  Scenario: The scan report names the store and its elapsed time
    When I run "botspy scan --once -o json --store {out_dir}/scan-meta.db" against the fixture home
    Then the exit code is 0
    And the JSON "store" is "{out_dir}/scan-meta.db"
    And the JSON "elapsed_ms" is more than 0
    And the JSON totals report "new" 13

  Scenario: A dry-run scan reports what would change and writes nothing
    When I run "botspy scan --once --dry-run" with BOTSPY_HOME at the fixture home
    Then the exit code is 0
    And stdout reports "13 added"
    And stdout reports "dry run"
    And the fixture store holds 0 sessions

  Scenario: JSON output carries the per-adapter report
    When I run "botspy scan --once -o json" with BOTSPY_HOME at the fixture home
    Then the exit code is 0
    And stdout parses as a JSON object
    And the JSON "adapters" list has 6 entries
    And the JSON scan source "claude_code" has "discovered" 4 and "added_sessions" 4
    And the JSON scan source "cursor" has "discovered" 2 and "added_sessions" 2
    And the JSON scan source "cursor_cli" has "discovered" 1 and "added_sessions" 1
    And the JSON scan source "codex" has "discovered" 2 and "added_sessions" 2
    And the JSON scan source "grok_bot" has "discovered" 3 and "added_sessions" 2
    And the JSON scan source "grok_bot" has "errors" 1
    And the JSON scan source "antigravity" has "discovered" 2 and "added_sessions" 2

  Scenario: NDJSON streams one entry per source
    When I run "botspy scan --once -o ndjson" with BOTSPY_HOME at the fixture home
    Then the exit code is 0
    And stdout parses as NDJSON with 6 lines

  Scenario: JSON output counts the messages the scan brought in
    When I run "botspy scan --once --source claude_code -o json --store {out_dir}/scan-msgs.db" against the fixture home
    Then the exit code is 0
    And the JSON scan source "claude_code" has "added_sessions" 4 and "added_messages" 8

  Scenario: --source scans a single adapter
    When I run "botspy scan --once --source claude_code --store {out_dir}/scan-single.db" against the fixture home
    Then the exit code is 0
    And the store at "{out_dir}/scan-single.db" holds 4 sessions

  Scenario: --source can be repeated
    When I run "botspy scan --once --source claude_code --source codex --store {out_dir}/scan-list.db" against the fixture home
    Then the exit code is 0
    And the store at "{out_dir}/scan-list.db" holds 6 sessions

  Scenario: --since keeps sessions last active before the cutoff out of the store
    When I run "botspy scan --once --source claude_code --since 2026-09-20T00:00:00Z --store {out_dir}/scan-since.db -o json" against the fixture home
    Then the exit code is 0
    And the store at "{out_dir}/scan-since.db" holds 3 sessions
    And the JSON scan source "claude_code" has "skipped_sessions" 1

  Scenario: --since accepts an ISO date
    When I run "botspy scan --once --source claude_code --since 2026-09-20 --store {out_dir}/scan-date.db" against the fixture home
    Then the exit code is 0
    And the store at "{out_dir}/scan-date.db" holds 3 sessions

  Scenario: --since accepts a relative duration
    When I run "botspy scan --once --source claude_code --since 1h --store {out_dir}/scan-recent.db" against the fixture home
    Then the exit code is 0
    And the store at "{out_dir}/scan-recent.db" holds 0 sessions

  Scenario: --since accepts a duration that reaches back past every session
    When I run "botspy scan --once --since 10000w --store {out_dir}/scan-old.db" against the fixture home
    Then the exit code is 0
    And the store at "{out_dir}/scan-old.db" holds 13 sessions

  Scenario: A cutoff scan never deletes the stored sessions it skipped
    When I run "botspy scan --once --store {out_dir}/scan-keep.db" against the fixture home
    Then the exit code is 0
    And the store at "{out_dir}/scan-keep.db" holds 13 sessions
    When I run "botspy scan --once --since 2999-01-01T00:00:00Z --store {out_dir}/scan-keep.db -o json" against the fixture home
    Then the exit code is 0
    And the store at "{out_dir}/scan-keep.db" holds 13 sessions
    And the JSON scan source "claude_code" has "skipped_sessions" 4

  Scenario: A source with no sessions is reported with its zero row
    Given a fixture home with only the sources "claude_code"
    When I run "botspy scan --once --store {out_dir}/scan-zeros.db -o json" against the fixture home
    Then the exit code is 0
    And the JSON scan source "codex" has "discovered" 0 and "added_sessions" 0
    And the JSON scan source "codex" has "errors" 0
    And the JSON totals report "new" 4

  Scenario: An unknown source is rejected
    When I run "botspy scan --source notepad"
    Then the exit code is 1
    And stderr contains "notepad", "claude_code" and "antigravity"

  Scenario: An invalid --since value is a usage error
    When I run "botspy scan --since bogus"
    Then the exit code is 2
    And stderr contains "--since" "bogus"

  Scenario: A file that is not a store fails cleanly
    Given a non-store file at "{out_dir}/bogus.db"
    When I run "botspy scan --store {out_dir}/bogus.db" against the fixture home
    Then the exit code is 3
    And stderr mentions "not a BotSpy store"

  Scenario: A scan runs one pass to start, then keeps scanning on the interval
    When I scan "botspy scan --store {out_dir}/scan-loop.db --interval 1" in a loop for 2 ticks with BOTSPY_HOME at the fixture home
    Then the exit code is 0
    And stdout reports the passes 1, 2 and 3 in order
    And the store at "{out_dir}/scan-loop.db" holds 13 sessions

  Scenario: The interval defaults to 60 seconds
    When I run "botspy scan --help"
    Then the exit code is 0
    And stdout contains "--interval" and "[default: 60]"

  Scenario: --once runs exactly one pass and exits, for scripts and CI
    When I run "botspy scan --once" with BOTSPY_HOME at the fixture home
    Then the exit code is 0
    And stdout reports "13 added"
    And stdout does not contain "pass 2"
    And the fixture store holds 13 sessions

  Scenario: An interval below one second is a usage error
    When I run "botspy scan --interval 0 --once"
    Then the exit code is 2
    And stderr contains "--interval"

  Scenario: An interrupt between passes stops the loop cleanly with exit code 130
    When I scan "botspy scan --store {out_dir}/scan-sig.db --interval 1" in a loop for 1 tick and then the interrupt lands with BOTSPY_HOME at the fixture home
    Then the exit code is 130
    And stdout reports the passes 1 and 2 in order
    And the store at "{out_dir}/scan-sig.db" holds 13 sessions

  Scenario: An interrupt during a pass lets the pass finish and commit, then stops
    When I scan "botspy scan --store {out_dir}/scan-sig2.db --interval 1" and the interrupt lands during the first pass with BOTSPY_HOME at the fixture home
    Then the exit code is 130
    And stdout reports the passes 1 in order
    And the store at "{out_dir}/scan-sig2.db" holds 13 sessions

  Scenario: A pass with adapter errors reports them and the loop keeps scanning
    When I scan "botspy scan --store {out_dir}/scan-err.db --interval 1" in a loop for 2 ticks with BOTSPY_HOME at the fixture home
    Then the exit code is 0
    And stdout reports the passes 1, 2 and 3 in order
    And stdout reports "1 errors"

  Scenario: Passes never overlap, however long one takes
    When I scan "botspy scan --store {out_dir}/scan-seq.db --interval 1" in a loop for 2 ticks with BOTSPY_HOME at the fixture home
    Then the exit code is 0
    And stdout reports the passes 1, 2 and 3 in order

  Scenario: A loop scan reports each pass in JSON
    When I scan "botspy scan --store {out_dir}/scan-loopjson.db --interval 1 -o json" in a loop for 1 tick with BOTSPY_HOME at the fixture home
    Then the exit code is 0
    And stdout parses as 2 JSON documents
    And JSON document 1 has "pass" 1
    And JSON document 2 has "pass" 2

  Scenario: A loop scan streams one line per source per pass in NDJSON
    When I scan "botspy scan --store {out_dir}/scan-loopnd.db --interval 1 -o ndjson" in a loop for 1 tick with BOTSPY_HOME at the fixture home
    Then the exit code is 0
    And stdout parses as NDJSON with 12 lines

  Scenario: A loop scan never prunes the sessions it skips
    When I run "botspy scan --once --store {out_dir}/scan-nokeep.db" against the fixture home
    Then the exit code is 0
    And the store at "{out_dir}/scan-nokeep.db" holds 13 sessions
    When I scan "botspy scan --store {out_dir}/scan-nokeep.db --interval 1 --since 2999-01-01T00:00:00Z" in a loop for 1 tick with BOTSPY_HOME at the fixture home
    Then the exit code is 0
    And the store at "{out_dir}/scan-nokeep.db" holds 13 sessions