//! Tree-sitter Python analyzer. Per-predicate extractors run over the parsed
//! Python AST and populate `SyntaxFacts`. Code outside any `def` is reported
//! under the pseudo-function name `<module>`.

use super::facts::SyntaxFacts;
use super::parsed::ParsedFile;
use tree_sitter::Node;

/// Parameter-count threshold: a `def` with this many parameters or more is
/// flagged. `self` / `cls` first parameters are not counted.
const PARAM_COUNT_THRESHOLD: usize = 6;

/// Top-level entry. Parses once, then runs every predicate extractor.
pub fn extract(content: &str) -> SyntaxFacts {
    let Some(parsed) = ParsedFile::parse_python(content) else {
        return SyntaxFacts::default();
    };
    let ParsedFile::Python { tree, source } = &parsed else {
        return SyntaxFacts::default();
    };
    let root = tree.root_node();
    let src = source.as_bytes();

    SyntaxFacts {
        python_bare_excepts: extract_bare_excepts(root, src),
        python_mutable_default_args: extract_mutable_defaults(root, src),
        python_function_param_counts_high: extract_param_counts_high(root, src),
        python_functions_missing_docstring: extract_missing_docstrings(root, src),
        python_print_calls: extract_print_calls(root, src),
        python_call_in_default_args: extract_call_in_default_args(root, src),
        python_exception_handler_passes: extract_handler_passes(root, src),
        python_import_time_io: extract_import_time_io(root, src),
        python_is_literal_comparisons: extract_is_literal_comparisons(root, src),
        python_mutated_module_globals: extract_mutated_module_globals(root, src),
        python_star_imports: extract_star_imports(root, src),
        python_global_statements: extract_global_statements(root, src),
        python_globals_subscript_assignments: extract_globals_subscript_assignments(root, src),
        python_dynamic_class_creations: extract_dynamic_class_creations(root, src),
        python_new_overrides: extract_new_overrides(root, src),
        python_isinstance_chains: extract_isinstance_chains(root, src),
        python_containers_own_iterator: extract_containers_own_iterator(root, src),
        python_multiple_inheritance: extract_multiple_inheritance(root, src),
        python_inheritance_depths: extract_inheritance_depths(root, src),
        python_mixins_with_init: extract_mixins_with_init(root, src),
        python_static_delegation_wrappers: extract_static_delegation_wrappers(root, src),
        python_mutable_class_attributes: extract_mutable_class_attributes(root, src),
        python_equality_with_none: extract_equality_with_none(root, src),
        ..SyntaxFacts::default()
    }
}

// ─── python-patterns.guide predicates ───────────────────────────────────
//
// These feed the opt-in `python-patterns` pack. Each is a syntactic
// heuristic for a shape the guide argues against; the rule messages in
// `init.rs` carry the guide URL and the limits of each heuristic.

fn text<'a>(node: Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

/// Every `class_definition` with its name, in source order.
fn classes<'a>(root: Node<'a>, src: &[u8]) -> Vec<(String, Node<'a>)> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() == "class_definition"
            && let Some(name) = node.child_by_field_name("name")
        {
            out.push((text(name, src).to_string(), node));
        }
    });
    out
}

/// Direct methods of a class body, unwrapping `decorated_definition`.
fn class_methods<'a>(class: Node<'a>, src: &[u8]) -> Vec<(String, Node<'a>)> {
    let Some(body) = class.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = body.walk();
    for stmt in body.named_children(&mut cursor) {
        let def = match stmt.kind() {
            "function_definition" => Some(stmt),
            "decorated_definition" => stmt.child_by_field_name("definition"),
            _ => None,
        };
        if let Some(def) = def
            && def.kind() == "function_definition"
            && let Some(name) = def.child_by_field_name("name")
        {
            out.push((text(name, src).to_string(), def));
        }
    }
    out
}

/// Base-class expressions of a class (positional entries of the
/// `superclasses` argument list; `metaclass=` keywords are skipped).
fn class_bases<'a>(class: Node<'a>) -> Vec<Node<'a>> {
    let Some(supers) = class.child_by_field_name("superclasses") else {
        return Vec::new();
    };
    let mut cursor = supers.walk();
    supers
        .named_children(&mut cursor)
        .filter(|c| !matches!(c.kind(), "keyword_argument" | "comment"))
        .collect()
}

/// Guide: Prebound Methods / Global Object — a function that rebinds a
/// module global with `global` is the shape the guide replaces with an
/// instance plus explicitly prebound methods.
fn extract_global_statements(root: Node, src: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "global_statement" {
            return;
        }
        let fn_name = enclosing_function_name(node, src);
        if fn_name == "<module>" {
            return;
        }
        let mut cursor = node.walk();
        for name in node.named_children(&mut cursor) {
            if name.kind() == "identifier" {
                out.push((fn_name.clone(), text(name, src).to_string()));
            }
        }
    });
    out
}

/// Guide: Prebound Methods — `globals()[name] = ...` introspection loops.
fn extract_globals_subscript_assignments(root: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if !matches!(node.kind(), "assignment" | "augmented_assignment") {
            return;
        }
        let Some(left) = node.child_by_field_name("left") else {
            return;
        };
        if left.kind() != "subscript" {
            return;
        }
        let is_globals_call = left
            .child_by_field_name("value")
            .filter(|v| v.kind() == "call")
            .and_then(|v| v.child_by_field_name("function"))
            .is_some_and(|f| f.kind() == "identifier" && text(f, src) == "globals");
        if is_globals_call {
            out.push(enclosing_function_name(node, src));
        }
    });
    out
}

/// Guide: Composition Over Inheritance — `type(name, bases, ns)` builds
/// classes at runtime, which the guide calls undebuggable.
fn extract_dynamic_class_creations(root: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "call" {
            return;
        }
        let Some(func) = node.child_by_field_name("function") else {
            return;
        };
        if func.kind() != "identifier" || text(func, src) != "type" {
            return;
        }
        let Some(args) = node.child_by_field_name("arguments") else {
            return;
        };
        let mut cursor = args.walk();
        let positional = args
            .named_children(&mut cursor)
            .filter(|a| !matches!(a.kind(), "keyword_argument" | "comment"))
            .count();
        if positional == 3 {
            out.push(enclosing_function_name(node, src));
        }
    });
    out
}

