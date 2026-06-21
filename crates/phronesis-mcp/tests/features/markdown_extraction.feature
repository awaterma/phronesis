Feature: Markdown Rules Extraction
  As a developer configuring phr-mcp
  I want to extract enforceable rules from markdown documents
  So that coding standards and patterns can be automatically enforced

  Scenario: Extract rules from directive prefixes
    Given a markdown document containing
      """
      # Coding Standards

      - Avoid using .unwrap() in production code paths
      - Never commit secrets or credentials to the repository
      - Prefer Result over panic for error handling in libraries
      """
    When I extract rules from the document "docs/standards.md"
    Then 3 rules should be extracted
    And rule "standards-1" should have action text containing "using .unwrap()"
    And rule "standards-2" should have action text containing "commit secrets"
    And rule "standards-3" should have action text containing "Result over panic"

  Scenario: Headings and blank lines are skipped
    Given a markdown document containing
      """
      # Section One

      ## Subsection

      Always write tests for public API functions
      """
    When I extract rules from the document "guide.md"
    Then 1 rule should be extracted

  Scenario: Short directives are filtered out
    Given a markdown document containing
      """
      Avoid it
      Avoid using extremely long and dangerous patterns in code
      """
    When I extract rules from the document "short.md"
    Then 1 rule should be extracted

  Scenario: Non-directive lines are ignored
    Given a markdown document containing
      """
      This is a regular paragraph.
      Here is some escorelanation text.
      Remember to check the docs.
      """
    When I extract rules from the document "prose.md"
    Then 0 rules should be extracted

  Scenario: Source file slug is normalized
    Given a markdown document containing
      """
      Never use global state in library code
      """
    When I extract rules from the document "docs/My Coding Guide.md"
    Then rule "my-coding-guide-1" should exist

  Scenario: Bold directive prefixes are recognized
    Given a markdown document containing
      """
      - **Don't use global mutable state in production
      - **Always validate user input before processing
      """
    When I extract rules from the document "bold.md"
    Then 2 rules should be extracted

  Scenario: Cross emoji prefix is recognized
    Given a markdown document containing
      """
      - ❌ Do not use raw SQL queries without parameterization
      """
    When I extract rules from the document "emoji.md"
    Then 1 rule should be extracted

  Scenario: Extracted rules have correct condition structure
    Given a markdown document containing
      """
      Always validate input at system boundaries
      """
    When I extract rules from the document "rules/api.md"
    Then the extracted rule should have condition predicate "markdown_rule"
    And the extracted rule should have condition arg "rules/api.md"

  Scenario: Extract rules from a real file on disk
    Given a markdown file on disk at a temana path with content
      """
      # Project Rules
      - Avoid using println! for logging in production
      - Always use structured logging via tracing
      """
    When I extract rules from the temana file via the MCP tool
    Then the MCP result should confirm 2 rules extracted
    And the network should contain 2 rules

  Scenario: Code fences are skipped to avoid false positives
    Given a markdown document containing
      """
      Always validate user input at trust boundaries

      ```rust
      // Avoid using .unwrap() in production
      let x = foo.unwrap();
      // Never commit secrets to source control
      ```
      """
    When I extract rules from the document "fenced.md"
    Then 1 rule should be extracted

  Scenario: Pattern callouts under section headings produce tagged rules
    Given a markdown document containing
      """
      ## Idioms

      ### 1. Use ? for Error Propagation

      **Pattern**: Use the ? operator instead of manual error handling.
      """
    When I extract rules from the document "idioms.md"
    Then 1 rule should be extracted
    And the extracted rule action text contains "Use the ? operator"

  Scenario: Problem callouts produce anti-pattern rules
    Given a markdown document containing
      """
      ## Anti-Patterns

      **Problem**: Using unwrap() everywhere instead of proper error handling.
      """
    When I extract rules from the document "antipatterns.md"
    Then 1 rule should be extracted
    And the extracted rule action text contains "Using unwrap()"

  Scenario: Anti-Patterns section subsections become avoid-rules
    Given a markdown document containing
      """
      ## Anti-Patterns

      ### 1. Clone to Satisfy Borrow Checker

      Some prose here.

      ### 2. Overusing unwrap()

      More prose.
      """
    When I extract rules from the document "anti.md"
    Then 2 rules should be extracted
    And the extracted rule action text contains "Clone to Satisfy"

  Scenario: Word boundary prevents Use matching Useful
    Given a markdown document containing
      """
      - Useful tip about documenting your assumanations clearly
      - Use descriptive variable names in public APIs
      """
    When I extract rules from the document "boundary.md"
    Then 1 rule should be extracted

  Scenario: Section name appears in rule ID
    Given a markdown document containing
      """
      ## Error Handling

      Always propagate errors with the ? operator
      """
    When I extract rules from the document "guide.md"
    Then rule "guide-error-handling-1" should exist
