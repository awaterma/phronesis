use phronesis_mcp::server::{extract_rules_from_markdown, strip_directive_prefix};

#[test]
fn strip_avoid_prefix() {
    assert_eq!(
        strip_directive_prefix("- Avoid using .unwrap() in production"),
        Some("using .unwrap() in production")
    );
}

#[test]
fn strip_never_prefix() {
    assert_eq!(
        strip_directive_prefix("Never commit secrets to the repository"),
        Some("commit secrets to the repository")
    );
}

#[test]
fn strip_always_prefix() {
    assert_eq!(
        strip_directive_prefix("Always handle errors explicitly"),
        Some("handle errors explicitly")
    );
}

#[test]
fn strip_prefer_prefix() {
    assert_eq!(
        strip_directive_prefix("Prefer composition over inheritance"),
        Some("composition over inheritance")
    );
}

#[test]
fn strip_bold_prefix() {
    assert_eq!(
        strip_directive_prefix("- **Don't use global mutable state"),
        Some("use global mutable state")
    );
}

#[test]
fn strip_use_prefix() {
    assert_eq!(
        strip_directive_prefix("- Use descriptive variable names everywhere"),
        Some("descriptive variable names everywhere")
    );
}

#[test]
fn no_directive_returns_none() {
    assert_eq!(strip_directive_prefix("This is a normal line"), None);
    assert_eq!(strip_directive_prefix("Some random text"), None);
}

#[test]
fn short_constraint_text_skipped() {
    assert_eq!(strip_directive_prefix("Avoid it"), Some("it"));
    let md = "Avoid it\n";
    let rules = extract_rules_from_markdown(md, "test.md");
    assert!(rules.is_empty());
}

#[test]
fn extract_rules_basic() {
    let md = "\
# Coding Standards

- Avoid using .unwrap() in production code paths
- Never commit secrets or credentials to the repository
Just a regular line that should be ignored.
- Prefer Result over panic for error handling in libraries
";
    let rules = extract_rules_from_markdown(md, "docs/standards.md");
    assert_eq!(rules.len(), 3);

    assert_eq!(rules[0].id, "standards-1");
    assert_eq!(rules[0].actions[0].action_type, "constraint_warning");
    assert!(rules[0].actions[0].params[0].contains("using .unwrap()"));

    assert_eq!(rules[1].id, "standards-2");
    assert!(rules[1].actions[0].params[0].contains("commit secrets"));

    assert_eq!(rules[2].id, "standards-3");
    assert!(rules[2].actions[0].params[0].contains("Result over panic"));
}

#[test]
fn extract_rules_skips_headings_and_blank_lines() {
    let md = "\
# Section One

## Subsection

Always write tests for public API functions

";
    let rules = extract_rules_from_markdown(md, "guide.md");
    assert_eq!(rules.len(), 1);
    // Section context "Subsection" is encoded in the rule ID
    assert_eq!(rules[0].id, "guide-subsection-1");
}

#[test]
fn extract_rules_source_slug_normalization() {
    let rules = extract_rules_from_markdown(
        "Never use global state in library code\n",
        "docs/My Coding Guide.md",
    );
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].id, "my-coding-guide-1");
}

#[test]
fn extract_rules_conditions_reference_source() {
    let rules =
        extract_rules_from_markdown("Always validate input at boundaries\n", "rules/api.md");
    assert_eq!(rules[0].conditions[0].predicate, "markdown_rule");
    assert_eq!(rules[0].conditions[0].args[0], "rules/api.md");
}

// ──────────────────────────────────────────────────────────────────────
// Improved extractor: code fences, callouts, sections, word boundaries
// ──────────────────────────────────────────────────────────────────────

