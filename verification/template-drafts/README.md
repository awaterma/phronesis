# Template drafts

This directory holds **draft** verification templates: Rhai render scripts
that are not yet trusted. See
`docs/specs/SPEC-verification-artifact-generation.md`.

- `phr-mcp verify render` and `phr-mcp verify run` read templates from
  `verification/templates/`, which is a trust anchor. A human owns that
  directory. The `llm` rule pack refuses agent writes to it.
- A draft here is read **only** when you pass `--allow-drafts`, and only
  when `verification/templates/` has no template of the same name. Without
  the flag, a draft that matches is refused, and the error names it.
- A result produced from a draft is recorded with
  `template_origin: "template_drafts"`. It never counts as verified
  evidence: hydration reports it as
  `unbound_evidence(<property>, <verifier>, draft_template)` and never as
  `verification_result`.

## Promoting a draft

Agents may write drafts here. Only a human promotes one:

1. Review the draft, and at least one artifact rendered from it, in
   `verification/unreviewed/`.
2. Copy the draft into `verification/templates/` under the same name
   (`<verifier>-<kind>.rhai`) and commit it as a human-authored change.
3. Re-render without `--allow-drafts`. Because the provenance header names
   the template directory and hash, the artifact bytes change, so the
   artifact needs a fresh allowlist approval before it can run.

## Naming

Each template is `<verifier>-<kind>.rhai`. `<verifier>` comes from the
property's encoding, and `<kind>` comes from the property's `kind`. Both are
limited to lowercase ASCII letters, digits, and `_`.

## Files

- `verus-postcondition.rhai` is a seam example. It renders a standalone
  Verus-native module that proves the `safe_divide` postconditions, and it
  refuses any other subject. It is not a general postcondition template.
