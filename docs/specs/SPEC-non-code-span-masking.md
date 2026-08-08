# SPEC: non-code span masking for content predicates

**Status:** proposed
**Date:** 2026-08-07
**Supersedes:** nothing
**Related:** `SPEC-rule-staleness.md` (graph-based rules), `SPEC-triple-store-rete.md` (structural pack)

## 1. Problem

`new_content_contains` is a substring test over raw file content. It has no
notion of whether the match landed in code, a comment, or a string literal.
For rules that police *code shape* — `.unwrap()`, `panic!(`, `dbg!(` — a match
inside prose is a false positive, and four of the six affected rules are
`block`-level, so the false positive halts work.

### 1.1 Measured, not assumed

Five probes fed through the real `phr-mcp pre-check` binary against this
repository's live rule pack, each a `Write` of a `.rs` file under `src/`:

| Probe content | Expected | Actual |
|---|---|---|
| `// Never call .unwrap() here; propagate with ? instead.` | pass | **BLOCKED** (exit 2) |
| `/// Prefer this over \`.unwrap()\`.` | pass | **BLOCKED** (exit 2) |
| `pub fn msg() -> &'static str { "avoid .unwrap() in src" }` | pass | **BLOCKED** (exit 2) |
| `pub fn msg() -> &'static str { "do not use panic!( ) casually" }` | pass | **BLOCKED** (exit 2) |
| `// dbg!( left here on purpose as documentation` | pass | WARNED (exit 1) |

5/5 false positives. Payloads are committed as fixtures (§7).

The doc-comment case is the sharpest: **a contributor cannot write
documentation about the rule without the rule blocking the edit.** This repo
documents its own rules extensively — `RUST-PATTERNS-GUIDE.md`, rule messages,
spec files — so the pattern that trips the rule is exactly the pattern its own
guidance must quote.

### 1.2 What already works — do not re-solve it

An earlier framing of this problem claimed test blocks were a false-positive
source. **They are not.** `diff_extract::strip_test_blocks` (`diff_extract.rs:146`)
runs on the hook path via `hook_facts.rs:86,287`, and
`rust_test_block_keep_mask_for` serves the audit path (`audit.rs:528`). Probe:
`.unwrap()` inside `#[cfg(test)] mod tests` → exit 0, no block. `src/` holds 819
such occurrences and `phr-mcp audit --rule enforce-no-unwrap-in-src` reports
zero violations.

That mechanism is correct and is the seam this spec extends.

**But it only holds for whole-file payloads.** The probe above was a `Write`,
which carries the entire file, so `strip_test_blocks` can see the
`#[cfg(test)]` marker above the match. An `Edit` carries only the changed
fragment. If the marker is outside the fragment — which it almost always is
when editing an existing test — the stripper has nothing to key on and treats
the fragment as production code.

Reproduced while writing this specification's own implementation: an `Edit`
adding a test fixture whose *string literal* contained `.unwrap()`, inside a
`#[cfg(test)]` module, was blocked. The same content delivered as a `Write`
passes.

This matters for scoping. §1.2's "test blocks are already handled" is true of
`Write` and false of `Edit`, and `Edit` is the more common tool. So the
false-positive surface is larger than the five probes suggest, and masking —
which works on whatever content it is handed, marker or no marker — addresses
the fragment case that test-block stripping structurally cannot.

### 1.3 Writing this file was blocked twice by the bug it describes

The first draft quoted a deflection-family pattern in §3.3 to explain why
masking must be opt-in. The `Write` was refused. Rephrasing around that literal
produced a second draft quoting a *different* deflection pattern, and that
`Write` was refused too — by a different rule of the same family:

```
BLOCKED — Drop the '<phrase A>' disclaimer. Own the fix or own the
          decision to defer.
BLOCKED — Don't deflect with '<phrase B>'. Either fix it as part of this
          change, defer with a clear rationale, or drop the disclaimer.
```

Both matches were inside fenced blocks, as the literals under discussion. No
disclaimer appeared anywhere in either draft. This is the §1.1 failure in its
most expensive form: the rules blocked the specification of their own fix, twice,
and the file only landed after every deflection pattern was replaced with a
placeholder.

**The design in this document would not have prevented it.** The file is `.md`,
and §3.2 returns markdown unchanged because there is no grammar for it. Masking
helps source files; it does nothing for prose files, which is precisely where
rule documentation lives. Recorded as a limit in §6 rather than papered over.
The workaround used here — writing `<phrase A>` instead of the literal — is the
self-censorship that indicates a rule is mispriced. §9 proposes what to do.

