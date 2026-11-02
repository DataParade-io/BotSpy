Feature: Cursor importer reads one snapshot per store state
  The Cursor IDE store (`state.vscdb`) is a hot WAL database that can be
  several gigabytes, and copying it takes minutes. The importer reads it
  through a private snapshot copy, and reuses that one copy for every
  call (discovery, the doctor report, and the extraction of each composer)
  for as long as the store is unchanged. The store counts as changed when
  the size or modification time of the database or of its `-wal` file
  differs from when the copy was taken; nothing else, such as elapsed
  time, makes a copy stale. At most one copy exists at a time, and it is
  removed when it is replaced or when the source is dropped.

  Scenario: Discovering and extracting many composers takes one copy
    Given a Cursor KV store with 5 composers in "composerHeaders"
    When the Cursor importer discovers every composer and extracts each one
    Then discovery yields 5 sessions
    And the Cursor source has taken 1 snapshot copy

  Scenario: Reading again while the store is unchanged reuses the copy
    Given a Cursor KV store with 3 composers in "composerHeaders"
    When the Cursor importer discovers every composer and extracts each one
    And the Cursor importer detects changes again
    And the Cursor importer extracts composer "composer-1"
    Then the Cursor source has taken 1 snapshot copy

  Scenario: A changed store gets a fresh copy on the next call
    Given a Cursor KV store with 3 composers in "composerHeaders"
    When the Cursor importer extracts composer "composer-1"
    And the app adds a bubble to composer "composer-1"
    And the Cursor importer extracts composer "composer-1"
    Then the extraction holds 2 messages
    And the Cursor source has taken 2 snapshot copies
    And the first snapshot copy no longer exists and one copy remains

  Scenario: The copy is removed when the source is dropped
    Given a Cursor KV store with 3 composers in "composerHeaders"
    When the Cursor importer discovers every composer and extracts each one
    Then a snapshot copy exists
    When the Cursor source is dropped
    Then the snapshot copy has been removed
