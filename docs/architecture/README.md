# Architecture

Explanations of how the shipped system behaves and why it is built that way. Shipped behavior only — proposals and designs with open questions belong in [Specs](../specs/README.md), historical intent in the [Archive](../archive/README.md).

- [Rust ownership evidence](OWNERSHIP-EVIDENCE.md) — what clone/filter/await/lock observations mean, how strong each observation is, and why it is evidence rather than proof.
- [Participatory governance](participatory-governance.md) — how the model participates in its own rule evolution: the three workflows that close the loop and why this text lives outside `.phronesis/durable.md`.
- [The explainer](https://awaterma.github.io/phronesis/explainer.html) ([source](../explainer.html)) — long-form essay on the RETE engine and design intent.
- Crate-level architecture lives beside the code: [`phronesis`](../../crates/phronesis/README.md), [`phronesis-governance`](../../crates/phronesis-governance/README.md), [`phronesis-mcp`](../../crates/phronesis-mcp/README.md), [`phronesis-rhai`](../../crates/phronesis-rhai/README.md), [`phronesis-metrics`](../../crates/phronesis-metrics/README.md).
- Specs that describe shipped behavior with design context: see the [Specs index](../specs/README.md) — each spec carries an explicit lifecycle-status header.
