Feature: Claude Code importer
  The Claude Code importer watches append-only JSONL transcripts under
  `~/.claude/projects/<encoded-cwd>/` — one file per session — and emits
  normalized records as they appear. Sub-agent transcripts under
  `<session-id>/subagents/agent-<id>.jsonl` are imported as sessions of
  their own, spawned by the parent session. The source tree is only ever
  read.
  Evidence: session files carry `uuid`/`parentUuid`, `cost-state`,
  `pr-link`, `custom-title` records; files are appended live while
  sessions run. A sub-agent's `agent-<id>.meta.json` names its
  `agentType`, `description`, `toolUseId`, `spawnDepth` and `model`; its
  records are flagged `isSidechain` and carry the parent's `sessionId`.

  Scenario: Discovery finds one session per transcript
    Given a Claude Code source root with project transcripts "a.jsonl" and "b.jsonl"
    When the Claude Code importer detects changes
    Then discovery yields 2 sessions

  Scenario: A sub-agent is discovered as a session spawned by its parent
    Given a Claude Code session transcript "a.jsonl" with sub-agent "agent-1" of type "Explore" at depth 1
    When the Claude Code importer detects changes
    Then discovery yields 2 sessions
    And discovery lists the session "a--agent-1" in project "proj-a"
    And the doctor report counts 1 sub-agent
    And the doctor report counts 1 transcript

  Scenario: A sub-agent's kind and depth come from its meta file
    Given a Claude Code session transcript "a.jsonl" with sub-agent "agent-1" of type "Explore" at depth 2
    When the Claude Code importer extracts the sub-agent session "a--agent-1"
    Then the session is spawned by "a"
    And the session is a "Explore" sub-agent at depth 2
    And the session is titled "look around"
    And the session holds 2 messages

  Scenario: A sub-agent without a meta file is still imported
    Given a Claude Code session transcript "a.jsonl" with sub-agent "agent-1" and no meta file
    When the Claude Code importer extracts the sub-agent session "a--agent-1"
    Then the session is spawned by "a"
    And the session is a "subagent" sub-agent of unknown depth
    And the session holds 2 messages

  Scenario: Sidechain records are not counted in the parent session
    Given a Claude Code session transcript "a.jsonl" with sub-agent "agent-1" of type "Explore" at depth 1
    When the Claude Code importer extracts the transcript
    Then extraction yields 1 message
    And the session has no parent and is not a sub-agent

  Scenario: A stray sidechain record in a main transcript is skipped
    Given a Claude Code transcript "a.jsonl" holding a user record and a stray sidechain record
    When the Claude Code importer extracts the transcript
    Then extraction yields 1 message
    And 1 record is skipped as unknown

  Scenario: Appended records appear on the next read
    Given a Claude Code transcript "a.jsonl" with 3 records
    When the Claude Code importer extracts the transcript
    Then extraction yields 3 messages
    When 2 more records are appended to the transcript
    And the Claude Code importer extracts the transcript again
    Then extraction yields 2 more messages

  Scenario: Native ids become provenance, not identity
    Given a Claude Code transcript "a.jsonl" holding a user record "uuid-1" with parent "uuid-0"
    When the Claude Code importer extracts the transcript
    Then the first message's provenance has record_id "uuid-1" and parent record "uuid-0"

  Scenario: Auxiliary records park in session state
    Given a Claude Code transcript "a.jsonl" holding "cost-state", "pr-link", and "custom-title" records
    When the Claude Code importer extracts the transcript
    Then the session carries the cost, the PR link, and the title

  Scenario: Partial trailing lines are held back, malformed lines skipped
    Given a Claude Code transcript "a.jsonl" with 2 records ending in a partial trailing line
    When the Claude Code importer extracts the transcript
    Then extraction yields 2 messages
    And 1 partial trailing line is held back
    When the file is completed with the rest of the record
    And the Claude Code importer extracts the transcript again
    Then extraction yields 1 more message

  Scenario: The working directory comes from the transcript records
    Given a Claude Code transcript "a.jsonl" whose records carry the working directory "/work/Chattic.us"
    When the Claude Code importer extracts the transcript
    Then the session's working directory is "/work/Chattic.us"
    And discovery lists the session "a" with the working directory "/work/Chattic.us"

  Scenario: The first non-empty working directory names the project
    Given a Claude Code transcript "a.jsonl" whose first record has no working directory and later records name "/work/first" and "/work/second"
    When the Claude Code importer extracts the transcript
    Then the session's working directory is "/work/first"

  Scenario: A transcript whose records carry no working directory leaves it unset
    Given a Claude Code transcript "a.jsonl" with 2 records
    When the Claude Code importer extracts the transcript
    Then the session has no working directory

  Scenario: A sub-agent's working directory comes from its own records
    Given a Claude Code sub-agent whose records carry the working directory "/work/Chattic.us"
    When the Claude Code importer extracts the sub-agent session "a--agent-1"
    Then the session's working directory is "/work/Chattic.us"
    And discovery lists the session "a--agent-1" with the working directory "/work/Chattic.us"