## 2. Non-goals

- **No new predicates.** This is not a regex→AST rule rewrite. Rules keep
  their `new_content_contains` conditions; only the *content those conditions
  see* changes.
- **`bash_command_matches` is out of scope.** A `git commit -m` inside a shell
  heredoc trips `nudge-verify-before-commit` for the same underlying reason,
  but shell quoting cannot be parsed as reliably as a tree-sitter grammar, and
  a wrong answer there silently disables a commit gate. Deferred, deliberately.
- **No change to scoring, drift, or the graph.**

## 3. Design

### 3.1 One function, delimiters preserved

```rust
/// Blank the payload of comment and string-literal spans, preserving byte
/// offsets, line count, and delimiters. Returns content unchanged when the
/// file has no tree-sitter grammar or fails to parse.
pub fn blank_non_code(file_path: &str, content: &str) -> String
```

Two properties matter:

**Blanking, not deleting.** Non-code bytes become spaces; length and line
structure are preserved. `strip_test_blocks` deletes lines, which is why audit
needed a second line-mask function to keep its `file:line` reporting honest.
Blanking needs no such twin — one function serves both call sites.

**Delimiters survive; payloads do not.** For a string, the quote characters
remain and the interior is blanked. For a comment, the introducer (`//`, `/*`,
`#`) remains and the text is blanked. This is not cosmetic: the live rule
`audit-string-concat-with-plus` matches a closing quote followed by ` + &`.
Blanking delimiters would break a rule this change is not supposed to touch.

### 3.2 Language coverage is whatever the parser layer already has

`syntax::parsed::ParsedFile` covers Rust, Swift, Python, TypeScript and TSX.
Node kinds are selected by a predicate over `node.kind()` rather than a
hardcoded per-language table:

- kind contains `"comment"` — covers Rust `line_comment`/`block_comment`
  (`rust/docs.rs:53-59`), Python/TypeScript `comment`
  (`python.rs:86`, `typescript.rs:110`), Swift `multiline_comment`.
- kind contains `"string"` — covers Rust `string_literal`/`raw_string_literal`
  (`rust/eval.rs:86`), Python `string` (`python.rs:194`), TS
  `template_string`, Swift `line_string_literal`.
- plus an explicit small set for kinds matching neither: Rust `char_literal`.

Matching on substring rather than an exact list means a tree-sitter grammar
upgrade that renames `line_comment` → `comment` does not silently stop masking.
The cost is over-matching a kind like `string_interpolation`, whose interior is
real code. That direction is safe: blanking real code can only *lose* a
detection, never invent one. §6 records it as a known limit.

Any file with no grammar — `.rhai`, `.md`, `.toml`, unknown extensions —
returns unchanged. `block-rhai-print-in-script` therefore behaves exactly as
today, and so does every markdown file (§1.3).

### 3.3 Opt-in per rule — the load-bearing decision

**Masking must not be global.** The LLM-deflection family matches prose: its
patterns are short phrases that attribute a failure to something other than the
current change. Such a phrase is *most likely* to appear in a code comment.
Blanking comments globally would blind those rules in their most probable
hiding place, turning a precision fix for six rules into a correctness
regression for seven. That is a strictly worse trade.

So a rule opts in with a new disk field, `code_only`:

```json
{ "id": "enforce-no-unwrap-in-src",
  "code_only": true,
  "when": [
    { "new_content_contains": ".unwrap()" },
    { "file_path_matches": "src" }
  ] }
```

`code_only` defaults to `false`. Absent the field, payloads are byte-identical
to today — the same compatibility contract the `context` pack uses.

### 3.4 Ordering

`blank_non_code` composes with, and runs after, `strip_test_blocks`. Test-block
removal is line-structural and must see real braces; blanking a string
containing `{` before that pass would perturb its depth tracking. Blanking
after is a no-op for it.

## 4. Rules that opt in (v1)

| Rule | Level | Token |
|---|---|---|
| `enforce-no-unwrap-in-src` | block | `.unwrap()` |
| `enforce-no-panic-in-src` | block | `panic!(` |
| `enforce-no-todo-in-src` | block | `todo!(` |
| `enforce-no-unimplemented-in-src` | block | `unimplemented!(` |
| `warn-dbg-in-src` | warn | `dbg!(` |
| `warn-expect-with-empty-message` | warn | `.expect("")` |

