use serde_json::{Value, json};

pub(super) fn python_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "warn-print-in-src",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_print_call": ["?file", "?fn"]},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "print() in ?fn (?file) — consider logging.info()/debug() instead. Remove debug prints before committing. Upstream: style guidance, not a correctness rule."}
            },
            {
                "id": "enforce-no-bare-except",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"python_bare_except": ["?file", "?fn"]}
                ],
                "then": {"block": "Don't use bare `except:` — catch specific exception types. Bare except swallows KeyboardInterrupt and SystemExit. Upstream: PLE0704 (pylint), Bugbear B001."}
            },
            {
                "id": "warn-python-mutable-default-arg",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_mutable_default_arg": ["?file", "?fn", "?param"]}
                ],
                "then": {"warn": "Mutable default `?param` in ?fn — defaults are created once at def time and shared across calls. Use None and create inside. Upstream: Bugbear B006."}
            },
            {
                "id": "audit-python-call-in-default-arg",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"python_call_in_default_arg": ["?file", "?fn", "?param", "?callee"]}
                ],
                "then": {"warn": "Function call `?callee()` in default argument `?param` of ?fn — evaluated once at def time. If it returns a mutable or side-effectful value, this is a bug. Upstream: Bugbear B008 (narrower: only flags call expressions, not bare mutable literals which are covered by B006)."}
            },
            {
                "id": "warn-python-swallowed-exception",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_exception_handler_passes": ["?file", "?fn", "?exception"]}
                ],
                "then": {"warn": "Handler for ?exception in ?fn is empty (`pass`/`...`/comments) — the exception is silently swallowed. Recommend handling, re-raising, or documenting the intentional fallback. Upstream: Bugbear B110 (narrower: typed handlers only; bare handlers caught by enforce-no-bare-except)."}
            },
            {
                "id": "audit-python-high-param-count",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"python_function_param_count_high": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "Function ?fn in ?file has ?count parameters — consider grouping into a config object or using the builder pattern. Exclude self/cls. Upstream: design smell (maintainability, not correctness)."}
            },
            {
                "id": "audit-python-missing-docstring",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"python_function_missing_docstring": ["?file", "?fn"]}
                ],
                "then": {"warn": "Public def ?fn in ?file has no docstring. Upstream: documentation best practice; audit-only because docstring policy is project-dependent."}
            },
            {
                "id": "warn-python-import-time-io",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_import_time_io": ["?file", "?callee"]}
                ],
                "then": {"warn": "Import-time I/O via `?callee` in ?file makes importing the module expensive and fallible even when callers never use that feature. Defer it until first use unless this is a small packaged-data read."}
            },
            {
                "id": "warn-python-is-literal",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_is_literal_comparison": ["?file", "?fn"]}
                ],
                "then": {"warn": "`?fn` in ?file compares a value literal by identity — use `==`/`!=`. Reserve `is` for None, booleans, Ellipsis, and unique sentinel objects."}
            },
            {
                "id": "audit-python-mutated-module-global",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"python_mutated_module_global": ["?file", "?fn", "?global"]}
                ],
                "then": {"warn": "`?fn` in ?file mutates module-level container `?global` — review the resulting cross-call/test coupling and prefer passing state explicitly when practical. This is syntactic evidence and may be a shadowed local."}
            },
            {
                "id": "audit-python-star-import",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"python_star_import": ["?file", "?module"]}
                ],
                "then": {"warn": "?file uses `from ?module import *`, obscuring dependencies and allowing silent name collisions. Import explicit names; package `__init__.py` re-export surfaces are an intentional exception."}
            }
        ]
    })
}

