Feature: Coverage evidence
  As a maintainer editing a branch
  I want the hook to know which tests exercise the code I changed
  So that relevance claims come from durable evidence, not guesswork

  Background:
    Given a project with the safe_divide fixture and its real coverage export
    And the coverage evidence is hydrated at hook fire

  # Mirrors acceptance A2 (golden trace, coverage_golden.rs).
  Scenario: Branch change derives the branch-relevant test
    When the zero-denominator error message is edited
    Then rule log-relevant-test-for-change fires for test "rejects_zero_denominator" at the branch region
    And it does not fire for "divides_positive_values" at the branch region
    And it does not fire for "divides_negative_values" at the branch region

  # Mirrors acceptance A3 (evidence gap, SPEC §5.2).
  Scenario: Changed region without any dynamic evidence reports the gap
    Given a coverage store with no imported evidence
    When the zero-denominator error message is edited
    Then rule warn-evidence-gap fires for the changed region
    And the warning names regions with "neither dynamic test evidence nor formal proof evidence"

  # Mirrors acceptance A4 (demand gating).
  Scenario: Coverage facts stay bounded by construction
    Given no loaded rule mentions any coverage relation
    When an edit event hydrates
    Then zero coverage facts are asserted

  # Mirrors acceptance A5 (staleness, SPEC §5.3).
  Scenario: Stale coverage warns before a commit
    Given a coverage store imported at a revision other than HEAD
    When a pre-check runs with "git commit -m x"
    Then rule warn-commit-on-stale-coverage fires
    And the warning names the staleness without blocking the commit