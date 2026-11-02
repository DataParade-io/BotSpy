Feature: Cursor sessions are grouped by their workspace, never by their title
  A Cursor IDE composer is a conversation, and its `name` is that
  conversation's title. A title is not a project: two conversations can
  share one, and one project holds many conversations. The importer
  therefore never derives a project from a title.
  The project comes from the workspace the composer was opened in. In the
  global store, the `composerHeaders` table gives each composer a
  `workspaceId`. That id is the name of a directory under the sibling
  `User/workspaceStorage/`, and the `workspace.json` in it names the folder:
  either a `folder` (a `file://` URI) or a `workspace` (the URI of a
  `.code-workspace` file). The importer reads that file read-only, once per
  workspace id for each source, percent-decodes the URI into a path, and
  sets the session's working directory (`metadata.cwd`) to it. For a
  `.code-workspace` file the working directory is the directory that holds
  the file. The project id is the workspace id: stable, and it does not name
  anything.
  A composer with no workspace id, a special one such as `empty-window`, a
  workspace id that has no directory, or a directory whose `workspace.json`
  names no local folder, falls under one neutral project,
  `cursor-no-workspace`, with no working directory. A store that keeps its
  headers in the older `composerHeaders` key has no workspace ids, so all of
  its composers fall there too.
  The title stays available as `metadata.title`, which the timeline draws
  only when it is asked to show titles. A composer that the headers mark as
  a sub-agent carries sub-agent information whose kind is its
  `subagentTypeName`, or `subagent` when it has none. The headers name no
  parent, so the parent stays unset.
  The store may be a symbolic link, as it is when it is linked into a data
  root: the workspace directory is found beside the real store.

  Scenario: A composer is grouped by its workspace folder
    Given a Cursor installation with the workspace "alpha" at the folder "file:///work/demo"
    And a composer "c1" titled "Quarterly layoff plan" in the workspace "alpha"
    When the Cursor importer reads the installation
    Then the composer "c1" has the working directory "/work/demo"
    And the composer "c1" has the project id of the workspace "alpha"

  Scenario: Two composers in the same workspace share a project
    Given a Cursor installation with the workspace "alpha" at the folder "file:///work/demo"
    And a Cursor installation with the workspace "beta" at the folder "file:///work/other"
    And a composer "c1" titled "First idea" in the workspace "alpha"
    And a composer "c2" titled "A different title" in the workspace "alpha"
    And a composer "c3" titled "First idea" in the workspace "beta"
    When the Cursor importer reads the installation
    Then the composers "c1" and "c2" share one project
    And the composers "c1" and "c3" are in different projects

  Scenario: A composer with no workspace falls under the neutral project
    Given a Cursor installation with the workspace "alpha" at the folder "file:///work/demo"
    And a composer "c1" titled "Has no workspace id" with no workspace
    And a composer "c2" titled "Opened in an empty window" in the workspace id "empty-window"
    And a composer "c3" titled "Directory is gone" in the workspace id "0123456789abcdef0123456789abcdef"
    And a composer "c4" titled "Workspace without a folder" in a workspace that names no folder
    When the Cursor importer reads the installation
    Then the composers "c1", "c2", "c3" and "c4" are in the project "cursor-no-workspace"
    And the composers "c1", "c2", "c3" and "c4" have no working directory

  Scenario: The older headers key has no workspace ids
    Given a Cursor KV store whose "composerHeaders" key lists a composer "c1" titled "Old layout"
    When the Cursor importer reads the installation
    Then the composer "c1" is in the project "cursor-no-workspace"

  Scenario: The title is never the project
    Given a Cursor installation with the workspace "alpha" at the folder "file:///work/demo"
    And a composer "c1" titled "Quarterly layoff plan" in the workspace "alpha"
    And a composer "c2" titled "Salary talks" with no workspace
    When the Cursor importer reads the installation
    Then no project id is a title
    And the composer "c1" keeps the title "Quarterly layoff plan" in its metadata

  Scenario: The timeline leaves titles out unless asked
    Given a Cursor installation with the workspace "alpha" at the folder "file:///work/demo"
    And a composer "c1" titled "Quarterly layoff plan" in the workspace "alpha"
    And a composer "c2" titled "Salary talks" with no workspace
    When the Cursor importer reads the installation
    And the timeline example renders the Cursor sessions
    Then the page does not contain "Quarterly layoff plan"
    And the page does not contain "Salary talks"
    And the timeline has a group of lanes for the project "demo"
    And the timeline has a group of lanes for the project "cursor-no-workspace"
    When the timeline example renders the Cursor sessions with "--titles"
    Then the page contains "Quarterly layoff plan"

  Scenario: A code-workspace file names the directory that holds it
    Given a Cursor installation with the workspace "alpha" in the code-workspace file "file:///work/demo/main.code-workspace"
    And a composer "c1" titled "Multi-root work" in the workspace "alpha"
    When the Cursor importer reads the installation
    Then the composer "c1" has the working directory "/work/demo"
    And the composer "c1" has the project id of the workspace "alpha"

  Scenario: A percent-encoded folder URI is decoded
    Given a Cursor installation with the workspace "alpha" at the folder "file:///work/my%20demo/caf%C3%A9"
    And a composer "c1" titled "Spaces and accents" in the workspace "alpha"
    When the Cursor importer reads the installation
    Then the composer "c1" has the working directory "/work/my demo/café"

  Scenario: A store reached through a symbolic link finds its workspaces
    Given a Cursor installation with the workspace "alpha" at the folder "file:///work/demo"
    And a composer "c1" titled "Linked store" in the workspace "alpha"
    And the store is reached through a symbolic link in another directory
    When the Cursor importer reads the installation
    Then the composer "c1" has the working directory "/work/demo"

  Scenario: Reading workspaces never changes them
    Given a Cursor installation with the workspace "alpha" at the folder "file:///work/demo"
    And a composer "c1" titled "Read only" in the workspace "alpha"
    When the Cursor importer reads the installation
    Then the workspace files were never mutated

  Scenario: A sub-agent composer carries its kind
    Given a Cursor installation with the workspace "alpha" at the folder "file:///work/demo"
    And a composer "c1" titled "Main conversation" in the workspace "alpha"
    And a composer "c2" titled "Looking around" in the workspace "alpha" that is a sub-agent of type "explore"
    And a composer "c3" titled "Untyped helper" in the workspace "alpha" that is a sub-agent without a type
    When the Cursor importer reads the installation
    Then the composer "c2" is a sub-agent of kind "explore" with no parent
    And the composer "c3" is a sub-agent of kind "subagent" with no parent
    And the composer "c1" is not a sub-agent
    And the composers "c1" and "c2" share one project

  Scenario: The timeline draws a sub-agent composer without a missing-parent note
    Given a Cursor installation with the workspace "alpha" at the folder "file:///work/demo"
    And a composer "c2" titled "Looking around" in the workspace "alpha" that is a sub-agent of type "explore"
    When the Cursor importer reads the installation
    And the timeline example renders the Cursor sessions
    Then the timeline draws the composer "c2" as a sub-agent lane
    And the page does not contain "which is not in this view"
