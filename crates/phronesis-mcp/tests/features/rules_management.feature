Feature: Rules Management
  As an LLM agent using phr-mcp
  I want to resourcege rules in the RETE network
  So that I can define constraints and patterns for bounded interaction

  Background:
    Given a fresh RETE network

  Scenario: Add a simple rule
    When I add a rule "no-unwrap" with priority 10
      | predicate            | args       |
      | new_content_contains | .unwrap()  |
    And the rule has actions
      | action_type            | params                                    |
      | constraint_violation   | Avoid .unwrap() in production code        |
    Then the rule "no-unwrap" should exist in the network
    And the network should contain 1 rule

  Scenario: List all rules
    Given the following rules are loaded
      | id          | priority |
      | rule-alpha  | 10       |
      | rule-beta   | 5        |
      | rule-gamma  | 8        |
    When I list all rules
    Then I should see 3 rules

  Scenario: Get a specific rule by ID
    Given the following rules are loaded
      | id          | priority |
      | target-rule | 7        |
    When I get the rule "target-rule"
    Then the result should contain "target-rule"

  Scenario: Get a non-existent rule
    When I get the rule "ghost-rule"
    Then the result should contain "not found"

  Scenario: Remove a rule
    Given the following rules are loaded
      | id            | priority |
      | doomed-rule   | 5        |
    When I remove the rule "doomed-rule"
    Then the network should contain 0 rules

  Scenario: Rules have correct priority ordering
    Given the following rules are loaded
      | id         | priority |
      | low-pri    | 1        |
      | high-pri   | 100      |
    When I get the rule "high-pri"
    Then the rule priority should be 100
