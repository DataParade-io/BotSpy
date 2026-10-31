Feature: Privacy controls for session transcripts
  Agent histories routinely hold pasted keys, .env contents, database URLs
  and personal data. The privacy example reads sessions through BotSpy,
  streams each text-bearing part to a local `leftblock scan --jsonl`
  process, and either audits (counts and provenance, never values) or
  redacts, after which the meter and sentiment examples score only the
  redacted text. The committed fixtures stay free of fake secrets: the
  scenarios plant fake values at run time into a temp agent-data root.
  A stub scanner that speaks the scan contract stands in for LeftBlock,
  because CI has none; one check runs the real thing where it is installed.
  Evidence: BOTSPY-ab18a2 (example use cases epic).

  Scenario: The audit counts findings by rule and kind per session
    Given a planted corpus of fake secrets and personal data
    And a stub LeftBlock scanner
    When I run the privacy example with "audit"
    Then it exits with status 0
    And output contains "3 findings"
    And output contains "aws-access-token x1, database-url x1, github-pat x1"
    And output contains "by kind: secret x3"
    And output contains "Totals: 3 findings in 1 of 1 sessions"

  Scenario: The audit traces each finding back to its source
    Given a planted corpus of fake secrets and personal data
    And a stub LeftBlock scanner
    When I run the privacy example with "audit"
    Then output contains "ClaudeCode 00000000-0000-4000-8000-0000000000aa"
    And output contains "aws-access-token [secret] message 0 part 0, source {transcript}:"
    And output contains "database-url [secret] message 2 part 0, source {transcript}:"

  Scenario: The audit never prints matched values
    Given a planted corpus of fake secrets and personal data
    And a stub LeftBlock scanner
    When I run the privacy example with "audit --deep"
    Then output never contains a planted value
    And error output is empty

  Scenario: Deep scanning passes through to LeftBlock
    Given a planted corpus of fake secrets and personal data
    And a stub LeftBlock scanner
    When I run the privacy example with "audit --deep"
    Then the scanner was invoked with "scan --jsonl --deep"
    And output contains "privacy-filter-private_email x1"
    And output contains "by kind: deep x1, secret x3"

  Scenario: Records are keyed by opaque indexes
    Given a planted corpus of fake secrets and personal data
    And a stub LeftBlock scanner
    When I run the privacy example with "audit"
    Then the scanner was invoked with "scan --jsonl"
    And the scanner received only record ids shaped like s0/m0/p0

  Scenario: Redaction replaces exactly the reported byte ranges
    Given a corpus with the message "thanks, key {aws-key} is for staging"
    And a stub LeftBlock scanner
    When I run the privacy example with "redact"
    Then it exits with status 0
    And output contains "thanks, key [REDACTED:aws-access-token] is for staging"
    And output never contains a planted value

  Scenario: Redaction slices by UTF-8 bytes in multi-byte text
    Given a corpus with the message "café ☕ key {aws-key} ñandú"
    And a stub LeftBlock scanner
    When I run the privacy example with "redact"
    Then output contains "café ☕ key [REDACTED:aws-access-token] ñandú"

  Scenario: Overlapping findings merge into one replacement
    Given a corpus with the message "see {{vault|hunter2}}! ok"
    And a stub LeftBlock scanner
    When I run the privacy example with "redact"
    Then output contains "see [REDACTED:vault] ok"
    And output does not contain "REDACTED:vault][REDACTED"

  Scenario: A finding that does not fit its record is refused
    Given a corpus with the message "short"
    And a stub LeftBlock scanner that reports a range past the end
    When I run the privacy example with "redact"
    Then it exits with status 2
    And error output is one line mentioning "byte range"
    And output does not contain "short"

  Scenario: Meter and sentiment see only the redacted text
    Given a corpus with the messages "thanks for the help [[vault|what the fuck]]" and "works fine [[vault|terrible]]"
    And a stub LeftBlock scanner
    When I run the privacy example with "redact"
    Then output contains "Meter: 1 thanks vs 0 F-bombs across 1 sessions"
    And output contains "Sentiment: 1 positive, 0 negative, 1 neutral messages"
    And scoring the unredacted corpus would have counted 1 F-bomb and 1 negative message

  Scenario: The demo corpus plants its own fake secrets and cleans up
    Given a stub LeftBlock scanner
    When I run the privacy example with "audit --plant"
    Then it exits with status 0
    And output contains "Totals: 3 findings in 1 of 1 sessions"
    And the planted demo corpus is gone

  Scenario: A missing LeftBlock exits 2 with an install hint
    Given a planted corpus of fake secrets and personal data
    And no LeftBlock executable
    When I run the privacy example with "audit"
    Then it exits with status 2
    And error output is one line mentioning "pipx install leftblock"
    And output is empty

  Scenario: A LeftBlock error exits 2
    Given a planted corpus of fake secrets and personal data
    And a stub LeftBlock scanner that fails
    When I run the privacy example with "redact"
    Then it exits with status 2
    And error output is one line mentioning "simulated scanner failure"
    And output is empty

  Scenario: Thousands of records stream without deadlock
    Given a corpus of 4000 messages of 2000 bytes each carrying one planted key
    And a stub LeftBlock scanner
    When I run the privacy example with "audit"
    Then it finishes within 120 seconds with status 0
    And output contains "Totals: 4000 findings in 1 of 1 sessions"

  @real-leftblock
  Scenario: The real LeftBlock finds the planted secrets
    Given a planted corpus of fake secrets and personal data
    When I run the privacy example with "audit" against the real LeftBlock
    Then it exits with status 0
    And output reports at least 1 finding
    And output never contains a planted value
