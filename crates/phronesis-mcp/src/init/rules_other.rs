use serde_json::{Value, json};

/// Rhai-specific rules. Apply to projects that embed the Rhai scripting
/// language, whether via the `rhai` crate from Rust or as standalone `.rhai`
/// scripts. Messages are intentionally generic; project-specific guidance
/// (which loader helper to call, which response-proxy to use, etc.) should
/// be layered in via project-local rules in `.phronesis/rules.json`.
pub(crate) fn rhai_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "block-rhai-inline-eval-string",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"engine_eval_string_literal": ["?file", "?fn"]},
                    {"file_extension_is": "rs"}
                ],
                "then": {"block": "`?fn` in ?file calls `engine.eval(<string literal>)`. Inline string-eval can't be tested independently of the surrounding Rust code and bypasses any script registry. Move the script to a `.rhai` file and load it via `engine.compile_file(...)` (or `compile(...)` on `include_str!`-ed content) so the AST can be cached, values-checked at build time, and exercised in isolation."}
            },
            {
                // Intentionally lexical until a Rhai parser-backed producer
                // exists; adding one solely for this rule is out of scope.
                "id": "block-rhai-print-in-script",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"new_content_contains": "print("},
                    {"file_extension_is": "rhai"}
                ],
                "then": {"block": "`print(` appears in a .rhai script. `print` is Rhai's equivalent of `dbg!()` — debug output that bypasses whatever response/logging channel your host has registered. Use the host-registered function for emitting output (commonly a `log`, `emit`, or `response_*` proxy your `Engine` exposes via `register_fn`) so script output flows through the same path as the rest of your application."}
            }
        ]
    })
}

/// Structural rules over the code graph (`docs/specs/SPEC-triple-store-rete.md`).
///
/// Both rules read relations supplied by `graph::hydrate`, so they only fire
/// once `.phronesis/graph.jsonl` exists — run `phr-mcp graph rebuild` after
/// adding this pack. Until then hydration finds no edges and the rules are
/// silent rather than wrong.
///
/// Both **warn** rather than block. Measured on this repository (spec §10),
/// `in_cycle` was 2/2 genuine and the untested-risky-call join fired 6 times
/// with a hand-audited true positive — precise enough to surface, not yet
/// measured on a second corpus, which is what promotion to `block` requires.
///
/// Both rules set `audit: true`. The file-scanning audit engine understands
/// only content and path predicates, so `phr-mcp audit` routes graph rules
/// through `graph::audit::audit_graph_rules` and folds the findings in with
/// `audit::merge_graph_hits`. Without opting in they would be filtered out
/// before that step, and a clean audit would mean "never checked" rather than
/// "nothing found".
///
/// Both rules open on `edited_file`, which scopes them to the file in front
/// of the user. Graph relations describe the whole repository, so without
/// that join a rule re-reports every violation in the project on every single
/// edit — the fastest way to get a pack switched off.
pub(crate) fn structural_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "warn-untested-risky-call",
                "phase": "pre",
                "priority": 20,
                "audit": true,
                "when": [
                    {"edited_file": "?file"},
                    {"file_type": ["?file", "production"]},
                    {"defines_fn": ["?file", "?func"]},
                    {"or": [
                        {"calls_api": ["?func", "unwrap"]},
                        {"calls_api": ["?func", "expect"]},
                        {"calls_api": ["?func", "panic"]},
                        {"calls_api": ["?func", "todo"]},
                        {"calls_api": ["?func", "unimplemented"]}
                    ]},
                    {"no_direct_test": ["?func"]}
                ],
                "then": {"warn": "`?func` (in ?file) calls a panicking API from the unwrap/expect/panic/todo/unimplemented family, and no test calls it directly. A panicking path with no test is where a crash reaches a user unnoticed — add a test that exercises this function, or handle the failure case explicitly. Coverage is matched by direct call only, so a function covered transitively may still be flagged."}
            },
            {
                "id": "warn-import-cycle",
                "phase": "pre",
                "priority": 20,
                "audit": true,
                "when": [
                    {"edited_file": "?file"},
                    {"declares_module": ["?file", "?module"]},
                    {"in_cycle": ["?module", "?cycle"]}
                ],
                "then": {"warn": "Module `?module` is part of import cycle `?cycle`. Mutually importing modules can't be understood, tested, or extracted independently, and the cycle tends to attract more coupling over time. Move the shared items into a third module both can depend on."}
            },
            {
                "id": "warn-ts-untested-risky-call",
                "phase": "pre",
                "priority": 20,
                "audit": true,
                "when": [
                    {"edited_file": "?file"},
                    {"file_type": ["?file", "production"]},
                    {"defines_fn": ["?file", "?func"]},
                    {"calls_api": ["?func", "non_null_assertion"]},
                    {"no_direct_test": ["?func"]}
                ],
                "then": {"warn": "`?func` uses a non-null assertion (`!`) and has no direct test. `!` tells the compiler a value cannot be null and produces no runtime check, so when the assumption is wrong the failure surfaces later and elsewhere, usually as a TypeError. Add a test that exercises the null case, or narrow the type so the assertion is unnecessary."}
            },
            {
                "id": "warn-unconsumed-config-key",
                "phase": "pre",
                "priority": 20,
                "audit": true,
                "when": [
                    {"edited_file": "?file"},
                    {"declares_module": ["?file", "?artifact"]},
                    {"unconsumed_data_key": ["?artifact", "?pointer", "?consumer"]}
                ],
                "then": {"warn": "Generated configuration key `?pointer` in ?artifact has no accepted field on ?consumer and may be silently dropped. Rename or alias the field, remove the key, or make intentional extra-field handling explicit."}
            },
            {
                "id": "warn-generated-artifact-diagnostic",
                "phase": "pre",
                "priority": 20,
                "audit": true,
                "when": [
                    {"edited_file": ".phronesis/graph.toml"},
                    {"generated_artifact_diagnostic": ["?kind", "?reference"]}
                ],
                "then": {"warn": "Generated-artifact binding has diagnostic `?kind` for `?reference`; no cross-language seam was guessed. Correct the exact producer, artifact, or consumer reference."}
            }
        ]
    })
}

