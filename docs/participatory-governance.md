# Participatory governance

The model is both governed by rules and a participant in rule
evolution. Three workflows close the loop.

> **Why this lives here and not in `.phronesis/durable.md`.**
> It used to be a section of `durable.md`. Measured with
> `phr-mcp context inspect --event session`, that section rendered as
> `charter:2` at 1825 bytes against a `charter_max_bytes` ceiling of
> 2048 that earlier sections had already consumed — so it was dropped
> with reason `kind_ceiling` on every single session, and never
> reached the model at all. Worse, the bytes it displaced were coming
> out of the active-rule list.
>
> Prose the model must act on belongs in `durable.md` or `kernel.md`
> and must fit the measured budget. Reference material belongs in
> `docs/`, where it is read on demand rather than paid for every
> session. This is the latter.

## Remember → decide → enforce

When the user says "remember X" or "make a rule for X":

1. Check drift first — is the gap real?
2. Scaffold a decision: `phr-mcp decision new <slug>`
3. Fill in Context, Decision, Enforcement, Consequences
4. If enforceable (code-shape, command pattern):
   - Propose a rule using available predicates
     (`new_content_contains`, `file_path_matches`,
     `file_extension_is`, etc.)
   - Write it to `.phronesis/rules.json`
   - Wire `enforces: [rule-id]` in the decision frontmatter
5. If not enforceable (process, naming, social):
   - Note in Enforcement that no automated rule is possible
   - Offer to add prose guidance to `durable.md` instead
6. Ask the human to approve before committing

## Friction-driven proposals

When a rule blocks you 3+ times in the same session for the same
pattern, pause and assess:

- Use `get_action_log` with `only_nonzero_exit: true` to review
- If the rule scope is too broad (legitimate code keeps tripping
  it): propose a decision page that refines the scope — narrower
  `file_path_matches`, an exclusion, a predicate change. Present
  the proposal to the human.
- If you keep hitting it legitimately: the rule is working. Adjust
  your approach, don't propose weakening enforcement.

## Cross-session knowledge transfer

When you discover something significant — a bug pattern, a design
insight, a rollout lesson — consider writing a decision page. ADR
pages in `.phronesis/wiki/decisions/` travel with the repo and are
available to future sessions. This turns a session-local discovery
into durable project knowledge. Ask the human before writing —
not every insight warrants a formal decision.