/// Guide: Singleton / Flyweight — `__new__` overrides. A body that touches
/// an `_instance`-style attribute is classified `singleton`; any other
/// `__new__` (e.g. a flyweight cache keyed by argument) is `custom`.
fn extract_new_overrides(root: Node, src: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (class_name, class) in classes(root, src) {
        for (method, def) in class_methods(class, src) {
            if method != "__new__" {
                continue;
            }
            let mut singleton = false;
            walk(def, &mut |n| {
                if n.kind() == "attribute"
                    && let Some(attr) = n.child_by_field_name("attribute")
                {
                    let name = text(attr, src).trim_start_matches('_');
                    if matches!(name, "instance" | "instances" | "singleton") {
                        singleton = true;
                    }
                }
            });
            let shape = if singleton { "singleton" } else { "custom" };
            out.push((class_name.clone(), shape.to_string()));
        }
    }
    out
}

/// Guide: Composite — `if isinstance(x, A)` / `elif isinstance(x, B)` chains
/// that dispatch on the same value's domain type instead of relying on a
/// symmetric interface. Independent guards, negative validation, and checks
/// against built-in scalar/container types are deliberately excluded.
fn extract_isinstance_chains(root: Node, src: &[u8]) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "function_definition" {
            return;
        }
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let function_name = text(name, src);
        walk(node, &mut |n| {
            if n.kind() != "if_statement" {
                return;
            }
            // Count conditions belonging to this def, not nested defs.
            if enclosing_function_name(n, src) != function_name {
                return;
            }

            let Some((subject, _)) = n
                .child_by_field_name("condition")
                .and_then(|condition| isinstance_domain_check(condition, src))
            else {
                return;
            };

            let mut count = 1usize;
            let mut cursor = n.walk();
            for alternative in n.named_children(&mut cursor) {
                if alternative.kind() != "elif_clause" {
                    continue;
                }
                if alternative
                    .child_by_field_name("condition")
                    .and_then(|condition| isinstance_domain_check(condition, src))
                    .is_some_and(|(alternative_subject, _)| alternative_subject == subject)
                {
                    count += 1;
                }
            }

            if count >= 2 {
                out.push((function_name.to_string(), count));
            }
        });
    });
    out
}

fn isinstance_domain_check(condition: Node, src: &[u8]) -> Option<(String, String)> {
    if condition.kind() != "call"
        || !condition
            .child_by_field_name("function")
            .is_some_and(|function| {
                function.kind() == "identifier" && text(function, src) == "isinstance"
            })
    {
        return None;
    }

    let arguments = condition.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let positional: Vec<Node> = arguments
        .named_children(&mut cursor)
        .filter(|argument| argument.kind() != "keyword_argument")
        .collect();
    if positional.len() != 2 || !is_domain_type_expression(positional[1], src) {
        return None;
    }

    Some((
        text(positional[0], src).trim().to_string(),
        text(positional[1], src).trim().to_string(),
    ))
}

fn is_domain_type_expression(node: Node, src: &[u8]) -> bool {
    match node.kind() {
        "identifier" => !is_builtin_validation_type(text(node, src)),
        "attribute" => true,
        "tuple" => {
            let mut cursor = node.walk();
            let types: Vec<Node> = node.named_children(&mut cursor).collect();
            !types.is_empty()
                && types
                    .into_iter()
                    .all(|candidate| is_domain_type_expression(candidate, src))
        }
        _ => false,
    }
}

fn is_builtin_validation_type(name: &str) -> bool {
    matches!(
        name,
        "bool"
            | "bytes"
            | "bytearray"
            | "complex"
            | "dict"
            | "float"
            | "frozenset"
            | "int"
            | "list"
            | "memoryview"
            | "object"
            | "range"
            | "set"
            | "str"
            | "tuple"
            | "type"
    )
}

fn returns_only_self(def: Node, src: &[u8]) -> bool {
    let Some(body) = def.child_by_field_name("body") else {
        return false;
    };
    let mut cursor = body.walk();
    let stmts: Vec<Node> = body
        .named_children(&mut cursor)
        .filter(|s| s.kind() != "comment")
        .filter(|s| {
            !(s.kind() == "expression_statement"
                && s.child(0).is_some_and(|c| c.kind() == "string"))
        })
        .collect();
    stmts.len() == 1
        && stmts[0].kind() == "return_statement"
        && stmts[0]
            .named_child(0)
            .is_some_and(|v| v.kind() == "identifier" && text(v, src) == "self")
}

/// Guide: Iterator — a container whose `__iter__` returns `self` can only
/// support one traversal at a time. A class counts as a container when it
/// also implements `__len__`, `__getitem__`, or `__contains__`; pure
/// iterator classes (which legitimately return `self`) are excluded.
fn extract_containers_own_iterator(root: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for (class_name, class) in classes(root, src) {
        let methods = class_methods(class, src);
        let has = |n: &str| methods.iter().any(|(m, _)| m == n);
        let iter_self = methods
            .iter()
            .any(|(m, def)| m == "__iter__" && returns_only_self(*def, src));
        let container = has("__len__") || has("__getitem__") || has("__contains__");
        if iter_self && has("__next__") && container {
            out.push(class_name);
        }
    }
    out
}

fn base_is_concrete(base: Node, src: &[u8]) -> bool {
    let base_text = text(base, src);
    // `typing.Generic[T]` -> `Generic`; `abc.ABC` -> `ABC`.
    let head = base_text.split('[').next().unwrap_or(base_text);
    let leaf = head.rsplit('.').next().unwrap_or(head);
    if leaf.ends_with("Mixin") || leaf.ends_with("ABC") || leaf == "object" {
        return false;
    }
    !matches!(
        leaf,
        "Protocol" | "Generic" | "ABCMeta" | "NamedTuple" | "TypedDict" | "Enum"
    )
}

/// Guide: Composition Over Inheritance — combining two or more concrete
/// classes by multiple inheritance. Mixins, ABCs, Protocols, Generic, and
/// `metaclass=` keywords are not counted.
fn extract_multiple_inheritance(root: Node, src: &[u8]) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for (class_name, class) in classes(root, src) {
        let concrete = class_bases(class)
            .into_iter()
            .filter(|b| base_is_concrete(*b, src))
            .count();
        if concrete >= 2 {
            out.push((class_name, concrete));
        }
    }
    out
}