#[test]
fn code_fences_are_skipped() {
    let md = "\
Always validate user input at trust boundaries

```rust
// Avoid using .unwrap() in production
let x = foo.unwrap();
// Never commit secrets to source control
```

After the fence resumes ordinary text.
";
    let rules = extract_rules_from_markdown(md, "test.md");
    assert_eq!(
        rules.len(),
        1,
        "Lines inside ```fences``` must not produce rules"
    );
    assert!(rules[0].actions[0].params[0].contains("validate user input"));
}

#[test]
fn tilde_code_fences_are_skipped() {
    let md = "\
Always handle errors explicitly

~~~rust
// Avoid panics in library code
~~~
";
    let rules = extract_rules_from_markdown(md, "test.md");
    assert_eq!(rules.len(), 1);
}

#[test]
fn pattern_callout_is_extracted() {
    let md = "\
## Idioms

### 1. Use ? for Error Propagation

**Pattern**: Use the ? operator instead of manual error handling.
";
    let rules = extract_rules_from_markdown(md, "test.md");
    assert_eq!(rules.len(), 1);
    let action = &rules[0].actions[0].params[0];
    // SPEC-extract-rules-defaults: bracketed prefix is stripped at extraction.
    assert!(!action.contains("[pattern]"), "got: {}", action);
    assert!(action.contains("Use the ? operator"), "got: {}", action);
}

#[test]
fn problem_callout_is_extracted() {
    let md = "\
**Problem**: Using unwrap() everywhere instead of proper error handling.
";
    let rules = extract_rules_from_markdown(md, "test.md");
    assert_eq!(rules.len(), 1);
    let action = &rules[0].actions[0].params[0];
    assert!(!action.contains("[problem]"));
    assert!(action.contains("Using unwrap()"));
}

#[test]
fn anti_pattern_section_extracts_subsection_titles() {
    let md = "\
## Anti-Patterns

### 1. Clone to Satisfy Borrow Checker

Some prose here describing the anti-pattern.

### 2. Overusing `unwrap()`

More prose.
";
    let rules = extract_rules_from_markdown(md, "test.md");
    assert_eq!(rules.len(), 2);
    assert!(rules[0].actions[0].params[0].contains("Clone to Satisfy"));
    assert!(rules[1].actions[0].params[0].contains("Overusing"));
    // SPEC-extract-rules-defaults: bracketed prefix is stripped at extraction.
    assert!(!rules[0].actions[0].params[0].contains("[anti_pattern]"));
}

#[test]
fn non_anti_pattern_subsections_are_not_extracted_as_anti_patterns() {
    let md = "\
## Idioms

### 1. Use ? for Error Propagation

Some prose without callouts.
";
    let rules = extract_rules_from_markdown(md, "test.md");
    // No `**Pattern**:` callout and not in Anti-Patterns section → no rule
    assert_eq!(rules.len(), 0);
}

#[test]
fn section_context_appears_in_rule_id() {
    let md = "\
## Error Handling

Always propagate errors with the ? operator
";
    let rules = extract_rules_from_markdown(md, "guide.md");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].id, "guide-error-handling-1");
}

#[test]
fn section_context_appears_in_conditions() {
    let md = "\
## Concurrency

Always use Send-safe types across await points
";
    let rules = extract_rules_from_markdown(md, "guide.md");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].conditions[0].args[0], "guide.md");
    assert_eq!(rules[0].conditions[0].args[1], "Concurrency");
}

#[test]
fn word_boundary_use_does_not_match_useful() {
    use phronesis_mcp::server::strip_directive_prefix;
    assert_eq!(strip_directive_prefix("Useful tip about something"), None);
    assert_eq!(strip_directive_prefix("- Useful pattern"), None);
}

#[test]
fn word_boundary_never_does_not_match_nevertheless() {
    use phronesis_mcp::server::strip_directive_prefix;
    assert_eq!(
        strip_directive_prefix("Nevertheless we should consider it"),
        None
    );
}