pub(crate) fn typescript_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "warn-console-log-in-src",
                "phase": "pre",
                "priority": 5,
                "when": [
                    {"ts_console_log_call": ["?file", "?fn", "?count"]},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "console.log in src/ — remove before committing, or use a proper logger."}
            },
            {
                "id": "warn-ts-explicit-any-ast",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"ts_explicit_any": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "?count explicit `any` annotation(s) in ?fn (?file) — narrow the type, or use `unknown` and refine with type guards."}
            },
            {
                "id": "warn-ts-suppression-comment",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"ts_suppression_comment": ["?file", "?count"]}
                ],
                "then": {"warn": "?count @ts-ignore/@ts-expect-error/@ts-nocheck comment(s) in ?file — each one turns the type checker off somewhere. Fix the type instead."}
            },
            {
                "id": "audit-ts-non-null-assertion",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"ts_non_null_assertion": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "?count non-null assertion(s) (`x!`) in ?fn (?file) — prefer explicit narrowing or optional chaining."}
            },
            {
                "id": "warn-ts-function-param-count-high",
                "phase": "post",
                "priority": 5,
                "audit": true,
                "when": [
                    {"ts_function_param_count_high": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "Function `?fn` in ?file has ?count parameters. Consider grouping related params into an options object or splitting the function — long signatures correlate with God-function debt."}
            }
        ]
    })
}

pub(crate) fn swift_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "warn-swift-force-unwrap",
                "phase": "post",
                "priority": 10,
                "when": [
                    {"function_uses_force_unwrap": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "Function `?fn` in ?file uses ?count force-unwrap(s). Prefer guard let or if let; reserve ! for invariants you can document."}
            },
            {
                "id": "warn-swift-throws-with-force-unwrap",
                "phase": "post",
                "priority": 5,
                "audit": true,
                "when": [
                    {"function_throws": ["?file", "?fn"]},
                    {"function_uses_force_unwrap": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "`?fn` in ?file is declared `throws` yet force-unwraps ?count time(s). Since the function can already propagate failure, replace the `!` with `guard let ... else { throw ... }` so callers get an error instead of a crash."}
            },
            {
                "id": "warn-swift-try-bang",
                "phase": "pre",
                "priority": 5,
                "when": [
                    {"swift_governed_construct": ["?file", "?fn", "try_force"]},
                    {"file_extension_is": "swift"}
                ],
                "then": {"warn": "try! crashes on error — prefer try with do/catch, or try? when an Optional result is acceptable."}
            },
            {
                "id": "warn-swift-force-cast",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"swift_governed_construct": ["?file", "?fn", "force_cast"]},
                    {"file_extension_is": "swift"}
                ],
                "then": {"warn": "Force-cast `as!` crashes on type mismatch — prefer `as?` with `if let`/`guard let`. Completes the force-bang trio with `!` and `try!`."}
            },
            {
                "id": "audit-swift-fatal-error",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"swift_governed_construct": ["?file", "?fn", "fatal_error"]},
                    {"file_extension_is": "swift"}
                ],
                "then": {"warn": "`fatalError(` aborts the process — for recoverable conditions prefer a `throws` API and let the caller decide. Reserve `fatalError` for genuinely unreachable invariants (and prefer `precondition`/`assertionFailure` when the intent is a debug-only trap)."}
            },
            {
                "id": "audit-swift-mutable-singleton",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"swift_governed_construct": ["?file", "?fn", "mutable_singleton"]},
                    {"file_extension_is": "swift"}
                ],
                "then": {"warn": "`static var shared` is a mutable global — the Singleton pattern (eleev/swift-design-patterns §Creational/Singleton) uses `static let shared` so the instance can't be swapped at runtime. If mutability is intentional, add a comment or move state inside the instance."}
            },
            {
                "id": "audit-swift-legacy-constructor",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"swift_governed_construct": ["?file", "?fn", "legacy_constructor"]},
                    {"file_extension_is": "swift"}
                ],
                "then": {"warn": "Legacy C-style constructor — prefer the modern Swift initializer (e.g. `CGRect(x:y:width:height:)`, `UIEdgeInsets(top:left:bottom:right:)`). Mirrors SwiftLint's `legacy_constructor` rule."}
            },
            {
                "id": "audit-swift-legacy-random",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"swift_governed_construct": ["?file", "?fn", "legacy_random"]},
                    {"file_extension_is": "swift"}
                ],
                "then": {"warn": "Legacy random API — Swift 4.2+ ships `Int.random(in:)`, `Double.random(in:)`, and `Collection.randomElement()`, which work on all platforms (not just Darwin) and are uniformly distributed without modulo bias. Mirrors SwiftLint's `legacy_random` rule."}
            }
        ]
    })
}

