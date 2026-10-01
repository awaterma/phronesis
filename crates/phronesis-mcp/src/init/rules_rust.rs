//! Rust starter-pack rule definitions.
//!
//! The rule bodies below embed the very strings the rules match, so three
//! lexical rules would fire on this file's own text. Each is exempted by
//! name here (all three set `doc_excepted`); structural rules still run.
//!
//! phronesis-allow: audit-newtype-id-string (rule text embeds `_id: String`)
//! phronesis-allow: audit-allow-dead-code-in-src (rule text embeds the allow attribute)
//! phronesis-allow: audit-string-concat-with-plus (rule text embeds the pattern)

use serde_json::{Value, json};

pub(super) fn rust_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "enforce-no-unwrap-in-src",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"rust_governed_invocation": ["?file", "?fn", "unwrap"]},
                    {"file_path_matches": "src"}
                ],
                "then": {"block": "Avoid .unwrap() in src/ — use ? for error propagation, or expect() with a clear message if truly unreachable."}
            },
            {
                "id": "enforce-no-todo-in-src",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"rust_governed_invocation": ["?file", "?fn", "todo"]},
                    {"file_path_matches": "src"}
                ],
                "then": {"block": "Don't ship todo!() in src/ — finish the implementation or split it into a tracked task."}
            },
            {
                "id": "enforce-no-panic-in-src",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"rust_governed_invocation": ["?file", "?fn", "panic"]},
                    {"file_path_matches": "src"}
                ],
                "then": {"block": "Avoid panic!() in src/ — return a Result and let the caller decide."}
            },
            {
                "id": "enforce-no-unimplemented-in-src",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"rust_governed_invocation": ["?file", "?fn", "unimplemented"]},
                    {"file_path_matches": "src"}
                ],
                "then": {"block": "Avoid unimplemented!() in src/ — implement the path or remove it."}
            },
            {
                "id": "enforce-no-result-string-error",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"function_returns_result_string": ["?file", "?fn"]}
                ],
                "then": {"block": "`?fn` in ?file returns Result<_, String>. Define a proper error enum with thiserror."}
            },
            {
                "id": "warn-dbg-in-src",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"rust_governed_invocation": ["?file", "?fn", "dbg"]},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "dbg!() in src/ — remove before committing, or use tracing::debug!() for diagnostics that stay."}
            },
            {
                "id": "warn-rust-public-fn-takes-string-ref",
                "phase": "post",
                "priority": 10,
                "when": [
                    {"function_is_public": ["?file", "?fn"]},
                    {"function_param_type": ["?file", "?fn", "?param", "&String"]}
                ],
                "then": {"warn": "Public `?fn` takes `?param: &String` — prefer `&str` for ergonomics and to avoid forcing callers to own a String."}
            },
            {
                "id": "warn-rust-function-param-count-high",
                "phase": "post",
                "priority": 5,
                "audit": true,
                "when": [
                    {"function_param_count_high": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "Function `?fn` in ?file has ?count parameters. Consider grouping related params into a struct (builder/options pattern) or splitting the function — long signatures correlate with God-function debt."}
            },
            {
                "id": "audit-file-loc-high",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "doc_excepted": true,
                "when": [
                    {"file_extension_is": "rs"},
                    {"file_path_matches": "src"},
                    {"file_line_count_above": "800"}
                ],
                "then": {"warn": "File exceeds 800 lines — consider splitting into focused submodules. Long files correlate with God-object debt and slow down navigation. (Scoped to src/; test blocks excluded from the count; a top-of-file `//! phronesis-allow: audit-file-loc-high <reason>` doc-comment exempts intentional god-files.)"}
            },
            {
                "id": "warn-rust-public-fn-takes-vec-ref",
                "phase": "post",
                "priority": 10,
                "when": [
                    {"function_is_public": ["?file", "?fn"]},
                    {"function_param_is_vec_ref": ["?file", "?fn", "?param"]}
                ],
                "then": {"warn": "Public `?fn` takes `?param: &Vec<T>` — prefer `&[T]` for ergonomics; a slice accepts arrays, slices, and Vecs alike. From the patterns guide §API Design 1."}
            },
            {
                "id": "warn-cargo-build-without-workspace",
                "phase": "pre",
                "priority": 3,
                "when": [
                    {"cargo_command_lacks_workspace": "?cmd"}
                ],
                "then": {"warn": "Running `?cmd` without `--workspace` only checks part of the workspace. Use `cargo <subcommand> --workspace --tests --examples` to catch sibling-crate breakage, or pass `-p <crate>` if scope was intentional."}
            },
            {
                "id": "warn-clone-heavy",
                "phase": "pre",
                "priority": 5,
                "when": [
                    {"function_clone_count_high": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "`?fn` in ?file calls .clone() ?count times — review whether references or borrowed slices would work."}
            },
            {
                "id": "warn-pub-fn-missing-doc",
                "phase": "post",
                "priority": 3,
                "audit": true,
                "when": [
                    {"pub_fn_without_doc_comment": ["?file", "?fn"]},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "Public fn `?fn` in ?file has no doc comment — public items should carry a `///` doc comment explaining what they do (Rust API Guidelines C-DOC)."}
            },
            {
                "id": "warn-empty-test",
                "phase": "post",
                "priority": 5,
                "when": [
                    {"test_without_assertion": ["?file", "?fn"]}
                ],
                "then": {"warn": "Test `?fn` in ?file has no assertions or `?` propagation — a placeholder test that always passes hides regressions."}
            },
            {
                "id": "warn-deref-for-non-pointer-type",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"rust_trait_impl": ["?file", "?type", "Deref"]},
                    {"file_extension_is": "rs"}
                ],
                "then": {"warn": "`impl Deref for` — Deref polymorphism is an anti-pattern for non-pointer types. Reserve Deref for smart-pointer wrappers (Box/Arc/Rc); for other types, prefer explicit delegation methods so the API surface is intentional."}
            },
            {
                "id": "audit-manual-err-return",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"rust_governed_match_arm": ["?file", "?fn", "return_err"]},
                    {"file_extension_is": "rs"}
                ],
                "then": {"warn": "Manual `=> return Err(...)` in a match arm — the `?` operator usually replaces this whole shape. Surface during one-time audit sweeps; deliberately silent at hook time so in-progress refactors aren't blocked."}
            },
            {
                // Intentionally line-oriented: audit's `doc_excepted` contract
                // needs the match location to recognize a field-level `///`.
                "id": "audit-newtype-id-string",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "doc_excepted": true,
                "when": [
                    {"new_content_contains": "_id: String"},
                    {"file_extension_is": "rs"},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "Field named `*_id: String` — consider a newtype like `StateId(String)` for type safety so one ID kind can't be passed where another is expected. From the patterns guide §Design Patterns 2 (Newtype Pattern). (Scoped to src/ — test fixtures are exempt. A `///` doc-comment immediately above the field marks an intentional string ID, e.g. one crossing a JSON registry boundary, as an accepted exception.)"}
            },
            {
                "id": "audit-newtype-id-u64",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"rust_primitive_id_field": ["?file", "?field", "u64"]},
                    {"file_extension_is": "rs"},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "Field named `*_id: u64` — consider a newtype like `UserId(u64)` to prevent mixing different ID types. From the patterns guide §Design Patterns 2 (Newtype Pattern). (Scoped to src/ — test fixtures are exempt.)"}
            },
            {
                "id": "audit-if-let-opportunity-none-empty",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"rust_governed_match_arm": ["?file", "?fn", "none_empty"]},
                    {"file_extension_is": "rs"}
                ],
                "then": {"warn": "`match` with a `None => {}` arm — `if let Some(x) = ...` is usually clearer. From the patterns guide §Idioms 2."}
            },
            {
                "id": "audit-if-let-opportunity-err-empty",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"rust_governed_match_arm": ["?file", "?fn", "err_empty"]},
                    {"file_extension_is": "rs"}
                ],
                "then": {"warn": "`match` arm `Err(_) => {}` silently swallows errors. Either handle the error (log/return) or use `if let Ok(x) = ...` to make the intent explicit."}
            },
            {
                "id": "block-panic-in-drop-impl",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"rust_panic_in_drop": ["?file", "?type", "?construct"]},
                    {"file_extension_is": "rs"}
                ],
                "then": {"block": "Panicking construct (?construct) inside `Drop::drop` for `?type` — a panic that occurs during unwind inside Drop::drop aborts the whole process (std::process::abort) rather than failing gracefully. Log and swallow the error in drop() instead of panicking."}
            },
            {
                "id": "block-deny-warnings-attribute",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"rust_governed_attribute": ["?file", "deny_warnings"]},
                    {"file_extension_is": "rs"}
                ],
                "then": {"block": "`#![deny(warnings)]` breaks builds on toolchain upgrades, since each rustc release introduces new warnings. Move the policy to CI with `RUSTFLAGS=\"-D warnings\"` instead. From the patterns guide §Anti-patterns (deny-warnings)."}
            },
            {
                "id": "warn-public-fn-takes-box-ref",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"function_param_is_box_ref": ["?file", "?fn", "?param"]},
                    {"file_extension_is": "rs"}
                ],
                "then": {"warn": "Parameter type `&Box<T>` adds a useless layer of indirection — prefer `&T` directly. From the patterns guide §Idioms (borrowed-types-for-arguments)."}
            },
            {
                "id": "warn-expect-with-empty-message",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"rust_governed_invocation": ["?file", "?fn", "expect_empty"]},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "`.expect(\"\")` is strictly worse than `.unwrap()` — same panic, no explanation of the invariant. Either supply a real message or use `.unwrap()` and let the existing rule flag it."}
            },
            {
                "id": "audit-rc-refcell-in-src",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"rust_rc_refcell_type": ["?file", "?count"]},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "`Rc<RefCell<T>>` is the textbook 'fighting the borrow checker' shape — often a signal that an arena, index-based references, or a redesigned ownership model would be a better fit. Confirm intent."}
            },
            {
                // Intentionally lexical: Rust syntax alone cannot prove the
                // operands have the String/&str types this policy describes.
                "id": "audit-string-concat-with-plus",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "doc_excepted": true,
                "when": [
                    {"new_content_contains": "\" + &"},
                    {"file_extension_is": "rs"}
                ],
                "then": {"warn": "String concatenation with `\" + &` — prefer `format!(\"{}{}\", a, b)` for readability and to avoid intermediate allocations. From the patterns guide §Idioms (concat-format)."}
            },
            {
                // Intentionally line-oriented until AST facts carry spans:
                // `doc_excepted` must inspect the item's preceding `///`.
                "id": "audit-allow-dead-code-in-src",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "doc_excepted": true,
                "when": [
                    {"new_content_contains": "#[allow(dead_code)]"},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "`#[allow(dead_code)]` in src/ — either delete the code or add a `///` doc-comment immediately above explaining why it's kept (planned API, generic-constraint trick, intentional placeholder). Documented exceptions are not flagged."}
            },
            {
                "id": "audit-env-set-var-in-src",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"rust_governed_invocation": ["?file", "?fn", "env_set_var"]},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "`env::set_var(` in src/ — mutating process environment variables is unsound under concurrent reads (which is why edition 2024 marks the call unsafe). Verify the call site is genuinely single-threaded, or refactor to pass configuration explicitly through function arguments / a context struct. Tests where you control the thread count are usually fine; library code almost never is."}
            },
            {
                "id": "audit-rust-let-binding-count-high",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "doc_excepted": true,
                "when": [
                    {"file_path_matches": "src"},
                    {"function_let_binding_count_high": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "`?fn` in ?file has ?count outer-scope `let` bindings — consider scoping intermediate temporaries into a block (`let result = { let raw = ...; let parsed = ...; ... }`) so only the final value is visible to the rest of the function. Block pattern: John Nunley, 'Rust's Block Pattern' (Dec 2025). (Scoped to src/ — examples/benches/tests are not production code and are exempt.)"}
            },
            {
                "id": "audit-rust-let-mut-count-high",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "doc_excepted": true,
                "when": [
                    {"file_path_matches": "src"},
                    {"function_let_mut_count_high": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "`?fn` in ?file has ?count outer-scope `let mut` declarations — consider John Nunley's block pattern: wrap the mutation in `let x = { let mut tmp = ...; ...; tmp }` so the surrounding scope sees an immutable binding. Block pattern: John Nunley, 'Rust's Block Pattern' (Dec 2025). (Scoped to src/ — examples/benches/tests are not production code and are exempt.)"}
            },
            {
                "id": "audit-rust-sync-lock-across-await",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"rust_sync_lock_guard_across_await": ["?file", "?fn", "?guard"]}
                ],
                "then": {"warn": "`?fn` in ?file keeps std synchronization guard `?guard` in scope across `.await` — close the guard's lexical scope before awaiting. A synchronous Mutex/RwLock can block the executor and often makes the future non-Send."}
            },
            {
                "id": "audit-rust-unsafe-without-safety-comment",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"rust_unsafe_without_safety_comment": ["?file", "?fn"]}
                ],
                "then": {"warn": "Unsafe block in `?fn` (?file) has no nearby `SAFETY:` explanation — document the invariant that makes the operation sound so reviewers can verify it."}
            },
            {
                "id": "warn-rust-blocking-call-in-async",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"rust_async_blocking_call": ["?file", "?fn", "?callee"]}
                ],
                "then": {"warn": "Async function `?fn` in ?file calls blocking `?callee` directly — use the async runtime's equivalent or isolate the operation with `spawn_blocking`."}
            }
        ]
    })
}