#[test]
fn word_boundary_avoid_does_not_match_avoidance() {
    use phronesis_mcp::server::strip_directive_prefix;
    assert_eq!(
        strip_directive_prefix("Avoidance is sometimes the right call"),
        None
    );
}

#[test]
fn use_directive_still_works_with_space_boundary() {
    use phronesis_mcp::server::strip_directive_prefix;
    assert_eq!(
        strip_directive_prefix("- Use descriptive variable names"),
        Some("descriptive variable names")
    );
}

#[test]
fn callout_with_list_marker_is_extracted() {
    let md = "\
- **Pattern**: Prefer composition over inheritance in all cases
";
    let rules = extract_rules_from_markdown(md, "test.md");
    assert_eq!(rules.len(), 1);
    assert!(rules[0].actions[0].params[0].contains("Prefer composition"));
}

#[test]
fn extraction_against_real_patterns_guide() {
    // Smoke test: extracting against the canonical patterns doc should yield
    // a substantial set of rules, not zero and not thousands.
    let path = "docs/RUST-PATTERNS-GUIDE.md";
    if let Ok(content) = std::fs::read_to_string(path) {
        let rules = extract_rules_from_markdown(&content, path);
        assert!(
            rules.len() >= 20,
            "Expected ≥20 rules from the patterns guide, got {}",
            rules.len()
        );
        assert!(
            rules.iter().any(|r| r.id.contains("anti-patterns")),
            "Expected at least one anti-pattern rule"
        );
        // SPEC-extract-rules-defaults: no rule message carries a bracketed
        // extraction-time discriminator in the user-facing string.
        assert!(
            rules
                .iter()
                .all(|r| !r.actions[0].params[0].starts_with("[pattern]")
                    && !r.actions[0].params[0].starts_with("[anti_pattern]")
                    && !r.actions[0].params[0].starts_with("[problem]")
                    && !r.actions[0].params[0].starts_with("[context]")),
            "No extracted rule should leak a bracketed metadata prefix",
        );
    }
}

// ──────────────────────────────────────────────────────────────────────
// SPEC-extract-rules-defaults — scoped slice (0.14.0)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn extract_rules_defaults_action_to_warn() {
    let md = "\
# Coding Standards

- Avoid using .unwrap() in production code paths

## Idioms

### 1. Use ? for Error Propagation

**Pattern**: Use the ? operator instead of manual error handling.

## Anti-Patterns

**Problem**: Using unwrap() everywhere instead of proper error handling.

### 1. Clone to Satisfy Borrow Checker

Some prose.
";
    let rules = extract_rules_from_markdown(md, "guide.md");
    assert!(
        rules.len() >= 4,
        "expected at least 4 rules, got {}",
        rules.len()
    );
    for r in &rules {
        assert_eq!(
            r.actions[0].action_type, "constraint_warning",
            "rule {} had action_type {}, expected constraint_warning (warn)",
            r.id, r.actions[0].action_type,
        );
    }
}

#[test]
fn extract_rules_strips_bracketed_prefix() {
    let md = "\
## Idioms

### 1. Use ? for Error Propagation

**Pattern**: Use the ? operator instead of manual error handling.

**Use Case**: Complex object construction with optional parameters.

## Anti-Patterns

**Problem**: Using unwrap() everywhere instead of proper error handling.

### 1. Clone to Satisfy Borrow Checker

Some prose.
";
    let rules = extract_rules_from_markdown(md, "guide.md");
    assert!(!rules.is_empty(), "expected some extracted rules");
    for r in &rules {
        let msg = &r.actions[0].params[0];
        for tag in ["[pattern]", "[anti_pattern]", "[context]", "[problem]"] {
            assert!(
                !msg.starts_with(tag),
                "rule {} message leaked prefix {}: {}",
                r.id,
                tag,
                msg,
            );
            assert!(
                !msg.contains(tag),
                "rule {} message contains bracketed tag {}: {}",
                r.id,
                tag,
                msg,
            );
        }
    }
}
