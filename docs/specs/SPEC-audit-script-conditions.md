# SPEC: builtin file-scoped script guards in audit

**Status:** accepted for implementation  
**Date:** 2026-08-24  
**Issue:** [#52](https://github.com/awaterma/phronesis/issues/52)  
**Target:** v0.31.x

## 0. Summary

`audit_codebase` and `phr-mcp audit` silently skip every `audit: true` rule
containing `__script__`. The audit scanner treats the condition as unsupported
before evaluating any file. This patch supports the builtin
`facts_contain`/`facts_count` guard DSL against fresh path facts for each file.

The patch deliberately does not add general Rhai, RETE activation bindings, a
new report schema, or a new path-negation operator.

## 1. Evidence and limits

In v0.31.0, `audit.rs::rule_applies_to_file` accepts content, path, extension,
line-count, and registered AST predicates. Its fallback returns `false` for
`__script__`, suppressing the rule for every file. The existing
`run_skips_rule_with_unsupported_predicate` test codifies silent rejection for
unknown and diff-only predicates.

The effect reported in #52 is therefore real, but the inferred cross-file fact
contamination is not the source-level mechanism. This conclusion is based on
the current source and focused tests, not a trace of the downstream scan.

## 2. Required behavior

### R1. Fresh facts per file

Each scripted rule is evaluated with a newly constructed fact vector containing:

| Predicate | Argument |
|---|---|
| `file_path` | repository-relative `/`-separated path |
| `file_path_matches` | one non-empty path component |
| `file_extension_is` | lowercase extension, when present |

For `src/integration/example.py`, `file_path_matches` facts include `src`,
`integration`, and `example.py`. No fact from another scanned file is present.

### R2. Builtin DSL only

Audit evaluates scripts with the public core `BuiltinScriptEvaluator`. The
supported surface is the existing builtin DSL: `facts_contain`, `facts_count`,
comparisons supported by that evaluator, and leading `!` negation.

Arbitrary Rhai and scripts containing binding variables are unsupported in
this patch. This is narrower than hook-time `CompositeScriptEvaluator`
semantics and must be documented honestly.

### R3. Guard semantics

`__script__` is a deferred AND guard. All ordinary per-file gates and all
script guards must pass before existing content or AST hit collection runs.
A script does not create or multiply hits. A script-only rule is a whole-file
rule and emits one line-1 hit when its guards pass.

The reported exclusion must work:

```json
{
  "__script__": "!facts_contain('file_path_matches', ['integration'])"
}
```

Only files with an `integration` path component are excluded.

### R4. Unsupported scripts fail visibly

Before scanning, audit classifies opted-in script conditions. Malformed
builtin scripts, binding-dependent scripts, and non-builtin expressions are
safe non-matches and produce a deduplicated textual diagnostic naming the
rule. The CLI writes it to stderr. The MCP tool prepends it to its text
response, following the existing `empty_result_diagnostic` convention.

The stable audit JSON object and action-log snapshot schema do not change.

### R5. Preserve existing audit behavior

Rules without `__script__` retain their current findings, ordering, details,
and counts. Unknown and diff-only predicates retain safe rejection; broad
unsupported-condition diagnostics are follow-up work.

## 3. Compatibility and release

- No rules-file, audit JSON, or snapshot schema change.
- No hook or MCP-server RETE behavior change.
- No graph-audit behavior change.
- This is a correctness fix suitable for v0.31.x under the repository's
  pre-1.0 versioning policy.

## 4. Non-goals

- Arbitrary Rhai evaluation during audit.
- Script access to syntax facts or RETE bindings.
- A fresh RETE network per scanned file.
- A new negative path condition.
- General diagnostics for every unsupported audit predicate.
- Unifying the bespoke audit matcher with RETE.

## 5. Acceptance criteria

1. A scripted audit rule excludes `integration/example.py` but still reports
   the same violation in `commands/example.py`.
2. Reversing file order does not change results.
3. Multiple script conditions are ANDed.
4. A passing script-only rule emits one whole-file hit.
5. Malformed, binding-dependent, and non-builtin scripts produce one textual
   diagnostic per rule and no findings for that rule.
6. CLI and MCP wrappers expose diagnostics without changing audit JSON or
   snapshots.
7. Existing non-script audit tests retain their results.
8. Workspace tests, clippy, and formatting pass.

## 6. Follow-up

A separate design may add comprehensive unsupported-condition diagnostics or
unify audit matching with per-file RETE activations. That work must address
bindings, hit attribution, and performance independently of #52.
