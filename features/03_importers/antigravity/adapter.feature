Feature: Antigravity importer
  The Antigravity importer watches `conversation_summaries.db` for new
  and updated conversations, prefers each conversation's readable
  `brain/<id>/.system_generated/logs/transcript.jsonl`, and falls back to
  the per-conversation SQLite DB (protobuf payload blobs) for linkage the
  JSONL lacks. Per-conversation DBs are hot WAL databases: they are read
  through a snapshot copy, never in place.
  The summaries index comes in two schemas, and discovery picks whichever
  table exists. The current one is `conversation_summaries` (columns
  `conversation_id`, `title`, `last_modified_time` as a text datetime,
  `project_id`, `agent_name`, `parent_conversation_id`, `nesting_depth`
  and more); the older one is `conversations(id, name, updated_at)`. A
  conversation with a `parent_conversation_id` is a sub-agent spawned by
  that parent, at its `nesting_depth`. The per-conversation `payload.db` is
  optional and no longer exists on current installs.
  Evidence: transcript.jsonl records carry `content`, `created_at`,
  `source`, `status`, `step_index` and `type`: `USER_INPUT`,
  `PLANNER_RESPONSE`, `SYSTEM_MESSAGE`, `CHECKPOINT`, `ERROR_MESSAGE`,
  `GENERIC` and `EPHEMERAL_MESSAGE`. Planner steps often have no
  `content`, only `thinking` and `tool_calls`.

  Scenario: Conversations are discovered from the summaries index
    Given an Antigravity summaries index with 3 conversations
    When the Antigravity importer detects changes
    Then discovery yields 3 sessions

  Scenario: The transcript is the preferred source
    Given an Antigravity conversation with both a "transcript.jsonl" and a SQLite payload DB
    When the Antigravity importer extracts the conversation
    Then the messages come from the transcript records

  Scenario: Transcript types map to chapter-1 kinds
    Given an Antigravity transcript holding "USER_INPUT", "PLANNER_RESPONSE", "CHECKPOINT", and "ERROR_MESSAGE" records
    When the Antigravity importer extracts the transcript
    Then "USER_INPUT" and "PLANNER_RESPONSE" become user and assistant messages
    And "CHECKPOINT" becomes a compaction summary
    And "ERROR_MESSAGE" becomes an error message

  Scenario: Messages appear in step_index order
    Given an Antigravity transcript with steps "step_index 3", "step_index 1", and "step_index 2"
    When the Antigravity importer extracts the transcript
    Then the normalized messages appear in step order 1, 2, 3

  Scenario: The SQLite DB supplies linkage the transcript lacks
    Given an Antigravity conversation whose transcript lacks tool-call ids
    And whose payload DB carries the tool call and its result
    When the Antigravity importer extracts the conversation
    Then the tool call and result are linked by the DB-supplied call id

  Scenario: Hot per-conversation DBs are read through a snapshot copy
    Given an Antigravity conversation DB in WAL mode with a live app writing
    When the importer reads the DB through a snapshot copy
    Then the source DB and its WAL were never mutated
    And the live app kept writing rows without interference

  Scenario: Malformed steps are skipped and counted
    Given an Antigravity transcript holding 3 good records and 2 malformed ones
    When the Antigravity importer extracts the transcript
    Then extraction maps 3 good records
    And extraction reports 2 skipped records

  Scenario: The current summaries schema is discovered
    Given a current-schema Antigravity summaries index with a conversation "conv-1" and its sub-agents "conv-2" and "conv-3"
    When the Antigravity importer detects changes
    Then discovery yields 3 sessions
    And session "conv-1" has the project "demo-project", the title "title of conv-1" and a last activity in RFC 3339 shape

  Scenario: A conversation with a parent is a sub-agent at its nesting depth
    Given a current-schema Antigravity summaries index with a conversation "conv-1" and its sub-agents "conv-2" and "conv-3"
    When the Antigravity importer extracts the conversation "conv-2"
    Then the session is spawned by "conv-1" as a "research" sub-agent at nesting depth 1
    And extracting "conv-3" names its sub-agent kind "subagent"
    And extracting "conv-1" gives a session with no parent and no sub-agent info

  Scenario: The older summaries schema is still read
    Given an Antigravity summaries index with 2 conversations
    And a transcript for each conversation
    When the Antigravity importer detects changes
    Then discovery yields 2 sessions
    And extracting "conv-1" gives the project "demo-1" and no parent

  Scenario: A missing payload database is not an error
    Given a current-schema Antigravity summaries index with a conversation "conv-1" and its sub-agents "conv-2" and "conv-3"
    When the Antigravity importer extracts the conversation "conv-1"
    Then extraction maps 1 good record
    And extraction reports 0 skipped records

  Scenario: Current transcript records keep their roles, content and timestamps
    Given a current-schema Antigravity transcript with user, planner, generic, system, error and checkpoint steps
    When the Antigravity importer extracts the conversation "conv-1"
    Then the roles are user, assistant, tool, system, system and assistant
    And every message carries the record's "created_at" timestamp
    And the planner step holds its thinking and its tool call
    And the error step holds the record's error text and is an error
    And the checkpoint step is a compaction summary
    And extraction reports 0 skipped records

  Scenario: The working directory is the first file URI of the workspace list
    Given a current-schema Antigravity summaries index whose conversation "conv-1" has the workspaces "file:///work/my%20demo" and "file:///work/other"
    And a transcript for each conversation
    When the Antigravity importer detects changes
    Then session "conv-1" has the working directory "/work/my demo"
    And extracting "conv-1" gives the working directory "/work/my demo"

  Scenario: A workspace list with no file URI leaves the working directory unset
    Given a current-schema Antigravity summaries index whose conversation "conv-1" has the workspaces "vscode-remote://host/work/demo"
    And a transcript for each conversation
    When the Antigravity importer detects changes
    Then session "conv-1" has no working directory
    And extracting "conv-1" gives no working directory

  Scenario: The UUID project id is kept as the id but is not the working directory
    Given a current-schema Antigravity summaries index whose conversation "conv-1" has the workspaces "file:///work/demo"
    And a transcript for each conversation
    When the Antigravity importer detects changes
    Then extracting "conv-1" gives the project "demo-project" and the working directory "/work/demo"
