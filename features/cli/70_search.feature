Feature: botspy search
  Search mines the store, not the live sources. Text mode (FTS5) finds
  sessions by word, semantic mode (MiniLM embeddings) by meaning, and
  hybrid mode fuses both rankings. Every hit carries the session, the
  matched text, and the session's score, best session first. Filters
  compose with the query; a query that matches nothing is an empty
  result, never an error — and so is searching a store nothing was
  ever imported into.

  Background:
    Given a fixture home built from the synthetic corpus
    And the fixture home is imported into the text-only store

  Scenario: Text search finds the sessions whose messages match
    When I search the imported store for "parser"
    Then the exit code is 0
    And stdout reports "2 hits"
    And stdout contains "kick off the parser work"
    And stdout contains "the parser work is done"

  Scenario: A query that matches nothing is an empty result, not an error
    When I search the imported store for "zzz-no-such-word"
    Then the exit code is 0
    And stdout reports "0 hits"

  Scenario: Hits rank best session first, and every hit carries its coordinates
    When I search the imported store for "the" with JSON output
    Then the exit code is 0
    And stdout parses as a JSON array of 13 entries
    And the first JSON hit is for session "00000000-0000-4000-8000-000000000001"
    And every JSON entry has "session_id", "message_ordinal" and "score"

  Scenario: --limit caps the hits
    When I run "botspy search corpus --db {out_dir}/store.db --limit 1" against the fixture home
    Then the exit code is 0
    And stdout reports "1 hit"

  Scenario: --project composes with the search
    When I run "botspy search hello --db {out_dir}/store.db --project extra-demo" against the fixture home
    Then the exit code is 0
    And stdout reports "1 hit"

  Scenario: Searching a store nothing was imported into is an empty result, not an error
    When I run "botspy search anything --db {out_dir}/fresh.db" against an empty home
    Then the exit code is 0
    And stdout reports "0 hits"

  Scenario: Semantic search ranks the matching session first
    Given the fixture home is imported into the store with embeddings
    When I search the imported store semantically for "kick off the parser work"
    Then the exit code is 0
    And stdout contains "00000000-0000-4000-8000-000000000001"

  Scenario: Hybrid search fuses the text and semantic rankings
    Given the fixture home is imported into the store with embeddings
    When I search the imported store in hybrid mode for "kick off the parser work"
    Then the exit code is 0
    And stdout contains "00000000-0000-4000-8000-000000000001"

  Scenario: Semantic search over a text-only store is a clean empty result
    When I search the imported store semantically for "parser"
    Then the exit code is 0
    And stdout reports "0 hits"