/// Lua language pack rules.
///
/// Warning-first set of syntax rules (spec §Starter pack).
/// Primary value is graph participation — mixed repos can query Lua
/// definitions, tests, and imports alongside every other language.
pub(crate) fn lua_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "warn-lua-dynamic-code-load",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"lua_dynamic_code_load": ["?file", "?module", "?loader"]},
                    {"file_extension_is": "lua"}
                ],
                "then": {
                    "warn": "Dynamic code load (`?loader`) in Lua module `?module` — dynamic code loading bypasses static analysis and may execute untrusted code. Prefer pre-loaded modules via `require`."
                }
            }
        ]
    })
}

/// CUE language pack rules.
///
/// Low-noise starter set (spec §Starter pack). CUE is a constraint language;
/// graph claims must use its semantics rather than force it into an imperative
/// language shape. Import diagnostics are emitted only after repository-wide
/// package indexing and never invent a dependency edge.
pub(crate) fn cue_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "warn-cue-import-diagnostic",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"edited_file": "?file"},
                    {"cue_import_diagnostic": ["?file", "?import", "?kind"]}
                ],
                "then": {"warn": "CUE import `?import` in ?file is `?kind`; no repository-local dependency edge was guessed. Add a package qualifier or correct the module/package path."}
            }
        ]
    })
}

/// JSON language pack rules.
///
/// Syntax-safe, low-noise starter rules. Arbitrary application JSON has no
/// import or module system, so rules only fire when schema keywords identify
/// a document as a JSON Schema resource (spec §Starter pack).
pub(crate) fn json_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "audit-json-schema-unknown-dialect",
                "phase": "audit",
                "priority": 5,
                "audit": true,
                "when": [
                    {"json_schema_unknown_dialect": ["?file", "?module", "?dialect"]}
                ],
                "then": {"warn": "JSON Schema file ?file declares `$schema` as `?$dialect` which is not a supported dialect. Only recognized dialects receive schema resolution support — check the `$schema` value or file is not a JSON Schema."}
            }
        ]
    })
}

/// YAML language pack rules.
///
/// Conservative, syntax-safe starter rules. Generic YAML has no cross-file
/// import system, so rules focus on anchors, aliases, and unsafe tags
/// (spec §Starter pack).
pub(crate) fn yaml_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "block-yaml-duplicate-key",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"yaml_duplicate_key": ["?file", "?key", "?line"]}
                ],
                "then": {"warn": "Duplicate key `?key` at line ?line in ?file — YAML consumers may silently choose one value or reject the document. Merge the conflicting keys into a single entry."}
            },
            {
                "id": "block-yaml-invalid-alias",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"yaml_undefined_alias": ["?file", "?module", "?alias"]}
                ],
                "then": {"warn": "YAML alias `?alias` in module ?module (?file) is undefined — the anchor has not been defined earlier in this document. Define the anchor before using it."}
            },
            {
                "id": "warn-yaml-legacy-merge-key",
                "phase": "post",
                "priority": 10,
                "when": [
                    {"yaml_merge_key": ["?file", "?line"]}
                ],
                "then": {"warn": "Legacy merge key `<<` at line ?line in ?file — the YAML merge key is a legacy feature from YAML 1.1 that can produce unexpected results with complex mappings. Consider using explicit key duplication or a dedicated configuration format."}
            }
        ]
    })
}

/// Helm 3 language pack rules.
///
/// High-value chart defect detection without invoking a cluster (spec §Starter pack).
/// Rules target template source files under valid chart `templates/` directories.
pub(crate) fn helm3_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "audit-helm3-tpl",
                "phase": "audit",
                "priority": 5,
                "audit": true,
                "when": [
                    {"helm3_dynamic_tpl": ["?file", "?module", "?tpl_call"]}
                ],
                "then": {"warn": "Dynamic template evaluation `?tpl_call` in ?file — the template name is computed at render time, so the static graph cannot determine which template will be invoked. Verify the dynamic call resolves correctly at render time."}
            },
            {
                "id": "audit-helm3-lookup",
                "phase": "audit",
                "priority": 5,
                "audit": true,
                "when": [
                    {"helm3_cluster_lookup": ["?file", "?module"]}
                ],
                "then": {"warn": "Helm template in ?file uses `lookup` — render depends on live cluster state and cannot be fully validated offline. Use `helm template --dry-run` with a test cluster to verify behavior."}
            }
        ]
    })
}
