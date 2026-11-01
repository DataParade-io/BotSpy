Feature: Codex messages the harness wrote are not the human's
  Codex adds its own blocks to the conversation under the user role: the
  environment snapshot, the project's `AGENTS.md` instructions, browser
  and app context. The importer keeps them as user messages and marks
  them with the harness origin, so a reader that wants only the human's
  words can leave them out. What the human typed has no origin.
  Evidence: rollout `response_item` messages with role `user` whose text
  opens with `<environment_context>`, `<user_instructions>`,
  `<INSTRUCTIONS>`, `<codex_internal_context>`,
  `<in-app-browser-context>`, `<external_codex_apps_open_page>`, or
  `# AGENTS.md instructions for <directory>`.

  Scenario: A typed message has no origin
    When the Codex importer reads a user message with the text "please run the tests"
    Then the message is a user message
    And the message has no origin, so it is the human's

  Scenario Outline: A block the harness injected is marked
    When the Codex importer reads a user message with the text "<text>"
    Then the message is a user message
    And the message has origin "harness"

    Examples:
      | text                                                                |
      | <environment_context>\n  <cwd>/repo</cwd>\n</environment_context>   |
      | <user_instructions>always answer briefly</user_instructions>        |
      | # AGENTS.md instructions for /repo\n\n<INSTRUCTIONS>be brief</INSTRUCTIONS> |
      | <codex_internal_context>internal notes</codex_internal_context>     |
      | <in-app-browser-context>the open tab</in-app-browser-context>       |
      | <external_codex_apps_open_page>a page</external_codex_apps_open_page> |

  Scenario: A typed message that mentions a tag is still the human's
    Only a block that opens the text is injected.
    When the Codex importer reads a user message with the text "why does <environment_context> appear in my log?"
    Then the message has no origin, so it is the human's
