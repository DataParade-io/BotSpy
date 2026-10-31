Feature: Parallel sessions timeline
  As someone running several coding agents at once, I want a picture of my
  parallel sessions over time, so that I can see how much work ran
  concurrently, where it overlapped, and how sessions fanned out into
  sub-agents. The timeline example reads sessions through the BotSpy public
  API and writes one self-contained local HTML page: a concurrency curve on
  top, project-grouped swimlanes below. Idle detection has no fixed cutoff:
  a gap is idle when it is an outlier among that session's own gaps (Tukey
  upper fence); a session with fewer than 4 gaps is one stretch; a session
  without message timestamps is one span marked approximate. Titles are off
  unless asked for, and message text never appears.
  The scenarios generate their sessions at run time and assert on the
  rendered page's structure, text and attributes.
  Evidence: BOTSPY-8739de (parallel sessions timeline epic).

  Scenario: Overlapping active stretches add up
    Given session "a" active from 09:00 to 10:00
    And session "b" active from 09:30 to 11:00
    When the timeline example renders
    Then the concurrency curve is 1 from 09:00, 2 from 09:30, 1 from 10:00 and 0 from 11:00
    And the page states a peak of 2 concurrent sessions at 09:30

  Scenario: Idle gaps do not count as active
    Given session "a" has an idle break from 09:20 to 09:50
    When the timeline example renders
    Then session "a" does not count toward concurrency between 09:20 and 09:50

  Scenario: Lanes are grouped by project and ordered by start
    Given sessions in projects "alpha" and "beta"
    When the timeline example renders
    Then each project has a labelled group of lanes
    And lanes within a group are ordered by start time

  Scenario: An unusually long gap splits a session into stretches
    Given a session whose messages are a few minutes apart except one gap far longer than the rest
    When the timeline example renders
    Then that session's lane shows two active stretches

  Scenario: A session without message timestamps is approximate
    Given a session with a start and last-activity time but no message timestamps
    When the timeline example renders
    Then its lane shows one span marked approximate

  Scenario: A spawned session nests under its parent
    Given session "p" spawned session "c"
    When the timeline example renders
    Then lane "c" is drawn indented under lane "p"
    And a connector marks the moment "c" was spawned

  Scenario: A child whose parent is outside the window still shows
    Given session "c" was spawned by a session outside the selected time window
    When the timeline example renders
    Then lane "c" is shown at top level with a note naming its missing parent

  Scenario: Titles are left out by default
    Given a session titled "Fix the billing export"
    When the timeline example renders without options
    Then the page does not contain "Fix the billing export"
    And lanes are labelled by agent and a short session id

  Scenario: Titles can be opted in
    Given a session titled "Fix the billing export"
    When the timeline example renders with "--titles"
    Then the page contains "Fix the billing export"

  Scenario: The page is self-contained
    When the timeline example renders
    Then the page loads no external scripts, styles, fonts or images
    And message text never appears on the page

  Scenario: A time window keeps only overlapping activity
    Given sessions active on 2026-10-01 and 2026-10-02
    When the timeline example renders with "--since 2026-10-02T00:00:00Z"
    Then only activity on 2026-10-02 is drawn

  Scenario: Project and source filters combine
    When the timeline example renders with "--project alpha --source claude_code"
    Then only Claude Code sessions in project "alpha" are drawn

  Scenario: Agents are distinguishable without colour
    Given sessions from Claude Code, Codex and Cursor
    When the timeline example renders
    Then each agent's bars carry a distinct pattern
    And a legend names each agent with its pattern

  Scenario: The page follows the reader's colour scheme
    When the timeline page is viewed with a dark colour scheme
    Then it uses its dark theme

  Scenario: The demo corpus shows parallel work
    When the timeline example renders with "--demo"
    Then the demo shows several projects with overlapping sessions
    And the demo shows idle breaks, a spawned sub-agent and an orphaned child
    And the demo contains no real paths, usernames or emails

  Scenario: The page is written to the requested path
    When I run the timeline example with "--demo" and an output path
    Then the timeline example exits with status 0
    And the output path is printed and holds the page

  Scenario: The example uses only the public API
    Then the timeline example imports only botspy and the standard library
