Feature: Read-only sources and safe SQLite/WAL snapshotting
  Adapters never write to the sources they tap. SQLite sources running in
  WAL mode — a live app writing while we read — are read through a safe
  snapshot copy, never in place, and never interfere with the writer.
  Evidence: BOTSPY-296d2c; Cursor's store is a hot WAL database.

  Scenario: A SQLite/WAL source is read through a safe snapshot copy
    Given a SQLite source in WAL mode at "roots/demo/store.db" with 5 rows and a live writer appending rows
    When I read the source through a snapshot copy
    Then the snapshot yields a consistent read of the rows present at snapshot time
    And the source database file and its WAL file were never mutated
    And the live writer kept appending without interference

  Scenario: A snapshot destination cannot replace its source
    Given a SQLite source named "snapshot.db" containing a synthetic event
    When I request a snapshot in the source directory
    Then the snapshot request is rejected
    And the source database file and its WAL file were never mutated

  Scenario: A failed snapshot preserves the previous successful output
    Given an invalid SQLite source and an existing snapshot containing a synthetic event
    When I request a snapshot beside the invalid source
    Then the snapshot request is rejected
    And the previous snapshot is unchanged

  Scenario: A snapshot cannot replace a destination with SQLite sidecars
    Given a SQLite source and an existing snapshot with live WAL data
    When I request a snapshot beside that live destination
    Then the snapshot request is rejected
    And the previous snapshot is unchanged

  Scenario: Automatic snapshot copies are removed after inspecting sources
    Given a fixture home built from the synthetic corpus
    When I inspect the sources using an isolated temporary directory
    Then the exit code is 0
    And no automatic snapshot copies remain in the temporary directory

  # Automatic copies use unpredictable directories with access limited to
  # their owner on Unix. The directory lives only as long as its SQLite
  # reader and is also removed when opening or querying the copy fails.
  # A killed process can leave a private copy for the OS or user to remove.