/// File-local inheritance depth: a class with no known local base has
/// depth one. Bases defined in other modules are invisible, so this
/// understates depth and never overstates it.
fn extract_inheritance_depths(root: Node, src: &[u8]) -> Vec<(String, usize)> {
    const DEPTH_THRESHOLD: usize = 3;
    let all = classes(root, src);
    let mut depth: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (class_name, class) in &all {
        let d = class_bases(*class)
            .into_iter()
            .filter_map(|b| depth.get(text(b, src)).copied())
            .max()
            .unwrap_or(0)
            + 1;
        depth.insert(class_name.clone(), d);
    }
    all.into_iter()
        .filter_map(|(name, _)| {
            let d = depth.get(&name).copied().unwrap_or(1);
            (d >= DEPTH_THRESHOLD).then_some((name, d))
        })
        .collect()
}

/// Guide: Composition Over Inheritance — a mixin with `__init__` makes
/// cooperative construction order-dependent and fragile.
fn extract_mixins_with_init(root: Node, src: &[u8]) -> Vec<String> {
    classes(root, src)
        .into_iter()
        .filter(|(name, class)| {
            name.ends_with("Mixin")
                && class_methods(*class, src)
                    .iter()
                    .any(|(m, _)| m == "__init__")
        })
        .map(|(name, _)| name)
        .collect()
}

/// If `def` is `def name(self, ...): return self.<attr>.name(...)`, return
/// `attr`.
fn pure_delegation_target<'a>(method: &str, def: Node, src: &'a [u8]) -> Option<&'a str> {
    let body = def.child_by_field_name("body")?;
    let mut cursor = body.walk();
    let stmts: Vec<Node> = body
        .named_children(&mut cursor)
        .filter(|s| s.kind() != "comment")
        .collect();
    if stmts.len() != 1 || stmts[0].kind() != "return_statement" {
        return None;
    }
    let call = stmts[0].named_child(0)?;
    if call.kind() != "call" {
        return None;
    }
    let func = call.child_by_field_name("function")?;
    if func.kind() != "attribute" {
        return None;
    }
    if text(func.child_by_field_name("attribute")?, src) != method {
        return None;
    }
    let inner = func.child_by_field_name("object")?;
    if inner.kind() != "attribute" {
        return None;
    }
    let receiver = inner.child_by_field_name("object")?;
    if receiver.kind() != "identifier" || text(receiver, src) != "self" {
        return None;
    }
    Some(text(inner.child_by_field_name("attribute")?, src))
}

/// Guide: Decorator Pattern — a static wrapper that re-declares every
/// method just to forward it. Counts pure `return self.<attr>.m(...)`
/// methods; classes that already define `__getattr__` are dynamic wrappers
/// and are excluded.
fn extract_static_delegation_wrappers(root: Node, src: &[u8]) -> Vec<(String, String, usize)> {
    const DELEGATION_THRESHOLD: usize = 4;
    let mut out = Vec::new();
    for (class_name, class) in classes(root, src) {
        let methods = class_methods(class, src);
        if methods.iter().any(|(m, _)| m == "__getattr__") {
            continue;
        }
        let mut per_attr: std::collections::BTreeMap<&str, usize> = Default::default();
        for (method, def) in &methods {
            if let Some(attr) = pure_delegation_target(method, *def, src) {
                *per_attr.entry(attr).or_insert(0) += 1;
            }
        }
        if let Some((attr, count)) = per_attr.into_iter().max_by_key(|(_, c)| *c)
            && count >= DELEGATION_THRESHOLD
        {
            out.push((class_name, attr.to_string(), count));
        }
    }
    out
}

fn is_mutable_container_expr(value: Node, src: &[u8]) -> bool {
    match value.kind() {
        "list"
        | "dictionary"
        | "set"
        | "list_comprehension"
        | "dictionary_comprehension"
        | "set_comprehension" => true,
        "call" => value.child_by_field_name("function").is_some_and(|f| {
            matches!(
                text(f, src),
                "list" | "dict" | "set" | "defaultdict" | "OrderedDict" | "deque" | "bytearray"
            )
        }),
        _ => false,
    }
}

/// Guide: Global Object — a mutable container assigned in a class body is
/// one object shared by every instance, which is the same coupling hazard
/// as a mutable module global. Dunder names (`__slots__`) are skipped.
fn extract_mutable_class_attributes(root: Node, src: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (class_name, class) in classes(root, src) {
        let Some(body) = class.child_by_field_name("body") else {
            continue;
        };
        let mut cursor = body.walk();
        for stmt in body.named_children(&mut cursor) {
            if stmt.kind() != "expression_statement" {
                continue;
            }
            let Some(assign) = stmt.named_child(0) else {
                continue;
            };
            if assign.kind() != "assignment" {
                continue;
            }
            let (Some(left), Some(right)) = (
                assign.child_by_field_name("left"),
                assign.child_by_field_name("right"),
            ) else {
                continue;
            };
            let name = text(left, src);
            if left.kind() == "identifier"
                && !name.starts_with("__")
                && is_mutable_container_expr(right, src)
            {
                out.push((class_name.clone(), name.to_string()));
            }
        }
    }
    out
}

/// Guide: Sentinel Object — `None` is a sentinel and must be compared by
/// identity; `== None` invokes `__eq__` and can be overridden.
fn extract_equality_with_none(root: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "comparison_operator" {
            return;
        }
        let mut cursor = node.walk();
        let has_none = node.named_children(&mut cursor).any(|c| c.kind() == "none");
        if !has_none {
            return;
        }
        let mut cursor = node.walk();
        let has_eq = node
            .children(&mut cursor)
            .any(|c| matches!(c.kind(), "==" | "!="));
        if has_eq {
            out.push(enclosing_function_name(node, src));
        }
    });
    out
}

/// Name of the nearest enclosing `function_definition`, or `<module>`.
fn enclosing_function_name(node: Node, src: &[u8]) -> String {
    let mut cur = node;
    while let Some(parent) = cur.parent() {
        if parent.kind() == "function_definition"
            && let Some(name) = parent.child_by_field_name("name")
            && let Ok(text) = name.utf8_text(src)
        {
            return text.to_string();
        }
        cur = parent;
    }
    "<module>".to_string()
}

