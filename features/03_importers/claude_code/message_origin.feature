Feature: Claude Code messages the harness wrote are not the human's
  Claude Code stores several kinds of text under the user role that the
  human never typed: skill bodies and other injected context (`isMeta`),
  the summary a compacted session restarts from (`isCompactSummary`),
  background-task notifications, hook and command output, mail from other
  agent sessions, and interruption markers. The importer keeps all of them
  as user messages and marks them with an origin, so a reader that wants
  only the human's words can leave them out. A message the human typed,
  or pasted, has no origin.
  Evidence: user records carry `isMeta` and `isCompactSummary` flags, and
  an `origin` key whose `kind` is `human` for typed text, `peer` for mail
  from another session, and `task-notification` for background tasks.
  Texts led by a harness tag, such as `<system-reminder>` or
  `<command-name>`, appear without any flag.

  Scenario: A typed message has no origin
    When the Claude Code importer reads a user record marked "origin:human" with the text "please fix the build"
    Then the message is a user message
    And the message has no origin, so it is the human's

  Scenario: A typed message from before records had an origin key has no origin
    When the Claude Code importer reads a user record marked "none" with the text "please fix the build"
    Then the message is a user message
    And the message has no origin, so it is the human's

  Scenario: A typed message that quotes a tag is still the human's
    The record says the human typed it. The kind of the origin key wins
    over what the text looks like.
    When the Claude Code importer reads a user record marked "origin:human" with the text "<system-reminder> is what I want to ask about"
    Then the message has no origin, so it is the human's

  Scenario Outline: Text the human pasted or ran stays the human's
    The `<pasted_content>` and `<bash-input>` wrappers hold the human's own
    input. Only the listed harness tags are marked, never every tag.
    When the Claude Code importer reads a user record marked "none" with the text "<text>"
    Then the message has no origin, so it is the human's

    Examples:
      | text                                      |
      | <pasted_content>a pasted log</pasted_content> |
      | <bash-input>ls -la</bash-input>           |

  Scenario Outline: A record the harness flagged is marked as injected
    When the Claude Code importer reads a user record marked "<mark>" with the text "<text>"
    Then the message is a user message
    And the message has origin "<origin>"

    Examples:
      | mark                         | text                                              | origin        |
      | isMeta                       | Base directory for this skill: a skill body       | harness       |
      | isCompactSummary             | A summary of the work so far                      | harness       |
      | origin:task-notification     | A background task finished                        | harness       |
      | origin:peer                  | A note from another session                       | agent_message |

  Scenario: A compaction summary is also marked as a compaction summary
    When the Claude Code importer reads a user record marked "isCompactSummary" with the text "A summary of the work so far"
    Then the message is a compaction summary

  Scenario: A restart summary is recognised by its opening words
    The summary may arrive without the flag, so its fixed opening is
    recognised too.
    When the Claude Code importer reads a user record marked "none" with the text "This session is being continued from a previous conversation that ran out of context."
    Then the message has origin "harness"

  Scenario Outline: Text led by a harness tag is marked as injected
    When the Claude Code importer reads a user record marked "none" with the text "<text>"
    Then the message is a user message
    And the message has origin "harness"

    Examples:
      | text                                                          |
      | <task-notification>a task finished</task-notification>        |
      | <system-reminder>stay on task</system-reminder>               |
      | <wake>time to check</wake>                                    |
      | <project_claude_message>a note</project_claude_message>       |
      | <ci-monitor-event>checks passed</ci-monitor-event>            |
      | <relay>a relayed note</relay>                                 |
      | <artifact-view-context>the open page</artifact-view-context>  |
      | <create-pr-command>open a pull request</create-pr-command>    |
      | <command-name>/clear</command-name>                           |
      | <command-message>clear</command-message>                      |
      | <command-args>now</command-args>                              |
      | <local-command-stdout>done</local-command-stdout>             |
      | <local-command-caveat>Caveat: ignore this</local-command-caveat> |
      | <cross-session-message>hello from elsewhere</cross-session-message> |
      | Another Claude session sent a message                         |
      | [Request interrupted by user]                                 |
      | [Request interrupted by user for tool use]                    |

  Scenario: Leading blank space does not hide a harness tag
    When the Claude Code importer reads a user record marked "none" with the text "\n  <system-reminder>stay on task</system-reminder>"
    Then the message has origin "harness"