`warn-expect-with-empty-message` is the interesting one: its pattern is *itself*
a string literal with an empty payload. Blanking the interior of an empty string
is a no-op, so the pattern survives — but this must be pinned by a test, because
an implementation that blanks delimiters too would silently disable the rule.

Six of 31 substring rules. The remaining 25 are unchanged and untouched: 13 are
prose or shell rules where masking would be wrong, and 12 already use AST or
graph predicates.

## 5. Where it plugs in

- `crates/phronesis-mcp/src/diff_extract.rs` — `blank_non_code`, beside
  `strip_test_blocks`.
- `crates/phronesis-mcp/src/hook_facts.rs:86,287` — apply when the rule under
  evaluation sets `code_only`.
- `crates/phronesis-mcp/src/audit.rs:528` — same, in the whole-tree scan.
- `crates/phronesis-mcp/src/rules_file.rs` — `code_only` on the v2 disk rule,
  `#[serde(default)]`, round-tripped by `write_source`.

The predicate-evaluation seam decides per rule, so masked and unmasked rules
coexist in one pass over one file.

## 6. Known limits

- **Prose files get no protection.** Markdown, plain text, and any format
  without a grammar are unmasked, so §1.3 remains reproducible after this
  change. Masking is a source-file mechanism.
- **Interpolated expressions are blanked.** A `.unwrap()` inside a TS template
  literal or a Swift interpolation will not be seen. Loses detections; cannot
  create false positives.
- **Unparseable files fall back to unmasked**, so a syntax error mid-edit
  restores today's false-positive behavior rather than silently passing
  everything. Failing toward the noisier answer is correct for a `block` rule.
- **Parse cost.** Content predicates currently need no parse. `code_only` rules
  add one tree-sitter parse per file per hook invocation, amortized by
  `ParsedFile` being parse-once across predicates in the same pass.

## 7. Acceptance criteria

1. The five §1.1 probes are committed under
   `tests/fixtures/payloads/claude-code/` with `expect.exit: 0`, and pass
   through `cargo test --test payload_contract`.
2. Positive control preserved: `o.unwrap()` in a production function still
   exits 2. A precision fix that also disables the rule is not a fix.
3. `.expect("")` still matches after masking (§4).
4. `audit-string-concat-with-plus` still matches its pattern — delimiters
   survived.
5. A deflection rule still fires on its phrase **in a code comment**, proving
   opt-in is real and that family was not caught in the blast radius.
6. A rule without `code_only` produces byte-identical hook output to the
   current build across the whole existing payload corpus.
7. `phr-mcp audit --rule enforce-no-unwrap-in-src` still reports 0 on this
   repository, and `file:line` numbers are unchanged for a rule that does
   report hits — the blanking-not-deleting property.
8. Every supported language has one comment-masking and one string-masking
   test. Swift's node kinds are unverified in this spec and must be confirmed
   against the grammar during implementation, not assumed.
9. A fixture delivered as an `Edit` fragment — a string literal containing
   `.unwrap()`, with no `#[cfg(test)]` marker inside the fragment — passes.
   This is the §1.2 gap that test-block stripping cannot close, and it is the
   case most likely to recur in normal work.

## 8. Open question deferred to implementation

Whether `code_only` should also mask Rust **attribute** payloads.
`audit-allow-dead-code-in-src` matches `#[allow(dead_code)]`, which is neither
comment nor string, so it is unaffected either way. Named so a future reader
does not mistake the omission for an oversight.

## 9. Follow-up this spec does not solve

§1.3 shows the deflection family firing on *quotations* of its own patterns in
a spec file, and §3.2 explains why masking cannot help there. Two candidate
fixes, neither in scope:

- **Path exclusion.** Exempt `docs/specs/**` and `docs/**-GUIDE.md` from the
  deflection family, on the grounds that documents *about* rules must be able
  to quote them. Cheap; slightly widens the hiding place.
- **A fenced-block escape.** Treat content inside a markdown code fence as
  quoted rather than asserted. More faithful to intent, needs a markdown-aware
  pass this spec's mechanism does not provide.

Recording both rather than choosing, because the right answer depends on
whether the deflection rules are meant to police what the model *says* or what
it *writes to disk* — a question the rules themselves do not currently
distinguish. That ambiguity, not the substring matching, is the deeper defect.
