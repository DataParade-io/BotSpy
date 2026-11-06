Feature: Audit coding sessions for sensitive information
  As a DataParade user
  I want to audit selected sessions with LeftBlock
  So that I can locate findings without exporting the transcripts

  Scenario: Restrict the audit to a project and time window
    Given two local projects with synthetic coding sessions
    When I audit the first project since the start of October
    Then the audit has one finding from the first project
    And the audit contains no transcript text

  Scenario: Keep findings when Privacy Filter is unavailable
    Given two local projects with synthetic coding sessions
    When I audit the second session by its exact ID
    Then the audit has one finding from the second project
    And the audit reports that Privacy Filter is unavailable

  Scenario: Report an unreadable selected source
    Given two local projects with synthetic coding sessions
    When I audit a selected source that cannot be read
    Then the audit exits with a source error
