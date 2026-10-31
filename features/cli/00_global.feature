Feature: The botspy command
  The CLI is a thin shell over the library: it owns flags, exit codes,
  and rendering — never business logic. Exit codes: 0 success (including
  empty results), 1 target not found, 2 usage error, 3 source failure.

  Scenario: The version is the crate version
    When I run "botspy --version"
    Then the exit code is 0
    And stdout reports the current crate version

  Scenario: A bad flag value is a usage error
    When I run "botspy sessions --limit abc"
    Then the exit code is 2
    And stderr mentions "--limit"

  Scenario: An unknown verb is a usage error
    When I run "botspy export"
    Then the exit code is 2
    And stderr mentions "export"

  Scenario: BOTSPY_HOME redirects the default roots
    Given a fixture home built from the synthetic corpus
    When I run "botspy sources --no-truncate" with BOTSPY_HOME at the fixture home
    Then the exit code is 0
    And stdout contains the full claude projects path