/// Opt-in `python-patterns` pack: design-pattern advisories derived from
/// <https://python-patterns.guide/>. Every rule consumes a tree-sitter
/// predicate from `syntax/python.rs`; none uses substring matching. These
/// are opinionated, so most ship as `warn` or audit-only and every message
/// names the guide page and the limit of the heuristic.
pub(super) fn python_patterns_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "warn-python-global-statement",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_global_statement": ["?file", "?fn", "?name"]}
                ],
                "then": {"warn": "`?fn` in ?file rebinds module global `?name` with `global`. Shared mutable module state couples callers and tests and cannot be instantiated twice. Move the state onto a class and bind its methods to module names explicitly (Prebound Methods: https://python-patterns.guide/python/prebound-methods/; Global Object: https://python-patterns.guide/python/module-globals/). Heuristic limit: syntax only — a `global` used once for lazy initialization of an immutable value is also flagged."}
            },
            {
                "id": "warn-python-globals-introspection-assignment",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_globals_subscript_assignment": ["?file", "?fn"]}
                ],
                "then": {"warn": "`?fn` in ?file assigns through `globals()[...]`. Binding module names by introspection hides them from readers, linters, and IDEs; assign each prebound method explicitly (`random = _instance.random`). See https://python-patterns.guide/python/prebound-methods/. Heuristic limit: any `globals()[...] = ...` store is flagged, including deliberate plugin registries."}
            },
            {
                "id": "warn-python-dynamic-class-creation",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_dynamic_class_creation": ["?file", "?fn"]}
                ],
                "then": {"warn": "`?fn` in ?file builds a class at runtime with `type(name, bases, ns)`. Generated classes are hard to debug, navigate, and type-check; compose independent objects instead. See https://python-patterns.guide/gang-of-four/composition-over-inheritance/. Heuristic limit: flags every three-argument `type(...)` call, including metaclass-style library code that needs it."}
            },
            {
                "id": "warn-python-singleton-new",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_new_override": ["?file", "?class", "singleton"]}
                ],
                "then": {"warn": "Class `?class` in ?file implements the Singleton Pattern by overriding `__new__` around a cached `_instance`. `?class()` then reads like construction but is not, and tests cannot get a fresh instance. Prefer a module-level instance (The Global Object Pattern): https://python-patterns.guide/gang-of-four/singleton/. Heuristic limit: detection keys on an `_instance`/`_instances`/`_singleton` attribute inside `__new__`."}
            },
            {
                "id": "audit-python-custom-new",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"python_new_override": ["?file", "?class", "custom"]}
                ],
                "then": {"warn": "Class `?class` in ?file overrides `__new__`. If this is a Flyweight cache (`Grade(95)` returning a shared object), the guide prefers a plain factory function whose behaviour matches its spelling: https://python-patterns.guide/gang-of-four/flyweight/. Heuristic limit: `__new__` is also the right tool for immutable subclasses (`int`, `tuple`), which this audit cannot distinguish — review, do not rewrite blindly."}
            },
            {
                "id": "audit-python-isinstance-dispatch",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"python_isinstance_chain": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "`?fn` in ?file dispatches on the same value across ?count `isinstance(...)` branches. Type dispatch in the caller is what the Composite Pattern removes: give every object in the hierarchy the same method (leaf objects return an empty result) so callers treat them symmetrically. See https://python-patterns.guide/gang-of-four/composite/. Heuristic limit: only positive `if`/`elif` chains over non-builtin domain types are counted; framework boundary dispatch and functools.singledispatch fallbacks can still match."}
            },
            {
                "id": "warn-python-container-is-own-iterator",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_container_is_own_iterator": ["?file", "?class"]}
                ],
                "then": {"warn": "Container class `?class` in ?file returns `self` from `__iter__` and implements `__next__`, so only one traversal can be in flight at a time (nested `for` loops over the same object break). Return a separate iterator object or write `__iter__` as a generator. See https://python-patterns.guide/gang-of-four/iterator/. Heuristic limit: 'container' means the class also defines `__len__`, `__getitem__`, or `__contains__`; file-like stream objects that intentionally share state are not excluded."}
            },
            {
                "id": "warn-python-multiple-inheritance",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_multiple_inheritance": ["?file", "?class", "?count"]}
                ],
                "then": {"warn": "Class `?class` in ?file inherits from ?count concrete classes. Combining features by multiple inheritance is order-dependent, risks attribute collisions, and needs m×n combination tests; compose the pieces as attributes instead. See https://python-patterns.guide/gang-of-four/composition-over-inheritance/. Heuristic limit: bases named `*Mixin`/`*ABC`, `Protocol`, `Generic`, `NamedTuple`, `TypedDict`, `Enum`, and `metaclass=` are not counted; other abstract bases are."}
            },
            {
                "id": "audit-python-deep-inheritance",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"python_inheritance_depth": ["?file", "?class", "?depth"]}
                ],
                "then": {"warn": "Class `?class` in ?file sits ?depth levels deep in an inheritance chain defined in this file. Deep hierarchies are the 'subclass explosion' the guide warns about; prefer composing small independent classes. See https://python-patterns.guide/gang-of-four/composition-over-inheritance/. Heuristic limit: depth is computed from classes in the same file only, so it understates real depth and says nothing about framework base classes."}
            },
            {
                "id": "warn-python-mixin-with-init",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_mixin_with_init": ["?file", "?class"]}
                ],
                "then": {"warn": "Mixin `?class` in ?file defines `__init__`. A mixin with its own constructor makes cooperative `super().__init__` chains order-dependent and fragile; give the mixin class attributes with defaults, or compose the behaviour as a separate object. See https://python-patterns.guide/gang-of-four/composition-over-inheritance/. Heuristic limit: a mixin is recognised by the `Mixin` name suffix only."}
            },
            {
                "id": "warn-python-static-delegation-wrapper",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_static_delegation_wrapper": ["?file", "?class", "?attr", "?count"]}
                ],
                "then": {"warn": "Class `?class` in ?file re-declares ?count methods that only forward to `self.?attr`. This static Decorator-pattern wrapper must be maintained whenever the wrapped class changes and silently misses methods it never listed. Prefer a dynamic wrapper: `def __getattr__(self, name): return getattr(self.?attr, name)`, overriding only the methods you change. See https://python-patterns.guide/gang-of-four/decorator-pattern/. Heuristic limit: counts single-statement `return self.?attr.<same name>(...)` methods; an adapter that renames methods is not detected."}
            },
            {
                "id": "warn-python-mutable-class-attribute",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_mutable_class_attribute": ["?file", "?class", "?attr"]}
                ],
                "then": {"warn": "Class `?class` in ?file assigns mutable container `?attr` in the class body, so every instance shares one object — the same coupling hazard as a mutable module global. Create it in `__init__` (or use a dataclass `field(default_factory=...)`) unless it is a deliberate class-wide registry. See https://python-patterns.guide/python/module-globals/. Heuristic limit: dunder names are skipped; intentional class-level caches are still flagged."}
            },
            {
                "id": "warn-python-equality-with-none",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_equality_with_none": ["?file", "?fn"]}
                ],
                "then": {"warn": "`?fn` in ?file compares against `None` with `==`/`!=`. `None` is a sentinel and must be tested by identity (`is None` / `is not None`); `==` dispatches to `__eq__`, which NumPy arrays, ORMs, and mocks override. See https://python-patterns.guide/python/sentinel-object/. Upstream: pycodestyle E711."}
            }
        ]
    })
}
