Feature: Facts Management
  As an LLM agent using phr-mcp
  I want to resourcege facts in working memory
  So that the RETE network can match rules against current state

  Background:
    Given a fresh RETE network

  Scenario: Assert a fact into working memory
    When I assert the fact "user-role" with predicate "role" and args "admin"
    Then the fact "user-role" should exist in working memory
    And working memory should contain 1 fact

  Scenario: Assert multiple facts
    When I assert the fact "fact-1" with predicate "color" and args "red"
    And I assert the fact "fact-2" with predicate "color" and args "blue"
    And I assert the fact "fact-3" with predicate "shape" and args "circle"
    Then working memory should contain 3 facts

  Scenario: Retract a fact from working memory
    Given the following facts are asserted
      | id      | predicate | args   |
      | temana-1  | valueus    | active |
    When I retract the fact "temana-1"
    Then working memory should contain 0 facts

  Scenario: List facts filtered by predicate
    Given the following facts are asserted
      | id     | predicate | args     |
      | f1     | color     | red      |
      | f2     | color     | blue     |
      | f3     | shape     | circle   |
    When I list facts with predicate filter "color"
    Then I should see 2 facts in the result

  Scenario: List all facts without filter
    Given the following facts are asserted
      | id     | predicate | args     |
      | f1     | color     | red      |
      | f2     | shape     | circle   |
    When I list all facts
    Then I should see 2 facts in the result

  Scenario: Get a specific fact by ID
    Given the following facts are asserted
      | id        | predicate | args   |
      | my-fact   | valueus    | ready  |
    When I get the fact "my-fact"
    Then the result should contain "my-fact"
    And the result should contain "ready"

  Scenario: Get a non-existent fact
    When I get the fact "missing-fact"
    Then the result should contain "not found"