fn walk<'a>(root: Node<'a>, f: &mut dyn FnMut(Node<'a>)) {
    let mut walker = root.walk();
    let mut reached_root = false;
    while !reached_root {
        f(walker.node());
        if walker.goto_first_child() {
            continue;
        }
        loop {
            if walker.goto_next_sibling() {
                break;
            }
            if !walker.goto_parent() {
                reached_root = true;
                break;
            }
        }
    }
}

/// `except:` with no exception type — swallows everything including
/// KeyboardInterrupt/SystemExit. A bare `except_clause` has no child between
/// the `except` keyword and the `:`.
fn extract_bare_excepts(root: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "except_clause" {
            return;
        }
        // Children of a typed clause include an expression (identifier,
        // attribute, tuple, as_pattern, ...) before the colon; a bare
        // clause has only the keyword, the colon, and the block.
        let mut walker = node.walk();
        let has_filter = node
            .children(&mut walker)
            .any(|c| !matches!(c.kind(), "except" | ":" | "block" | "comment"));
        if !has_filter {
            out.push(enclosing_function_name(node, src));
        }
    });
    out
}

/// `def f(x=[])` / `def f(x={})` / `def f(x=set())` — the default is created
/// once at def time and shared across calls.
fn extract_mutable_defaults(root: Node, src: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if !matches!(node.kind(), "default_parameter" | "typed_default_parameter") {
            return;
        }
        let Some(value) = node.child_by_field_name("value") else {
            return;
        };
        let mutable = match value.kind() {
            "list" | "dictionary" | "set" => true,
            "call" => value
                .child_by_field_name("function")
                .and_then(|f| f.utf8_text(src).ok())
                .is_some_and(|name| matches!(name, "list" | "dict" | "set")),
            _ => false,
        };
        if mutable
            && let Some(name) = node.child_by_field_name("name")
            && let Ok(param) = name.utf8_text(src)
        {
            let fn_name = enclosing_function_name(node, src);
            out.push((fn_name, param.to_string()));
        }
    });
    out
}

fn extract_param_counts_high(root: Node, src: &[u8]) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "function_definition" {
            return;
        }
        let Some(params) = node.child_by_field_name("parameters") else {
            return;
        };
        let mut walker = params.walk();
        let mut count = 0usize;
        for (idx, child) in params
            .children(&mut walker)
            .filter(|c| {
                matches!(
                    c.kind(),
                    "identifier"
                        | "typed_parameter"
                        | "default_parameter"
                        | "typed_default_parameter"
                        | "list_splat_pattern"
                        | "dictionary_splat_pattern"
                )
            })
            .enumerate()
        {
            // Skip a leading self/cls receiver.
            if idx == 0
                && child.utf8_text(src).is_ok_and(|t| {
                    t == "self" || t == "cls" || t.starts_with("self:") || t.starts_with("cls:")
                })
            {
                continue;
            }
            count += 1;
        }
        if count >= PARAM_COUNT_THRESHOLD
            && let Some(name) = node.child_by_field_name("name")
            && let Ok(fn_name) = name.utf8_text(src)
        {
            out.push((fn_name.to_string(), count));
        }
    });
    out
}

/// Public `def`s (name not starting with `_`) whose body does not begin
/// with a docstring.
fn extract_missing_docstrings(root: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "function_definition" {
            return;
        }
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let Ok(fn_name) = name.utf8_text(src) else {
            return;
        };
        if fn_name.starts_with('_') {
            return;
        }
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        let mut walker = body.walk();
        let first_stmt = body.children(&mut walker).find(|c| c.kind() != "comment");
        let has_docstring = first_stmt.is_some_and(|stmt| {
            stmt.kind() == "expression_statement"
                && stmt.child(0).is_some_and(|c| c.kind() == "string")
        });
        if !has_docstring {
            out.push(fn_name.to_string());
        }
    });
    out
}

/// Recognize a call whose callee is the bare identifier `print`.
/// Excludes: `x.print()` (attribute access), `sprint()` (non-exact name),
/// comments, and string literals.
/// Rationale: Python logging HOWTO, <https://docs.python.org/3/howto/logging.html>.
fn extract_print_calls(root: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "call" {
            return;
        }
        let Some(func) = node.child_by_field_name("function") else {
            return;
        };
        // Must be an identifier named exactly "print" (not attribute, not prefix).
        if func.kind() == "identifier"
            && let Ok(name) = func.utf8_text(src)
            && name == "print"
        {
            out.push(enclosing_function_name(node, src));
        }
    });
    out
}

/// `def f(x=some())` — default argument whose value is a call expression.
/// Records (fn_name, param_name, callee_name). Immutable constructors
/// (list, dict, set) are also included so projects can selectively ignore
/// them; the distinction is visible in the callee field.
/// Upstream: Bugbear B008, <https://docs.astral.sh/ruff/rules/function-call-in-default-argument/>.
fn extract_call_in_default_args(root: Node, src: &[u8]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if !matches!(node.kind(), "default_parameter" | "typed_default_parameter") {
            return;
        }
        let Some(value) = node.child_by_field_name("value") else {
            return;
        };
        if value.kind() != "call" {
            return;
        }
        let Some(func) = value.child_by_field_name("function") else {
            return;
        };
        let Ok(callee) = func.utf8_text(src) else {
            return;
        };
        if let Some(name) = node.child_by_field_name("name")
            && let Ok(param) = name.utf8_text(src)
        {
            let fn_name = enclosing_function_name(node, src);
            out.push((fn_name, param.to_string(), callee.to_string()));
        }
    });
    out
}

/// Typed exception handlers whose body is only `pass`, comments, or
/// ellipsis (`...`). Excludes bare handlers (those are caught by the
/// bare-except rule). The effective body check: strip comments and
/// ellipsis expressions; if nothing remains, the handler swallows.
/// Upstream: Bugbear B110, <https://docs.astral.sh/ruff/rules/try-except-pass/>;
/// this predicate is narrower because it only reports typed handlers.
fn extract_handler_passes(root: Node, src: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "except_clause" {
            return;
        }
        // Skip bare except clauses — they have no exception type child.
        let has_filter = node
            .children(&mut node.walk())
            .any(|c| !matches!(c.kind(), "except" | ":" | "block" | "comment"));
        if !has_filter {
            return;
        }
        // Typed handler — check the body.
        let Some(block) = node
            .children(&mut node.walk())
            .find(|c| c.kind() == "block")
        else {
            return;
        };
        let is_empty_body = block.children(&mut block.walk()).all(|stmt| {
            matches!(stmt.kind(), "comment" | "pass_statement" | "ellipsis")
                || (stmt.kind() == "expression_statement"
                    && stmt.child(0).is_some_and(|c| c.kind() == "ellipsis"))
        });
        if is_empty_body {
            // Extract the exception type string.
            let exc_type = node
                .children(&mut node.walk())
                .find(|c| !matches!(c.kind(), "except" | ":" | "block" | "comment"))
                .and_then(|c| c.utf8_text(src).ok())
                .unwrap_or("?");
            out.push((enclosing_function_name(node, src), exc_type.to_string()));
        }
    });
    out
}

