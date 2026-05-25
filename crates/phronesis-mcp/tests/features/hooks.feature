Feature: Claude Code Hook Integration
  As a Claude Code user with phr-mcp hooks
  I want pre-check and post-check hooks to enforce rules
  So that rule violations are caught before or after file modifications

  Scenario: Pre-check allows non-Edit/Write tools
    When I run pre-check with tool "Read" and input
      """
      {"file_path": "src/main.rs"}
      """
    Then the hook should exit with code 0

  Scenario: Pre-check allows edits when no rules file exists
    When I run pre-check with tool "Edit" and input
      """
      {"file_path": "src/main.rs", "old_string": "old", "new_string": "new"}
      """
    Then the hook should exit with code 0

  Scenario: Post-check allows non-Edit/Write tools
    When I run post-check with tool "Bash" and input
      """
      {"command": "ls"}
      """
    Then the hook should exit with code 0

  Scenario: Post-check allows writes when no rules file exists
    When I run post-check with tool "Write" and input
      """
      {"file_path": "src/main.rs", "content": "fn main() {}"}
      """
    Then the hook should exit with code 0

  Scenario: Pre-check blocks when rules are violated
    Given a rules file with a pre-check rule blocking ".unwrap()"
    When I run pre-check with tool "Edit" and input
      """
      {"file_path": "src/lib.rs", "old_string": "old", "new_string": "let x = foo.unwrap();"}
      """
    Then the hook should exit with code 2
    And stderr should contain "BLOCKED"

  Scenario: Post-check warns when file content violates rules
    Given a rules file with a post-check rule requiring "SCHEMA_VERSION"
    And a file on disk at the checked path without "SCHEMA_VERSION"
    When I run post-check for the checked file
    Then the hook should exit with code 1
    And stderr should contain "WARNING"

  Scenario: Pre-check fails closed on malformed rules.json (security finding #7)
    Given a malformed rules file
    When I run pre-check with tool "Edit" and input
      """
      {"file_path": "src/x.rs", "old_string": "a", "new_string": "b"}
      """
    Then the hook should exit with code 2
    And stderr should contain "malformed"

  Scenario: Post-check warns on malformed rules.json
    Given a malformed rules file
    When I run post-check with tool "Write" and input
      """
      {"file_path": "src/x.rs", "content": "x"}
      """
    Then the hook should exit with code 1
    And stderr should contain "malformed"

  Scenario: Post-check rejects path traversal in file_path (security finding #2)
    Given a rules file with a post-check rule requiring "SCHEMA_VERSION"
    When I run post-check with tool "Write" and input
      """
      {"file_path": "../../../etc/passwd", "content": "x"}
      """
    Then the hook should exit with code 1
    And stderr should contain "outside project root"
