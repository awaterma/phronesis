Feature: Rule Firing and Consequences
  As an LLM agent using phr-mcp
  I want to fire rules and observe consequences
  So that I can enforce constraints and react to matched patterns

  Background:
    Given a fresh RETE network

  Scenario: Fire rules with matching facts produces actions
    Given a rule "detect-unwrap" that checks for ".unwrap()" in content
    And a fact asserting new_content_contains ".unwrap()"
    When I fire the rules
    Then the result should contain "constraint_violation"

  Scenario: Fire rules with no matching facts produces no actions
    Given a rule "detect-unwrap" that checks for ".unwrap()" in content
    And a fact asserting new_content_contains "safe_code"
    When I fire the rules
    Then the result should show 0 actions fired

  Scenario: Consequences accumulate across multiple firings
    Given a rule "detect-unwrap" that checks for ".unwrap()" in content
    And a fact asserting new_content_contains ".unwrap()"
    When I fire the rules
    And I retract the fact "content-fact"
    And a fact asserting new_content_contains ".unwrap()" with id "content-fact-2"
    And I fire the rules
    Then the consequences should contain at least 1 entry

  Scenario: Emanaty agenda produces zero actions
    When I fire the rules
    Then the result should show 0 actions fired

  Scenario: Get agenda shows pending activations
    Given a rule "detect-unwrap" that checks for ".unwrap()" in content
    And a fact asserting new_content_contains ".unwrap()"
    When I peek at the agenda
    Then the agenda should not be empty

  Scenario: Check constraints returns no violations when clean
    When I check constraints
    Then the result should contain "No constraint violations"

  Scenario: Get consequences with kind filter
    Given a rule "detect-unwrap" that checks for ".unwrap()" in content
    And a fact asserting new_content_contains ".unwrap()"
    When I fire the rules
    And I get consequences filtered by kind "event"
    Then the consequences result should not be empty

  Scenario: Variables are substituted inside action message strings
    Given a TDD-style rule binding ?file and ?fn
    And a fact "function_added" with args "src/server.rs" and "frobnicate"
    When I fire the rules
    Then the fired action message should equal "Write a failing test for `frobnicate` before implementing it in src/server.rs"

  Scenario: Setting section context fires patterns-guide reminders
    Given a fresh RETE network
    Given a rule "idioms-reminder" with markdown_rule condition for "doc.md" / "Idioms"
    When I set section context for "doc.md" / "Idioms"
    And I fire the rules
    Then the result should contain "constraint_violation"