fn is_deferred_scope(node: Node) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(
            parent.kind(),
            "function_definition" | "class_definition" | "lambda"
        ) {
            return true;
        }
        current = parent.parent();
    }
    false
}

/// Obvious file, network, database, or process I/O executed while a module is
/// imported. This deliberately uses a narrow callee allowlist: the rule is a
/// review lead, not a claim that every arbitrary constructor performs I/O.
fn extract_import_time_io(root: Node, src: &[u8]) -> Vec<String> {
    const EXACT: &[&str] = &[
        "open",
        "socket.socket",
        "socket.create_connection",
        "sqlite3.connect",
        "requests.get",
        "requests.post",
        "requests.put",
        "requests.delete",
        "urllib.request.urlopen",
        "subprocess.run",
        "subprocess.call",
        "subprocess.check_call",
        "subprocess.check_output",
        "os.system",
    ];
    const SUFFIXES: &[&str] = &[
        ".read_text",
        ".read_bytes",
        ".write_text",
        ".write_bytes",
        ".open",
        ".connect",
    ];

    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "call" || is_deferred_scope(node) {
            return;
        }
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        let Ok(callee) = function.utf8_text(src) else {
            return;
        };
        if EXACT.contains(&callee) || SUFFIXES.iter().any(|suffix| callee.ends_with(suffix)) {
            out.push(callee.to_string());
        }
    });
    out
}

fn is_identity_literal(node: Node) -> bool {
    matches!(
        node.kind(),
        "string" | "concatenated_string" | "integer" | "float" | "list" | "dictionary" | "set"
    )
}

/// Identity comparisons against value literals. `None`, booleans, ellipsis,
/// and named sentinel objects are intentionally absent from the literal set.
fn extract_is_literal_comparisons(root: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "comparison_operator" {
            return;
        }
        let Ok(text) = node.utf8_text(src) else {
            return;
        };
        if !(text.contains(" is ") || text.contains(" is not ")) {
            return;
        }
        let mut cursor = node.walk();
        if node.named_children(&mut cursor).any(is_identity_literal) {
            out.push(enclosing_function_name(node, src));
        }
    });
    out
}

fn module_mutable_names(root: Node, src: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor = root.walk();
    for statement in root.named_children(&mut cursor) {
        if !matches!(statement.kind(), "expression_statement") {
            continue;
        }
        let Some(assignment) = statement.named_child(0) else {
            continue;
        };
        if assignment.kind() != "assignment" {
            continue;
        }
        let (Some(left), Some(right)) = (
            assignment.child_by_field_name("left"),
            assignment.child_by_field_name("right"),
        ) else {
            continue;
        };
        let mutable = matches!(right.kind(), "list" | "dictionary" | "set")
            || (right.kind() == "call"
                && right
                    .child_by_field_name("function")
                    .and_then(|function| function.utf8_text(src).ok())
                    .is_some_and(|callee| matches!(callee, "list" | "dict" | "set")));
        if mutable
            && left.kind() == "identifier"
            && let Ok(name) = left.utf8_text(src)
        {
            names.push(name.to_string());
        }
    }
    names
}

/// Module-level mutable containers that are later mutated from a function.
/// This is audit-only evidence: Python name shadowing can make a syntactic
/// match refer to a local with the same name.
fn extract_mutated_module_globals(root: Node, src: &[u8]) -> Vec<(String, String)> {
    const MUTATORS: &[&str] = &[
        "append",
        "extend",
        "insert",
        "remove",
        "pop",
        "clear",
        "sort",
        "reverse",
        "add",
        "discard",
        "update",
        "setdefault",
    ];
    let globals = module_mutable_names(root, src);
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "call" || enclosing_function_name(node, src) == "<module>" {
            return;
        }
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        if function.kind() != "attribute" {
            return;
        }
        let (Some(object), Some(attribute)) = (
            function.child_by_field_name("object"),
            function.child_by_field_name("attribute"),
        ) else {
            return;
        };
        let (Ok(name), Ok(method)) = (object.utf8_text(src), attribute.utf8_text(src)) else {
            return;
        };
        if globals.iter().any(|global| global == name) && MUTATORS.contains(&method) {
            out.push((enclosing_function_name(node, src), name.to_string()));
        }
    });
    out.sort();
    out.dedup();
    out
}

