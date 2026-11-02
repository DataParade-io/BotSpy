Feature: Cursor importer
  The Cursor importer watches two sources: the IDE's SQLite KV store
  (`state.vscdb` — `composerHeaders`, `composerData:<id>`,
  `bubbleId:<composerId>:<bubbleId>` values, content-addressed protobuf
  `agentKv:blob:<sha256>` blobs) and the CLI agent transcripts under
  `~/.cursor/projects/<encoded-cwd>/agent-transcripts/`. The store is a
  hot 6 GB WAL database: it is read through a snapshot copy, never in
  place, never written.
  The IDE store lives in the platform's application-data directory:
  `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb`
  on macOS, `~/.config/Cursor/User/globalStorage/state.vscdb` on Linux and
  `%APPDATA%/Cursor/User/globalStorage/state.vscdb` on Windows.
  CLI transcripts come in two layouts. The current one nests each session
  in its own directory, `agent-transcripts/<session>/<session>.jsonl`, with
  its sub-agents beside it at `agent-transcripts/<session>/subagents/<id>.jsonl`;
  the older one is a flat `agent-transcripts/<session>.jsonl`. Records are
  `{role, message}` lines with no timestamps, so a session's times come
  from the transcript file's modification time, recorded outside the
  message timestamps. A sub-agent is a session of its own, spawned by its
  parent, with the id `<parent>--agent-<id>`.
  Current stores keep the composer headers in a `composerHeaders` table
  (`composerId`, `createdAt`, `lastUpdatedAt`, `value`) instead of the
  `composerHeaders` key; discovery reads the key when it exists and the
  table otherwise.
  Evidence: bubble keys are unordered; order comes from
  `composerData.fullConversationHeadersOnly`.

  Scenario: Composers are discovered from the headers table
    Given a Cursor KV store with 3 composers in "composerHeaders"
    When the Cursor importer detects changes
    Then discovery yields 3 sessions

  Scenario: Composers are discovered from the migrated headers table
    Given a Cursor KV store whose composer headers live in the "composerHeaders" table with 3 composers
    When the Cursor importer detects changes
    Then discovery yields 3 sessions

  Scenario: Changed composers are re-detected by lastUpdatedAt
    Given a Cursor KV store with 3 composers in "composerHeaders"
    When the Cursor importer detects changes
    And the app updates composer "composer-2" with a new bubble and bumps "lastUpdatedAt"
    And the Cursor importer detects changes again
    Then only composer "composer-2" is reported as changed

  Scenario: Bubble order comes from the headers list, not the keys
    Given a Cursor composer "composer-1" whose bubbles have keys "b3", "b1", and "b2"
    And whose "fullConversationHeadersOnly" lists "b1", "b2", and "b3"
    When the Cursor importer extracts the composer
    Then the normalized messages appear in headers order "b1", "b2", and "b3"

  Scenario: Bubbles become messages with thinking and tool calls
    Given a Cursor composer "composer-2" with a user bubble, an assistant bubble with thinking, and a "toolFormerData" tool call
    When the Cursor importer extracts the composer
    Then the normalized messages map the bubble roles, thinking, and tool call kinds

  Scenario: Blob-only tool results stay blob references
    Given a Cursor composer "composer-3" whose tool result lives only in "agentKv:blob:<sha256>"
    When the Cursor importer extracts the composer
    Then the tool result is a blob part with the blob hash and container "agentKv:blob"

  Scenario: The CLI transcripts are discovered per project directory
    Given a Cursor projects directory with CLI transcripts "a.jsonl" and "b.jsonl"
    When the Cursor importer detects changes
    Then discovery yields 2 sessions

  Scenario: The hot WAL store is read through a snapshot copy
    Given a Cursor KV store in WAL mode with a live app writing bubbles
    When the importer reads the store through a snapshot copy
    Then the source store and its WAL were never mutated
    And the live app kept writing without interference

  Scenario: The older flat CLI layout is still read
    Given a Cursor projects directory with the flat CLI transcript "old.jsonl"
    When the Cursor importer detects changes
    Then discovery yields 1 session
    And extracting "old" maps 2 messages with the flat record shape

  Scenario: The nested CLI layout is discovered one session per directory
    Given a Cursor projects directory with the nested CLI session "s1" and its sub-agents "c1" and "c2"
    When the Cursor importer detects changes
    Then discovery yields 3 sessions
    And discovery counts 1 transcript and 2 sub-agents
    And the session ids are "s1", "s1--agent-c1" and "s1--agent-c2"

  Scenario: A CLI sub-agent is a session spawned by its parent
    Given a Cursor projects directory with the nested CLI session "s1" and its sub-agents "c1" and "c2"
    When the Cursor importer extracts "s1--agent-c1"
    Then the session is spawned by "s1" as a "subagent"
    And extracting the CLI session "s1" gives a session with no parent and no sub-agent info

  Scenario: Current CLI records carry text and tool calls without timestamps
    Given a Cursor projects directory with the nested CLI session "s1" and its sub-agents "c1" and "c2"
    When the Cursor importer extracts "s1"
    Then the user record is a user text message
    And the assistant record holds its text and a "Shell" tool call
    And the status record is skipped as not part of the conversation
    And no message has a timestamp

  Scenario: CLI sessions get approximate times from the file's modification time
    Given a Cursor projects directory with the nested CLI session "s1" and its sub-agents "c1" and "c2"
    When the Cursor importer extracts "s1"
    Then the CLI session records the file modification time outside message timestamps

  Scenario: CLI sessions without timestamps still appear in the timeline
    Given a Cursor projects directory with the nested CLI session "s1" and its sub-agents "c1" and "c2"
    When the examples read the data root's Cursor CLI sessions into the timeline
    Then lane "s1" shows one span marked approximate
    And lane "s1--agent-c1" is drawn indented under lane "s1"

  Scenario Outline: The IDE store is found where the platform keeps it
    When the default Cursor store is resolved for "<platform>" under the home "/srv/demo" with application data "<appdata>"
    Then the store path is "<path>"

    Examples:
      | platform | appdata            | path                                                                      |
      | macos    |                    | /srv/demo/Library/Application Support/Cursor/User/globalStorage/state.vscdb |
      | linux    |                    | /srv/demo/.config/Cursor/User/globalStorage/state.vscdb                  |
      | windows  | /apps/Roaming      | /apps/Roaming/Cursor/User/globalStorage/state.vscdb                       |
      | windows  |                    | /srv/demo/AppData/Roaming/Cursor/User/globalStorage/state.vscdb          |

  Scenario: The registry lists the Cursor CLI source beside the IDE store
    Given a fixture home built from the synthetic corpus
    When I run "botspy sources -o json" against the fixture home
    Then the exit code is 0
    And the JSON entry "cursor_cli" has "agent" "cursor"
    And the JSON entry "cursor_cli" has "sessions" 1
    And the JSON entry "cursor_cli" has "status" "ok"
    And the JSON entry "cursor" has "sessions" 2

  Scenario: The registry finds the IDE store at the platform path under --home
    Given a fixture home built from the synthetic corpus
    When I run "botspy sources --source cursor -o json" against the fixture home
    Then the exit code is 0
    And the JSON entry "cursor" has "status" "ok"
    And the JSON entry "cursor" has "sessions" 2

  Scenario: The registry honours a --root override for the Cursor CLI source
    Given a Cursor projects directory with the nested CLI session "s1" and its sub-agents "c1" and "c2"
    When I run "botspy sources --source cursor_cli --root {cursor_cli_root} -o json" against the fixture corpus
    Then the exit code is 0
    And the JSON entry "cursor_cli" has "sessions" 3

  Scenario: Importing one Cursor source imports both, so pruning stays correct
    Given a fixture home built from the synthetic corpus
    When I run "botspy import --source cursor_cli --no-embed" against the fixture home
    And I run "botspy import --source cursor --no-embed" against the fixture home
    Then the exit code is 0
    And stdout reports "0 new sessions, 0 updated sessions, 3 unchanged sessions, 0 pruned sessions"

  Scenario: A CLI project directory that names a dotted folder resolves to that folder
    Given a Cursor projects directory with a CLI session "s1" encoded from the real folder "Chattic.us"
    When the Cursor importer detects changes
    Then the discovered CLI session "s1" is in the real folder
    And the project id stays the encoded name
    When the Cursor importer extracts "s1"
    Then the CLI session "s1" is in the real folder

  Scenario: A CLI project directory that names a hyphenated folder resolves to that folder
    Given a Cursor projects directory with a CLI session "s1" encoded from the real folder "my-app"
    When the Cursor importer detects changes
    Then the discovered CLI session "s1" is in the real folder

  Scenario: A CLI project directory that names a folder inside a dotted parent resolves
    Given a Cursor projects directory with a CLI session "s1" encoded from the real folder "dev.tools/app"
    When the Cursor importer detects changes
    Then the discovered CLI session "s1" is in the real folder

  Scenario: A CLI project folder inside a folder that cannot be listed still resolves
    A folder can be passable without being listable, and some emulated
    environments list the root of another filesystem. The resolver also
    tries each leading stretch of the name directly, so the real folder is
    still found.
    Given a Cursor projects directory with a CLI session "s1" encoded from the real folder "app" inside a folder that cannot be listed
    When the Cursor importer detects changes
    Then the discovered CLI session "s1" is in the real folder

  Scenario: A CLI project directory whose folder does not exist leaves the working directory unset
    Given a Cursor projects directory with a CLI session "s1" encoded from a folder that does not exist
    When the Cursor importer detects changes
    Then the discovered CLI session "s1" has no working directory
    When the Cursor importer extracts "s1"
    Then the CLI session "s1" has no working directory
