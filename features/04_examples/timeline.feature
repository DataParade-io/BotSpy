Feature: Parallel sessions timeline
  As someone running several coding agents at once, I want a picture of my
  parallel sessions over time, so that I can see how much work ran
  concurrently, where it overlapped, and how sessions fanned out into
  sub-agents. The timeline example reads sessions through the BotSpy public
  API and writes one self-contained local HTML page: a concurrency curve on
  top, project-grouped swimlanes below. Activity is defined by what the agent
  was doing, with no thresholds: a bar is a turn, from a human message to the
  last non-human message (assistant, tool, system) before the next human
  message. The time from there to the next human message is the agent
  waiting for you, drawn as a dotted line and not counted as working. Turns
  never have zero length, so a trailing human message with no reply is
  dropped. A session without message timestamps is one span marked
  approximate.
  The whole figure (title, peak line, short description, legend, concurrency
  chart, lanes and axis) is one self-contained SVG whose viewBox is exactly
  the content plus an even 16 px padding, so images for articles have no
  blank margins. The HTML page embeds that same figure, and the example can
  also write it as a standalone SVG or rasterize it to a PNG.
  The scenarios generate their sessions at run time and assert on the
  rendered page's structure, text and attributes.
  Evidence: BOTSPY-8739de (parallel sessions timeline epic).

  Scenario: Overlapping turns add up to agents working at once
    Given session "a" active from 09:00 to 10:00
    And session "b" active from 09:30 to 11:00
    When the timeline example renders
    Then the concurrency curve is 1 from 09:00, 2 from 09:30, 1 from 10:00 and 0 from 11:00
    And the page states a peak of 2 agents working at once at 09:30

  Scenario: Waiting for me does not count as working
    Given session "a" is waiting for me from 09:20 to 09:50
    When the timeline example renders
    Then session "a" does not count as working between 09:20 and 09:50
    And the lane shows a waiting interval from 09:20 to 09:50

  Scenario: Lanes are grouped by project and ordered by start
    Given sessions in projects "alpha" and "beta"
    When the timeline example renders
    Then each project has a labelled group of lanes
    And lanes within a group are ordered by start time

  Scenario: Each turn is one stretch and the time between turns is waiting
    Given a session with three turns separated by waiting
    When the timeline example renders
    Then that session's lane shows 3 active stretches and 2 waiting intervals
    And each stretch holds its own turn's messages

  Scenario: Consecutive human messages extend the turn's start
    Given a session where I wrote twice before the agent replied
    When the timeline example renders
    Then that session's lane shows 1 active stretch and 0 waiting intervals
    And the stretch runs from 09:00 to 09:20

  Scenario: A trailing human message with no reply is dropped
    Given a session whose last message is mine with no reply
    When the timeline example renders
    Then that session's lane shows 1 active stretch and 0 waiting intervals
    And the stretch runs from 09:00 to 09:10

  Scenario: Tool results coming back do not end the agent's turn
    Given a session where tool results come back during a turn
    When the timeline example renders
    Then that session's lane shows 1 active stretch and 0 waiting intervals
    And the stretch runs from 09:00 to 09:10

  Scenario: A session without message timestamps is approximate
    Given a session with a start and last-activity time but no message timestamps
    When the timeline example renders
    Then its lane shows one span marked approximate

  Scenario: A spawned session nests under its parent
    Given session "p" spawned session "c"
    When the timeline example renders
    Then lane "c" is drawn indented under lane "p"
    And a connector marks the moment "c" was spawned

  Scenario: A Claude Code sub-agent nests under the session that spawned it
    Given a Claude Code session "p" that spawned a sub-agent "agent-1"
    When the timeline example renders
    Then lane "p--agent-1" is drawn indented under lane "p"
    And a connector joins "p" to "p--agent-1" when the sub-agent began
    And the lane "p--agent-1" shows one active stretch

  Scenario: A child whose parent is outside the window still shows
    Given session "c" was spawned by a session outside the selected time window
    When the timeline example renders
    Then lane "c" is shown at top level with a note naming its missing parent

  Scenario: A project is labelled by its repository, never by the home directory
    Given a Claude Code session with an encoded project id and a working directory
    When the timeline example renders
    Then the project is labelled by the repository name
    And the page never contains the home-directory name

  Scenario: An encoded project id without a working directory is cut down
    Given a Claude Code session with an encoded project id and no working directory
    When the timeline example renders
    Then the project is labelled by what follows its "-Projects-" component
    And the page never contains the home-directory name

  Scenario: An encoded project id with no leading dash is cut down too
    Given a Cursor session with an unresolved encoded project id that has no leading dash
    When the timeline example renders
    Then the project is labelled "Widget-Factory"
    And the page never contains the home-directory name

  Scenario: A home directory is never shown as a project
    Given a session whose working directory is the home directory
    When the timeline example renders
    Then the project is labelled with a neutral "project-" name
    And the page never contains the home-directory name

  Scenario: A UUID project id is not shown
    Given a session whose project id is a UUID and that has no working directory
    When the timeline example renders
    Then the project is labelled with a neutral "project-" name
    And the page never contains the UUID

  Scenario: The same folder seen through three agents is one project
    Given the folder "Chattic.us" seen through Claude Code, Cursor and the Cursor CLI, each with its own project id
    When the timeline example renders
    Then the page has exactly 1 project group
    And that group is labelled "Chattic.us" and holds 3 lanes

  Scenario: The project filter also matches the folder name
    Given the folder "Chattic.us" seen through Claude Code, Cursor and the Cursor CLI, each with its own project id
    When the timeline example renders with "--project chattic.us"
    Then the page has exactly 1 project group
    And that group is labelled "Chattic.us" and holds 3 lanes

  Scenario: Long project names are ellipsized with the full name on hover
    Given a session in a project with a very long name
    When the timeline example renders
    Then the project label is ellipsized and its tooltip holds the full name
    And the project's session count is still shown

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

  Scenario: The figure's viewBox is the content plus an even padding
    When the timeline figure is drawn with "--demo"
    Then the viewBox is the content bounds plus the same padding on every side
    And no element extends past the viewBox
    And the padding is even: the content touches the figure's edges 16 px in on every side

  Scenario: A figure with a single session is just as tight
    Given session "a" active from 09:00 to 10:00
    When the timeline figure is drawn
    Then the viewBox is the content bounds plus the same padding on every side
    And no element extends past the viewBox
    And the padding is even: the content touches the figure's edges 16 px in on every side

  Scenario: The HTML page embeds the one figure and puts no cap on its width
    When the timeline example renders with "--demo"
    Then the page embeds the same figure with the title, legend and axis inside it
    And the page sets no maximum width on the figure

  Scenario: The PNG is the viewBox size at the default scale of 2
    When I run the timeline example with "--demo" writing a png file
    Then the timeline example exits with status 0
    And the output path is printed
    And the PNG is 2 times the size of the viewBox

  Scenario: The PNG scale can be changed
    When I run the timeline example with "--demo --scale 3" writing a png file
    Then the PNG is 3 times the size of the viewBox

  Scenario: The standalone SVG is written to the requested path
    When I run the timeline example with "--demo" writing a svg file
    Then the timeline example exits with status 0
    And the output path is printed
    And the file is a standalone SVG sized to its viewBox

  Scenario: The figure is light by default
    When the timeline figure is drawn with "--demo"
    Then the figure uses the light colours with no variables or media queries

  Scenario: The dark theme gives a dark figure
    When the timeline figure is drawn with "--demo --theme dark"
    Then the figure uses the dark colours with no variables or media queries

  Scenario: The PNG follows the light theme
    When I run the timeline example with "--demo --theme light" writing a png file
    Then the PNG's background is the light background

  Scenario: The PNG follows the dark theme
    When I run the timeline example with "--demo --theme dark" writing a png file
    Then the PNG's background is the dark background

  Scenario: An unknown theme or a bad scale is refused
    When I run the timeline example with "--demo --theme blue"
    Then the timeline example exits with status 2
    And the timeline example reports an error naming "--theme"
    When I run the timeline example with "--demo --scale 0"
    Then the timeline example exits with status 2
    And the timeline example reports an error naming "--scale"

  Scenario: A sub-agent lane is labelled by its kind and its own short id
    Given a sub-agent session "parent--agent-a3f9c2e1b7" of kind "explore" under session "parent"
    When the timeline example renders
    Then the lane "parent--agent-a3f9c2e1b7" is labelled "↳ Explore a3f9c2"
    And the lane "parent" is labelled "Claude Code parent"
    And the tooltip of lane "parent--agent-a3f9c2e1b7" holds its full session id and kind

  Scenario: Session ids can be left out of a figure meant for publishing
    Short session ids help when you are reading your own history, but they
    are identifiers, and a figure for an article does not need them. With
    "--no-ids" a lane names only its agent or sub-agent kind, and every id
    in the page is replaced by a pseudonym so none survives in the markup.
    Given a sub-agent session "c0ffee01--agent-a3f9c2e1b7" of kind "explore" under session "c0ffee01"
    When the timeline example renders with "--no-ids"
    Then the lane "session-1" is labelled "Claude Code"
    And the lane "session-2" is labelled "↳ Explore"
    And the page does not contain "a3f9c2"
    And the page does not contain "c0ffee01"

  Scenario: A long sub-agent kind is cut so the sub-agent's own id still shows
    Sibling sub-agents often share a long kind such as "general-purpose".
    Cutting the kind rather than the id keeps their lanes telling apart.
    Given a sub-agent session "parent--agent-a3f9c2e1b7" of kind "security-review-specialist" under session "parent"
    When the timeline example renders
    Then the lane "parent--agent-a3f9c2e1b7" is labelled "↳ Security-r… a3f9c2"

  Scenario: The demo corpus shows parallel work
    When the timeline example renders with "--demo"
    Then the demo shows several projects with overlapping sessions
    And the demo shows waiting for you, a spawned sub-agent and an orphaned child
    And the demo contains no real paths, usernames or emails

  Scenario: The page is written to the requested path
    When I run the timeline example with "--demo" and an output path
    Then the timeline example exits with status 0
    And the output path is printed and holds the page

  Scenario: The example uses only the public API
    Then the timeline example imports only botspy and the standard library, plus resvg for the PNG
