Feature: Cursor messages the app wrote are not the human's
  Cursor puts text the human never typed under the user role in both
  places it keeps conversations. In the IDE store, a user bubble flagged
  `isSimulatedMsg` was written by Cursor itself, for example when it runs
  a plan. In the CLI transcripts, the human's words come wrapped in
  `<user_query>` beside the context the CLI attached (a timestamp, open
  files, skills), and some records hold only context.
  The importer marks the IDE bubbles as simulated. For the CLI it keeps
  the human's words as the message text, keeps the attached context apart
  from the text, and marks a record with no human words as injected.
  Evidence: IDE bubbles carry `isSimulatedMsg`; CLI user texts look like
  `<timestamp>…</timestamp><user_query>…</user_query>`, or hold only a
  timestamp, a tool catalogue, or a command list.

  Scenario: A typed IDE bubble has no origin
    When the Cursor importer reads a user bubble with the text "please add a test"
    Then the message is a user message
    And the message has no origin, so it is the human's

  Scenario: A simulated IDE bubble is marked
    When the Cursor importer reads a user bubble with the text "Implement the plan.", flagged simulated
    Then the message is a user message
    And the message has origin "simulated"

  Scenario: A CLI message keeps the human's words as its text
    When the Cursor CLI importer reads a user record with the text "<timestamp>Monday</timestamp>\n<user_query>\nplease add a test\n</user_query>"
    Then the message is a user message
    And the message has no origin, so it is the human's
    And the message text is "please add a test"

  Scenario: The context around a CLI query is kept apart from the text
    When the Cursor CLI importer reads a user record with the text "<timestamp>Monday</timestamp>\n<user_query>\nplease add a test\n</user_query>"
    Then the message keeps the surrounding context "<timestamp>Monday</timestamp>" apart from its text

  Scenario: Plain text in the older flat shape is the human's
    When the Cursor CLI importer reads a user record with the text "please add a test"
    Then the message has no origin, so it is the human's
    And the message text is "please add a test"

  Scenario Outline: A CLI record with no human words is marked
    When the Cursor CLI importer reads a user record with the text "<text>"
    Then the message is a user message
    And the message has origin "harness"

    Examples:
      | text                                                       |
      | <timestamp>Monday</timestamp>                              |
      | <dynamic_tools><name>search</name></dynamic_tools>         |
      | <available_subagent_types>explore</available_subagent_types> |
      | <cursor_commands>list of commands</cursor_commands>        |