fn extract_star_imports(root: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "import_from_statement" {
            return;
        }
        let Ok(text) = node.utf8_text(src) else {
            return;
        };
        if !text.trim_end().ends_with("import *") {
            return;
        }
        let module = text
            .strip_prefix("from ")
            .and_then(|rest| rest.split_once(" import "))
            .map(|(module, _)| module)
            .unwrap_or("?");
        out.push(module.to_string());
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_except_is_flagged_with_enclosing_function() {
        let facts = extract(
            "def fetch(url):\n    \"\"\"Fetch.\"\"\"\n    try:\n        go(url)\n    except:\n        pass\n",
        );
        assert_eq!(facts.python_bare_excepts, vec!["fetch".to_string()]);
    }

    #[test]
    fn typed_except_is_not_flagged() {
        let facts = extract(
            "def fetch(url):\n    \"\"\"Fetch.\"\"\"\n    try:\n        go(url)\n    except ValueError as e:\n        raise e\n",
        );
        assert!(facts.python_bare_excepts.is_empty());
    }

    #[test]
    fn module_level_bare_except_reports_module() {
        let facts = extract("try:\n    setup()\nexcept:\n    pass\n");
        assert_eq!(facts.python_bare_excepts, vec!["<module>".to_string()]);
    }

    #[test]
    fn mutable_default_list_and_dict_call_are_flagged() {
        let facts = extract(
            "def add(item, bucket=[]):\n    \"\"\"Add.\"\"\"\n    bucket.append(item)\n\ndef make(opts=dict()):\n    \"\"\"Make.\"\"\"\n    return opts\n",
        );
        assert_eq!(
            facts.python_mutable_default_args,
            vec![
                ("add".to_string(), "bucket".to_string()),
                ("make".to_string(), "opts".to_string())
            ]
        );
    }

    #[test]
    fn immutable_defaults_are_fine() {
        let facts = extract(
            "def add(item, n=0, name=\"x\", flag=None):\n    \"\"\"Add.\"\"\"\n    return n\n",
        );
        assert!(facts.python_mutable_default_args.is_empty());
    }

    #[test]
    fn param_count_threshold_skips_self() {
        let facts = extract(
            "class C:\n    def m(self, a, b, c, d, e, f):\n        \"\"\"M.\"\"\"\n        return a\n",
        );
        assert_eq!(
            facts.python_function_param_counts_high,
            vec![("m".to_string(), 6)]
        );

        let under = extract(
            "class C:\n    def m(self, a, b, c, d, e):\n        \"\"\"M.\"\"\"\n        return a\n",
        );
        assert!(under.python_function_param_counts_high.is_empty());
    }

    #[test]
    fn missing_docstring_only_for_public_defs() {
        let facts = extract(
            "def documented():\n    \"\"\"Doc.\"\"\"\n    return 1\n\ndef naked():\n    return 2\n\ndef _private():\n    return 3\n",
        );
        assert_eq!(
            facts.python_functions_missing_docstring,
            vec!["naked".to_string()]
        );
    }

    // ─── python_print_call tests ───────────────────────────────────

    #[test]
    fn print_call_is_flagged() {
        let facts = extract("def process():\n    \"\"\"Process.\"\"\"\n    print('hello')\n");
        assert_eq!(facts.python_print_calls, vec!["process".to_string()]);
    }

    #[test]
    fn print_call_in_nested_function() {
        let facts = extract(
            "def outer():\n    \"\"\"Outer.\"\"\"\n    def inner():\n        print('hi')\n",
        );
        assert_eq!(facts.python_print_calls, vec!["inner".to_string()]);
    }

    #[test]
    fn print_call_excludes_attribute_access() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    logger.print('x')\n    printer.print('y')\n",
        );
        assert!(facts.python_print_calls.is_empty());
    }

    #[test]
    fn print_call_excludes_similar_names() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    sprint('x')\n    printx('y')\n    xprint('z')\n",
        );
        assert!(facts.python_print_calls.is_empty());
    }

    #[test]
    fn print_call_excludes_comments() {
        let facts = extract("def foo():\n    # print('in comment')\n    pass\n");
        assert!(facts.python_print_calls.is_empty());
    }

    #[test]
    fn print_call_excludes_strings() {
        let facts = extract("def foo():\n    x = \"print('in string')\"\n    pass\n");
        assert!(facts.python_print_calls.is_empty());
    }

    #[test]
    fn print_call_multiple_in_one_function() {
        let facts = extract("def foo():\n    \"\"\"Foo.\"\"\"\n    print('a')\n    print('b')\n");
        assert_eq!(facts.python_print_calls.len(), 2);
        assert!(facts.python_print_calls.iter().all(|n| *n == "foo"));
    }

    #[test]
    fn print_call_module_level() {
        let facts = extract("print('module level')\n");
        assert_eq!(facts.python_print_calls, vec!["<module>".to_string()]);
    }

    #[test]
    fn print_call_async_function() {
        let facts = extract("async def fetch():\n    \"\"\"Fetch.\"\"\"\n    print('async')\n");
        assert_eq!(facts.python_print_calls, vec!["fetch".to_string()]);
    }

    #[test]
    fn print_call_in_class() {
        let facts = extract(
            "class C:\n    def method(self):\n        \"\"\"Method.\"\"\"\n        print('x')\n",
        );
        assert_eq!(facts.python_print_calls, vec!["method".to_string()]);
    }

    // ─── python_call_in_default_args tests ─────────────────────────

    #[test]
    fn call_in_default_arg_is_flagged() {
        let facts = extract(
            "def make(default=[]):\n    \"\"\"Make.\"\"\"\n    return default\n\ndef make2(default=list()):\n    \"\"\"Make2.\"\"\"\n    return default\n\ndef make3(f=get_default()):\n    \"\"\"Make3.\"\"\"\n    return f\n",
        );
        // [] is a list literal (not a call), so only make2 and make3 match.
        assert_eq!(
            facts.python_call_in_default_args,
            vec![
                (
                    "make2".to_string(),
                    "default".to_string(),
                    "list".to_string()
                ),
                (
                    "make3".to_string(),
                    "f".to_string(),
                    "get_default".to_string()
                ),
            ]
        );
    }

    #[test]
    fn call_in_default_arg_nested_call() {
        let facts = extract("def make(f=config.read()):\n    \"\"\"Make.\"\"\"\n    return f\n");
        assert_eq!(
            facts.python_call_in_default_args,
            vec![(
                "make".to_string(),
                "f".to_string(),
                "config.read".to_string()
            )]
        );
    }

    #[test]
    fn no_call_in_default_arg() {
        let facts = extract("def make(f=1, g='x', h=None):\n    \"\"\"Make.\"\"\"\n    return f\n");
        assert!(facts.python_call_in_default_args.is_empty());
    }

    #[test]
    fn typed_default_parameter_with_call() {
        let facts = extract("def make(f: list = list()):\n    \"\"\"Make.\"\"\"\n    return f\n");
        assert_eq!(
            facts.python_call_in_default_args,
            vec![("make".to_string(), "f".to_string(), "list".to_string())]
        );
    }

    // ─── python_exception_handler_passes tests ─────────────────────

    #[test]
    fn typed_handler_with_only_pass_is_flagged() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    try:\n        go()\n    except ValueError:\n        pass\n",
        );
        assert_eq!(
            facts.python_exception_handler_passes,
            vec![("foo".to_string(), "ValueError".to_string())]
        );
    }

    #[test]
    fn typed_handler_with_ellipsis_is_flagged() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    try:\n        go()\n    except ValueError:\n        ...\n",
        );
        assert_eq!(
            facts.python_exception_handler_passes,
            vec![("foo".to_string(), "ValueError".to_string())]
        );
    }

    #[test]
    fn typed_handler_with_comment_only_is_flagged() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    try:\n        go()\n    except ValueError:\n        # intentional no-op\n        pass\n",
        );
        assert!(
            facts
                .python_exception_handler_passes
                .iter()
                .any(|(fn_, _)| fn_ == "foo"),
            "expected handler_passes for foo"
        );
    }

    #[test]
    fn typed_handler_with_real_body_is_not_flagged() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    try:\n        go()\n    except ValueError:\n        log_error(e)\n",
        );
        assert!(facts.python_exception_handler_passes.is_empty());
    }

    #[test]
    fn bare_handler_not_flagged_as_passes() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    try:\n        go()\n    except:\n        pass\n",
        );
        assert!(facts.python_exception_handler_passes.is_empty());
    }

    #[test]
    fn tuple_exception_type_flagged() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    try:\n        go()\n    except (ValueError, TypeError):\n        pass\n",
        );
        // The exception type string will be the text of the tuple node.
        assert!(
            facts
                .python_exception_handler_passes
                .iter()
                .any(|(fn_, _)| fn_ == "foo"),
            "expected handler_passes for foo"
        );
    }

    #[test]
    fn print_call_in_string_literal_not_flagged() {
        let facts = extract("def foo():\n    \"\"\"Foo.\"\"\"\n    x = \"print('not a call')\"\n");
        assert!(facts.python_print_calls.is_empty());
    }

    #[test]
    fn malformed_source_partial_except_not_crashed() {
        // tree-sitter is resilient; this shouldn't panic
        let facts = extract("def foo():\n    try:\n        pass\n    except\n");
        // May extract partial facts or empty; must not panic and must not
        // invent a print call that is not there.
        assert!(facts.python_print_calls.is_empty());
        assert!(facts.python_bare_excepts.len() <= 1);
    }

    #[test]
    fn positional_only_and_keyword_only_params_counted() {
        let facts = extract("def f(a, /, b, *, c):\n    \"\"\"F.\"\"\"\n    return a\n");
        // a, b, c = 3 params, should not trigger threshold
        assert!(facts.python_function_param_counts_high.is_empty());
    }

    #[test]
    fn varargs_and_kwargs_counted_in_param_threshold() {
        let facts = extract("def f(a, b, c, d, *args, **kwargs):\n    \"\"\"F.\"\"\"\n    pass\n");
        assert_eq!(
            facts.python_function_param_counts_high,
            vec![("f".to_string(), 6)]
        );
    }

    #[test]
    fn cls_not_counted_in_param_threshold() {
        let facts = extract(
            "class C:\n    def m(cls, a, b, c, d, e, f):\n        \"\"\"M.\"\"\"\n        pass\n",
        );
        assert_eq!(
            facts.python_function_param_counts_high,
            vec![("m".to_string(), 6)]
        );
    }

    #[test]
    fn import_time_io_reports_module_calls_but_not_function_bodies() {
        let facts =
            extract("CONFIG = open('config.json')\n\ndef later():\n    return open('later.txt')\n");
        assert_eq!(facts.python_import_time_io, vec!["open"]);
    }

    #[test]
    fn import_time_io_recognizes_path_and_network_calls() {
        let facts = extract(
            "TEXT = Path('x').read_text()\nRESPONSE = requests.get('https://example.test')\n",
        );
        assert_eq!(
            facts.python_import_time_io,
            vec!["Path('x').read_text", "requests.get"]
        );
    }

    #[test]
    fn identity_literal_flags_values_but_allows_none_and_sentinels() {
        let facts = extract(
            "def f(x):\n    return x is 'ready'\n\ndef g(x):\n    return x is None or x is MISSING\n",
        );
        assert_eq!(facts.python_is_literal_comparisons, vec!["f"]);
    }

    #[test]
    fn mutated_module_global_reports_mutator_calls() {
        let facts = extract(
            "CACHE = {}\nCONSTANT = ()\n\ndef remember(key, value):\n    CACHE.update({key: value})\n",
        );
        assert_eq!(
            facts.python_mutated_module_globals,
            vec![("remember".to_string(), "CACHE".to_string())]
        );
    }

    #[test]
    fn immutable_or_unmodified_module_values_are_accepted() {
        let facts = extract("CACHE = {}\nNAMES = ()\n\ndef read():\n    return CACHE.get('x')\n");
        assert!(facts.python_mutated_module_globals.is_empty());
    }

    #[test]
    fn star_import_reports_source_module() {
        let facts = extract("from package.tools import *\nfrom package.safe import thing\n");
        assert_eq!(facts.python_star_imports, vec!["package.tools"]);
    }

    // ─── python-patterns.guide predicates ──────────────────────────

    #[test]
    fn global_statement_reports_each_name_per_function() {
        let facts = extract(
            "_seed = 42\n\ndef set_seed(v):\n    global _seed, other\n    _seed = v\n\ndef read():\n    return _seed\n",
        );
        assert_eq!(
            facts.python_global_statements,
            vec![
                ("set_seed".to_string(), "_seed".to_string()),
                ("set_seed".to_string(), "other".to_string())
            ]
        );
    }

    #[test]
    fn global_statement_ignores_module_level_and_nonlocal() {
        let facts = extract(
            "global X\n\ndef outer():\n    x = 1\n    def inner():\n        nonlocal x\n        x = 2\n",
        );
        assert!(facts.python_global_statements.is_empty());
    }

    #[test]
    fn globals_subscript_assignment_is_flagged() {
        let facts = extract(
            "for name in dir(_instance):\n    globals()[name] = getattr(_instance, name)\n\ndef fine():\n    table = globals()\n    return table['x']\n",
        );
        assert_eq!(
            facts.python_globals_subscript_assignments,
            vec!["<module>".to_string()]
        );
    }

    #[test]
    fn dynamic_class_creation_requires_three_arg_type_call() {
        let facts = extract(
            "def build(name, a, b):\n    return type(name, (a, b), {})\n\ndef fine(x):\n    return type(x) is int\n",
        );
        assert_eq!(
            facts.python_dynamic_class_creations,
            vec!["build".to_string()]
        );
    }

    #[test]
    fn new_override_classifies_singleton_and_custom() {
        let facts = extract(
            "class Logger:\n    _instance = None\n    def __new__(cls):\n        if cls._instance is None:\n            cls._instance = super().__new__(cls)\n        return cls._instance\n\nclass Grade:\n    def __new__(cls, percent):\n        return super().__new__(cls)\n\nclass Plain:\n    def __init__(self):\n        pass\n",
        );
        assert_eq!(
            facts.python_new_overrides,
            vec![
                ("Logger".to_string(), "singleton".to_string()),
                ("Grade".to_string(), "custom".to_string())
            ]
        );
    }

    #[test]
    fn isinstance_chain_counts_if_and_elif_conditions() {
        let facts = extract(
            "def render(w):\n    if isinstance(w, Frame):\n        return 1\n    elif isinstance(w, Label):\n        return 2\n    return 0\n\ndef single(w):\n    if isinstance(w, Frame):\n        return 1\n    x = isinstance(w, Label)\n    return x\n",
        );
        assert_eq!(
            facts.python_isinstance_chains,
            vec![("render".to_string(), 2)]
        );
    }

    #[test]
    fn isinstance_chain_requires_same_subject_domain_dispatch() {
        let facts = extract(
            "def render(w, other):\n    if isinstance(w, Frame):\n        return 1\n    elif isinstance(other, Label):\n        return 2\n\ndef independent(w):\n    if isinstance(w, Frame):\n        return 1\n    if isinstance(w, Label):\n        return 2\n",
        );
        assert!(facts.python_isinstance_chains.is_empty());
    }

    #[test]
    fn isinstance_chain_excludes_validation_and_negative_guards() {
        let facts = extract(
            "def validate(value):\n    if isinstance(value, str):\n        return 1\n    elif isinstance(value, bytes):\n        return 2\n\ndef guarded(value):\n    if not isinstance(value, Frame):\n        return 1\n    elif isinstance(value, Label):\n        return 2\n",
        );
        assert!(facts.python_isinstance_chains.is_empty());
    }

    #[test]
    fn isinstance_chain_accepts_qualified_domain_types() {
        let facts = extract(
            "def render(node):\n    if isinstance(node, model.Frame):\n        return 1\n    elif isinstance(node, (model.Label, model.Button)):\n        return 2\n",
        );
        assert_eq!(
            facts.python_isinstance_chains,
            vec![("render".to_string(), 2)]
        );
    }

    #[test]
    fn container_is_own_iterator_needs_container_protocol() {
        let facts = extract(
            "class Bad:\n    def __len__(self):\n        return 3\n    def __iter__(self):\n        return self\n    def __next__(self):\n        raise StopIteration\n\nclass PureIterator:\n    def __iter__(self):\n        return self\n    def __next__(self):\n        raise StopIteration\n\nclass Good:\n    def __len__(self):\n        return 3\n    def __iter__(self):\n        return PureIterator()\n",
        );
        assert_eq!(
            facts.python_containers_own_iterator,
            vec!["Bad".to_string()]
        );
    }

    #[test]
    fn multiple_inheritance_excludes_mixins_abcs_and_keywords() {
        let facts = extract(
            "class A(FilteredLogger, SocketLogger):\n    pass\n\nclass B(FilterMixin, FileLogger):\n    pass\n\nclass C(ABC, Generic[T], Protocol, Base, metaclass=Meta):\n    pass\n\nclass D(X, Y, Z):\n    pass\n",
        );
        assert_eq!(
            facts.python_multiple_inheritance,
            vec![("A".to_string(), 2), ("D".to_string(), 3)]
        );
    }

    #[test]
    fn inheritance_depth_is_file_local() {
        let facts = extract(
            "class A:\n    pass\n\nclass B(A):\n    pass\n\nclass C(B):\n    pass\n\nclass D(C):\n    pass\n\nclass E(Unknown):\n    pass\n",
        );
        assert_eq!(
            facts.python_inheritance_depths,
            vec![("C".to_string(), 3), ("D".to_string(), 4)]
        );
    }

    #[test]
    fn mixin_with_init_is_flagged_by_name_suffix() {
        let facts = extract(
            "class FilterMixin:\n    def __init__(self):\n        self.pattern = ''\n\nclass PlainMixin:\n    def log(self):\n        pass\n\nclass Base:\n    def __init__(self):\n        pass\n",
        );
        assert_eq!(
            facts.python_mixins_with_init,
            vec!["FilterMixin".to_string()]
        );
    }

    #[test]
    fn static_delegation_wrapper_counts_pure_forwarding_methods() {
        let src = "class W:\n    def __init__(self, f):\n        self._file = f\n    def read(self, n):\n        return self._file.read(n)\n    def close(self):\n        return self._file.close()\n    def flush(self):\n        return self._file.flush()\n    def seek(self, p):\n        return self._file.seek(p)\n    def write(self, s):\n        log(s)\n        return self._file.write(s)\n";
        let facts = extract(src);
        assert_eq!(
            facts.python_static_delegation_wrappers,
            vec![("W".to_string(), "_file".to_string(), 4)]
        );
        let dynamic = extract(&format!(
            "{src}    def __getattr__(self, name):\n        return getattr(self._file, name)\n"
        ));
        assert!(dynamic.python_static_delegation_wrappers.is_empty());
        let few = extract(
            "class W:\n    def read(self, n):\n        return self._file.read(n)\n    def close(self):\n        return self._file.close()\n",
        );
        assert!(few.python_static_delegation_wrappers.is_empty());
    }

    #[test]
    fn mutable_class_attribute_skips_dunders_and_immutables() {
        let facts = extract(
            "class Registry:\n    items = []\n    index = dict()\n    __slots__ = []\n    NAME = 'x'\n    pairs = ()\n    def method(self):\n        local = []\n        return local\n",
        );
        assert_eq!(
            facts.python_mutable_class_attributes,
            vec![
                ("Registry".to_string(), "items".to_string()),
                ("Registry".to_string(), "index".to_string())
            ]
        );
    }

    #[test]
    fn equality_with_none_flags_eq_and_ne_only() {
        let facts = extract(
            "def f(x):\n    return x == None\n\ndef g(x):\n    return None != x\n\ndef h(x):\n    return x is None or x is not None\n",
        );
        assert_eq!(
            facts.python_equality_with_none,
            vec!["f".to_string(), "g".to_string()]
        );
    }
}
