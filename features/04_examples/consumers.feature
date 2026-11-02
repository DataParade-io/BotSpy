Feature: Example consumers of the unified history
  The example consumers — a coding-session thanks-vs-F-bombs meter and a
  local sentiment analysis — read BotSpy sessions and score the
  conversation. The meter scores only the messages you typed, never the
  agent's, and draws them as a weekly diverging bar chart (thanks up,
  F-bombs down) in the same figure family as the timeline. They are examples, not core:
  they live in `examples/` (or an optional `botspy[metrics]` extra), never
  in the dependency graph, and only consume the public API.
  Evidence: BOTSPY-ab18a2 (example use cases epic).

  # ---- The meter scores only you ------------------------------------

  Scenario: The meter counts thanks and F-bombs in your messages per session
    Given a session with messages "thanks, that fixed it", "what the f*** is this", and "thank you, works now"
    When the meter scores the session
    Then it reports 2 thanks and 1 F-bomb
    And the ratio favors thanks

  Scenario: The meter aggregates your messages across sessions
    Given sessions "m1" with 1 thanks and "m2" with 2 F-bombs
    When the meter scores the unified history
    Then it reports 1 thanks and 2 F-bombs across 2 sessions

  Scenario: Agent replies that say thanks do not count
    The meter scores your tone, not the agent's. A gracious reply is not
    gratitude from you.
    Given a session where I typed "please fix the build" and the agent replied "Thanks for the details, thank you for waiting"
    When the meter scores the session
    Then it reports 0 thanks and 0 F-bombs

  Scenario: Only the user role is scored
    Given a session where I typed "thanks, that fixed it" and the agent replied "what the f*** happened here"
    When the meter scores the session
    Then it reports 1 thanks and 0 F-bombs

  Scenario: Tool results stored under the user role do not count
    Adapters store tool results under the user role, but as tool parts and
    not as text, so no classifier ever sees them.
    Given a session where a tool result "thanks, what the f*** is this" came back under the user role
    When the meter scores the session
    Then it reports 0 thanks and 0 F-bombs

  Scenario: Text injected by tooling does not count
    Adapters mark user-role text that tooling injected (Cursor's simulated
    user turns, inter-agent mail) and agent-written compaction summaries.
    None of it is something you typed.
    Given a session where tooling injected the user-role text "thanks, carry on"
    And the same session holds the compaction summary "thanks, summary of the chat" stored under the user role
    When the meter scores the session
    Then it reports 0 thanks and 0 F-bombs

  Scenario Outline: Claude Code text the harness injected does not count
    Claude Code stores skill bodies, summaries, notifications, hook output
    and mail from other sessions under the user role. The importer marks
    each, and the meter skips them, even when they carry thanks and F-bombs.
    Given a Claude Code session where I typed "thanks, that fixed it" and the harness injected a record marked "<mark>" led by "<lead>"
    When the meter scores the session
    Then it reports 1 thanks and 0 F-bombs

    Examples:
      | mark                     | lead                          |
      | isMeta                   | Base directory for this skill |
      | isCompactSummary         | A summary of the work         |
      | origin:task-notification | A task finished               |
      | origin:peer              | A note from another session   |
      | none                     | <system-reminder>             |
      | none                     | <task-notification>           |
      | none                     | <wake>                        |
      | none                     | <command-name>                |
      | none                     | <local-command-stdout>        |
      | none                     | <cross-session-message>       |
      | none                     | This session is being continued from a previous conversation |
      | none                     | [Request interrupted by user] |

  Scenario Outline: Text pasted into Claude Code still counts
    Pasted text and shell input are the human's own input.
    Given a Claude Code session where I typed "thanks, that fixed it" and the harness injected a record marked "none" led by "<lead>"
    When the meter scores the session
    Then it reports 2 thanks and 1 F-bombs

    Examples:
      | lead               |
      | <pasted_content>   |
      | <bash-input>       |

  Scenario: Codex environment and instructions blocks do not count
    Given a Codex session where I typed "thanks, that fixed it" and the harness injected the user message led by "<environment_context>"
    When the meter scores the session
    Then it reports 1 thanks and 0 F-bombs

  Scenario: Codex AGENTS.md instructions do not count
    Given a Codex session where I typed "thanks, that fixed it" and the harness injected the user message led by "# AGENTS.md instructions for /repo"
    When the meter scores the session
    Then it reports 1 thanks and 0 F-bombs

  Scenario: Cursor CLI context records do not count, and the typed query does
    Given a Cursor CLI session where I typed "thanks, that fixed it" and the harness injected the user record led by "<dynamic_tools>"
    When the meter scores the session
    Then it reports 1 thanks and 0 F-bombs

  Scenario: Cursor IDE simulated bubbles do not count
    Given a Cursor session where I typed "thanks, that fixed it" and Cursor simulated a user bubble carrying thanks and an F-bomb
    When the meter scores the session
    Then it reports 1 thanks and 0 F-bombs

  Scenario: A sub-agent's prompts do not count
    The "user" messages of a sub-agent are prompts the parent agent wrote.
    Given a sub-agent session where the parent agent prompted "thanks, now check the build"
    When the meter scores the session
    Then it reports 0 thanks and 0 F-bombs

  Scenario: A message with several text parts counts once
    Given a message of mine with the text parts "thanks a lot" and "thank you again"
    When the meter scores the session
    Then it reports 1 thanks and 0 F-bombs

  Scenario: The text report says it counts your messages
    When I run the meter example with "--demo"
    Then the meter example exits with status 0
    And the meter report reads "demo-session-01: 14 thanks, 3 F-bombs in your messages"
    And the meter report reads "Meter: 421 thanks vs 31 F-bombs in your messages across 13 sessions"

  # ---- The weekly chart ---------------------------------------------

  Scenario: Messages are bucketed into weeks that start on Monday, UTC
    A Sunday-night thanks and a Monday-morning thanks land in different weeks.
    Given I typed "thanks" at 2026-10-04T23:59:00Z
    And I typed "thanks" at 2026-10-05T00:00:00Z
    When the meter figure is drawn
    Then the figure has 2 weeks
    And the week "2026-09-28" has 1 thanks and 0 F-bombs
    And the week "2026-10-05" has 1 thanks and 0 F-bombs

  Scenario: Weeks with nothing to count between the first and the last are kept
    Given I typed "thanks" at 2026-10-05T10:00:00Z
    And I typed the F-bomb at 2026-10-21T10:00:00Z
    When the meter figure is drawn
    Then the figure has 3 weeks
    And the week "2026-10-12" has 0 thanks and 0 F-bombs
    And the week "2026-10-19" has 0 thanks and 1 F-bomb

  Scenario: The chart scores only your messages
    Given I typed "thanks" at 2026-10-05T10:00:00Z
    And the agent said "thanks, thank you" at 2026-10-05T10:01:00Z
    And a tool result came back under the user role at 2026-10-05T10:02:00Z
    When the meter figure is drawn
    Then the week "2026-10-05" has 1 thanks and 0 F-bombs

  Scenario: Messages without a timestamp are left out of the chart
    Given I typed "thanks" at 2026-10-05T10:00:00Z
    And I typed "thanks" with no timestamp
    When the meter figure is drawn
    Then the week "2026-10-05" has 1 thanks and 0 F-bombs

  Scenario: A time window keeps only the messages inside it
    Given I typed "thanks" at 2026-10-05T10:00:00Z
    And I typed "thanks" at 2026-10-19T10:00:00Z
    When the meter figure is drawn with "--since 2026-10-12 --until 2026-10-25"
    Then the figure has 2 weeks
    And the week "2026-10-12" has 0 thanks and 0 F-bombs
    And the week "2026-10-19" has 1 thanks and 0 F-bombs

  Scenario: The x axis shows the week-start dates
    Given I typed "thanks" at 2026-10-05T10:00:00Z
    And I typed "thanks" at 2026-10-13T10:00:00Z
    When the meter figure is drawn
    Then the x axis shows the dates "2026-10-05" and "2026-10-12"
    And the x axis says weeks start on Monday, UTC

  Scenario: Thanks bars go up from the zero axis and F-bomb bars go down
    Given I typed 3 thanks and 1 F-bomb at 2026-10-05T10:00:00Z
    And I typed 2 thanks and 2 F-bombs at 2026-10-12T10:00:00Z
    When the meter figure is drawn
    Then every thanks bar stands on the zero axis and every F-bomb bar hangs from it
    And a bar's height is the same fraction of its count for thanks and for F-bombs

  Scenario: Each bar is labelled with its count where there is room
    Given I typed 3 thanks and 1 F-bomb at 2026-10-05T10:00:00Z
    When the meter figure is drawn
    Then the thanks bar of week "2026-10-05" is labelled "3"
    And the F-bomb bar of week "2026-10-05" is labelled "1"

  Scenario: A bar with no room for a label goes without one
    Given I typed "thanks" in each of 200 weeks from 2020-01-06
    When the meter figure is drawn
    Then the figure has 200 weeks
    And no bar is labelled

  Scenario: The legend tells thanks and F-bombs apart by pattern, not colour alone
    Given I typed 3 thanks and 1 F-bomb at 2026-10-05T10:00:00Z
    When the meter figure is drawn
    Then the legend names "Thanks" and "F-bombs"
    And the thanks swatch is a solid fill
    And the F-bomb swatch is a diagonal hatch
    And the bars use the same fills as their swatches

  Scenario: The title block gives the totals and the ratio
    Given I typed 44 thanks and 2 F-bombs at 2026-10-05T10:00:00Z
    When the meter figure is drawn
    Then the figure's title is "Thanks vs F-bombs"
    And the headline reads "44 thanks, 2 F-bombs: 22 thanks for every F-bomb"
    And the muted sentence reads "Counts only the messages you typed, and message text is never shown."

  Scenario: Totals of a thousand or more are grouped
    Given I typed 1234 thanks and 56 F-bombs at 2026-10-05T10:00:00Z
    When the meter figure is drawn
    Then the headline reads "1,234 thanks, 56 F-bombs: 22 thanks for every F-bomb"

  Scenario: With zero F-bombs the headline says "no F-bombs" and divides by nothing
    Given I typed 9 thanks and 0 F-bombs at 2026-10-05T10:00:00Z
    When the meter figure is drawn
    Then the headline reads "9 thanks, no F-bombs"
    And no bar hangs below the zero axis

  Scenario: A single F-bomb is singular
    Given I typed 3 thanks and 1 F-bomb at 2026-10-05T10:00:00Z
    When the meter figure is drawn
    Then the headline reads "3 thanks, 1 F-bomb: 3 thanks for every F-bomb"

  Scenario: More F-bombs than thanks flips the ratio the other way
    Given I typed 2 thanks and 6 F-bombs at 2026-10-05T10:00:00Z
    When the meter figure is drawn
    Then the headline reads "2 thanks, 6 F-bombs: 3 F-bombs for every thanks"

  Scenario: Nothing to count draws an empty chart without dividing by zero
    Given I typed "please run the tests" at 2026-10-05T10:00:00Z
    When the meter figure is drawn
    Then the headline reads "No thanks or F-bombs in the selected window"
    And the figure has 0 weeks

  Scenario: The figure's viewBox is the content plus an even padding
    When the meter figure is drawn with "--demo"
    Then the meter viewBox is the content bounds plus 16 px on every side
    And no element extends past the meter viewBox
    And the meter padding is even: the content touches the edges 16 px in on every side

  Scenario: A chart with a single week is just as tight
    Given I typed 3 thanks and 1 F-bomb at 2026-10-05T10:00:00Z
    When the meter figure is drawn
    Then the meter viewBox is the content bounds plus 16 px on every side
    And no element extends past the meter viewBox
    And the meter padding is even: the content touches the edges 16 px in on every side

  Scenario: The figure is light by default
    When the meter figure is drawn with "--demo"
    Then the meter figure uses the light colours with no variables or media queries

  Scenario: The dark theme gives a dark figure
    When the meter figure is drawn with "--demo --theme dark"
    Then the meter figure uses the dark colours with no variables or media queries

  Scenario: Both themes keep the pattern difference
    When the meter figure is drawn with "--demo --theme dark"
    Then the thanks swatch is a solid fill
    And the F-bomb swatch is a diagonal hatch

  Scenario: The PNG is the viewBox size at the default scale of 2
    When I run the meter example with "--demo" writing a png file
    Then the meter example exits with status 0
    And the meter output path is printed
    And the meter PNG is 2 times the size of the viewBox

  Scenario: The PNG scale can be changed
    When I run the meter example with "--demo --scale 3" writing a png file
    Then the meter PNG is 3 times the size of the viewBox

  Scenario: The PNG follows the light theme
    When I run the meter example with "--demo --theme light" writing a png file
    Then the meter PNG's background is the light background

  Scenario: The PNG follows the dark theme
    When I run the meter example with "--demo --theme dark" writing a png file
    Then the meter PNG's background is the dark background

  Scenario: The standalone SVG is written to the requested path
    When I run the meter example with "--demo" writing a svg file
    Then the meter example exits with status 0
    And the meter output path is printed
    And the file is a standalone meter SVG sized to its viewBox

  Scenario: An unknown theme or a bad scale is refused
    When I run the meter example with "--demo --theme blue"
    Then the meter example exits with status 2
    And the meter example reports an error naming "--theme"
    When I run the meter example with "--demo --scale 0"
    Then the meter example exits with status 2
    And the meter example reports an error naming "--scale"

  Scenario: The figure holds no message text, session titles or ids
    Given a session "secret-session-id" titled "secret session title" where I typed "thanks, never-show-this-message-text" at 2026-10-05T10:00:00Z
    When the meter figure is drawn
    Then the week "2026-10-05" has 1 thanks and 0 F-bombs
    And the figure holds no message text, session title or session id

  Scenario: The demo corpus shows the weekly chart
    When the meter figure is drawn with "--demo"
    Then the figure has 12 weeks
    And the headline reads "421 thanks, 31 F-bombs: 14 thanks for every F-bomb"
    And the figure holds no message text, session title or session id

  Scenario: Sentiment runs locally over message text
    Given a session with messages "I love this fix", "this is terrible", and "works fine"
    When the sentiment example scores the session
    Then it reports 1 positive, 1 negative, and 1 neutral message
    And no message text leaves the machine

  Scenario: Examples depend only on the public API
    Given the unified history with 2 fixture sessions
    When an example consumer is built against the published crate
    Then it uses only the public API surface
    And it adds no dependency to the core library