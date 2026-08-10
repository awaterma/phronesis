# Your first Phronesis rule

This tutorial runs one rule through the core RETE engine. It uses only the
public API of the `phronesis` crate and takes about five minutes.

## Run the example

You need Git and Rust 1.90 or newer. Clone the repository, then run:

```sh
git clone https://github.com/awaterma/phronesis.git
cd phronesis
cargo run --example minimal --package phronesis
```

Expected output:

```text
rule fired: send_welcome(Ada)
```

The example checks the result before printing it, so an unexpected rule result
also makes the command fail.

## What happened

The program in
[`crates/phronesis/examples/minimal.rs`](../crates/phronesis/examples/minimal.rs)
performs one complete rule cycle:

1. It creates a `ReteNetwork`.
2. It adds a rule: when `user_joined(?name)` matches, produce
   `send_welcome(?name)`.
3. It asserts the fact `user_joined(Ada)`.
4. It updates and executes the agenda.

`?name` is a variable. The matching fact binds it to `Ada`, and Phronesis
substitutes that value into the resulting action. The host application decides
what `send_welcome(Ada)` actually does; the rules engine remains domain-neutral
and deterministic.

## Map the example to the engine

| Concept | Public type | Implementation |
| --- | --- | --- |
| Working-memory input | `Fact` | [`engine_types.rs`](../crates/phronesis/src/engine_types.rs) |
| Match pattern | `Condition` | [`alpha_network.rs`](../crates/phronesis/src/alpha_network.rs) and [`beta_network.rs`](../crates/phronesis/src/beta_network.rs) |
| Rule and result | `Rule`, `Action` | [`production.rs`](../crates/phronesis/src/production.rs) |
| Rule cycle | `ReteNetwork` | [`network.rs`](../crates/phronesis/src/network.rs) |

## Continue learning

- Run `cargo run --example push_and_pull --package phronesis` to see the
  consequence and actor interfaces.
- Run `cargo run --example rule_driven_lookup --package phronesis` to see a
  rule invoke a deterministic lookup with end-to-end provenance.
- Read [the visual explainer](https://awaterma.github.io/phronesis/explainer.html)
  for the Alpha, Beta, production, and agenda internals.
- Read the [rule catalogue](https://awaterma.github.io/phronesis/catalogue.html)
  for governance-oriented rules used by `phronesis-mcp`.
