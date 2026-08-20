use super::super::parsed::ParsedFile;
use super::walk::{function_name, is_test_fn, walk_function_items};
use tree_sitter::Node;

pub fn extract_unsafe_without_safety(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let mut out = Vec::new();
    walk_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "unsafe_block" || has_safety_comment(node, src) || in_test_code(node, src)
        {
            return;
        }
        out.push(enclosing_function(node, src));
    });
    out
}

fn has_safety_comment(node: Node, source: &[u8]) -> bool {
    let start = node.start_byte();
    let prefix_start = source[..start]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .and_then(|last| {
            source[..last]
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map(|previous| previous + 1)
        })
        .unwrap_or(0);
    let prefix = std::str::from_utf8(&source[prefix_start..start]).unwrap_or("");
    if explains_safety(prefix) {
        return true;
    }

    let body_end = node.end_byte().min(source.len());
    let body_prefix_end = (start + 200).min(body_end);
    explains_safety(std::str::from_utf8(&source[start..body_prefix_end]).unwrap_or(""))
}

/// Whether nearby prose reads as a safety justification.
///
/// Requiring the literal token `SAFETY:` flags well-documented `unsafe` that
/// simply words it differently — all six hits on Phronesis were one `#[test]`
/// whose comment explained the reasoning without that exact token. The rule is
/// advisory, so it should ask "did the author explain themselves", not "did
/// the author use the blessed keyword".
///
/// Deliberately generous: a missed explanation is silence, while a false
/// "undocumented unsafe" trains people to ignore the rule.
fn explains_safety(text: &str) -> bool {
    let upper = text.to_ascii_uppercase();
    if upper.contains("SAFETY:") || upper.contains("# SAFETY") {
        return true;
    }
    // A comment that argues about soundness counts, however it is phrased.
    let has_comment = text.contains("//") || text.contains("/*");
    has_comment
        && ["SAFE", "SOUND", "INVARIANT", "UPHELD", "GUARANTEE", "VALID"]
            .iter()
            .any(|marker| upper.contains(marker))
}

pub fn extract_async_blocking_calls(parsed: &ParsedFile) -> Vec<(String, String)> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    walk_function_items(&mut cursor, src, &mut |function, name| {
        if !function_is_async(function, src) {
            return;
        }
        walk_nodes(function, &mut |node| {
            if node.kind() != "call_expression"
                || inside_spawn_blocking(node, function, src)
                || in_test_code(node, src)
            {
                return;
            }
            let Some(callee) = node.child_by_field_name("function") else {
                return;
            };
            let Ok(text) = callee.utf8_text(src) else {
                return;
            };
            if is_known_blocking_call(text) {
                out.push((name.to_string(), text.to_string()));
            }
        });
    });
    out
}

pub fn extract_sync_lock_guards_across_await(parsed: &ParsedFile) -> Vec<(String, String)> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    walk_function_items(&mut cursor, src, &mut |function, name| {
        if !function_is_async(function, src) {
            return;
        }
        walk_nodes(function, &mut |node| {
            if node.kind() != "let_declaration" {
                return;
            }
            let Some(value) = node.child_by_field_name("value") else {
                return;
            };
            if !binds_sync_lock_guard(value, src) {
                return;
            }
            let Some(pattern) = node.child_by_field_name("pattern") else {
                return;
            };
            let guard = pattern.utf8_text(src).unwrap_or("<guard>");
            let scope_end = nearest_block_end(node).unwrap_or(function.end_byte());
            // An explicit `drop(guard)` ends the scope early, and a temporary
            // bound to a value (not a guard name) is released at its statement.
            // Without these the rule flags code that demonstrably releases the
            // lock first — both Phronesis ownership fixtures for those shapes
            // were reported as hazards.
            let released_at = explicit_drop_byte(function, guard, src, node.end_byte(), scope_end)
                .unwrap_or(scope_end);
            if awaits_between(function, node.end_byte(), released_at) {
                out.push((name.to_string(), guard.to_string()));
            }
        });
    });
    out
}

/// True when `node` sits in test code: inside a `#[test]`/`#[tokio::test]`
/// function, or anywhere under a `#[cfg(test)]` module.
///
/// Dogfooding these rules on Phronesis produced 34 of 39 blocking-call hits in
/// test bodies. Blocking I/O in a test is not a latency defect, and the noise
/// buries the real findings — four production `async fn`s on the hook path.
/// The ownership extractor excludes test bodies for the same reason (D14).
fn in_test_code(node: Node, source: &[u8]) -> bool {
    let mut current = Some(node);
    while let Some(candidate) = current {
        match candidate.kind() {
            "function_item" if is_test_fn(candidate, source) => return true,
            "mod_item" if has_cfg_test_attribute(candidate, source) => return true,
            _ => {}
        }
        current = candidate.parent();
    }
    false
}

/// True if a preceding-sibling attribute on `node` is `#[cfg(test)]`.
fn has_cfg_test_attribute(node: Node, source: &[u8]) -> bool {
    let mut prev = node.prev_sibling();
    while let Some(sibling) = prev {
        match sibling.kind() {
            "attribute_item" => {
                let text = sibling.utf8_text(source).unwrap_or("");
                if text.replace(char::is_whitespace, "").contains("cfg(test)") {
                    return true;
                }
                prev = sibling.prev_sibling();
            }
            "line_comment" | "block_comment" => prev = sibling.prev_sibling(),
            _ => return false,
        }
    }
    false
}

/// True when this `let` initializer is *itself* a synchronous lock
/// acquisition, rather than merely containing the text `.lock()` somewhere.
///
/// The substring form misattributes the guard. In `phronesis::network`:
///
/// ```ignore
/// let p_state_activations = {
///     let mut beta_network = self.beta_network.lock()?;   // the real guard
///     ...
/// };                                                       // drops here
/// step().await;                                            // after the drop
/// ```
///
/// Text matching binds the guard to `p_state_activations` and then finds the
/// later `.await`, reporting a hazard that the inner block already prevented.
/// So the walk refuses to descend into a `block` or closure: a lock acquired
/// in a nested scope belongs to that scope, not to this binding.
///
/// An awaited acquisition (`m.lock().await`) is an async lock and is not a
/// synchronous guard at all.
fn binds_sync_lock_guard(value: Node, source: &[u8]) -> bool {
    if value.kind() == "await_expression" {
        return false;
    }
    let mut pending = vec![value];
    while let Some(node) = pending.pop() {
        if matches!(
            node.kind(),
            "block" | "closure_expression" | "async_block" | "await_expression"
        ) {
            continue;
        }
        if node.kind() == "call_expression"
            && node
                .child_by_field_name("function")
                .filter(|callee| callee.kind() == "field_expression")
                .and_then(|callee| callee.child_by_field_name("field"))
                .and_then(|field| field.utf8_text(source).ok())
                .is_some_and(|method| matches!(method, "lock" | "read" | "write"))
        {
            return true;
        }
        let mut cursor = node.walk();
        pending.extend(node.children(&mut cursor));
    }
    false
}

/// True if a real `await_expression` node starts within `[after, before)`.
///
/// Structural, so `.await` inside a comment or string literal cannot trigger
/// it — the text form counted those.
fn awaits_between(function: Node, after: usize, before: usize) -> bool {
    let mut pending = vec![function];
    while let Some(node) = pending.pop() {
        if node.kind() == "await_expression"
            && node.start_byte() >= after
            && node.start_byte() < before
        {
            return true;
        }
        let mut cursor = node.walk();
        pending.extend(node.children(&mut cursor));
    }
    false
}

/// Byte offset of an explicit `drop(<guard>)` between `after` and `before`.
///
/// Mirrors the ownership extractor's D6 case 2 without sharing its code: the
/// two subsystems make different claims and must stay independent (D11).
/// Assumes `drop` resolves to the prelude; a locally shadowed `drop` makes
/// this wrong, which is acceptable for an advisory rule.
fn explicit_drop_byte(
    function: Node,
    guard: &str,
    source: &[u8],
    after: usize,
    before: usize,
) -> Option<usize> {
    let mut pending = vec![function];
    while let Some(node) = pending.pop() {
        if node.kind() == "call_expression"
            && node.start_byte() > after
            && node.start_byte() < before
            && node
                .child_by_field_name("function")
                .and_then(|callee| callee.utf8_text(source).ok())
                .is_some_and(|callee| callee == "drop")
            && node
                .child_by_field_name("arguments")
                .filter(|args| args.named_child_count() == 1)
                .and_then(|args| args.named_child(0))
                .and_then(|arg| arg.utf8_text(source).ok())
                .is_some_and(|arg| arg == guard)
        {
            return Some(node.start_byte());
        }
        let mut cursor = node.walk();
        pending.extend(node.children(&mut cursor));
    }
    None
}

fn nearest_block_end(node: Node) -> Option<usize> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind() == "block" {
            return Some(parent.end_byte());
        }
        current = parent.parent();
    }
    None
}

fn function_is_async(function: Node, source: &[u8]) -> bool {
    function
        .children(&mut function.walk())
        .any(|child| child.kind() == "async" || child.utf8_text(source).is_ok_and(|t| t == "async"))
}

fn is_known_blocking_call(callee: &str) -> bool {
    const EXACT: &[&str] = &[
        "std::thread::sleep",
        "thread::sleep",
        "std::fs::read",
        "std::fs::read_to_string",
        "std::fs::write",
        "std::fs::copy",
        "std::fs::rename",
        "std::fs::remove_file",
        "std::fs::remove_dir",
        "std::fs::remove_dir_all",
        "std::fs::create_dir",
        "std::fs::create_dir_all",
        "std::fs::canonicalize",
        "std::fs::metadata",
        "std::fs::read_dir",
        "std::fs::File::open",
        "std::fs::File::create",
        "fs::read",
        "fs::read_to_string",
        "fs::write",
        "fs::copy",
        "fs::rename",
        "fs::remove_file",
        "fs::remove_dir_all",
        "fs::create_dir_all",
        "fs::canonicalize",
        "fs::metadata",
        "fs::read_dir",
        "File::open",
        "File::create",
    ];
    EXACT.contains(&callee)
}

fn inside_spawn_blocking(node: Node, function: Node, source: &[u8]) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.id() == function.id() {
            return false;
        }
        if parent.kind() == "call_expression"
            && parent
                .child_by_field_name("function")
                .and_then(|callee| callee.utf8_text(source).ok())
                .is_some_and(|callee| callee.ends_with("spawn_blocking"))
        {
            return true;
        }
        current = parent.parent();
    }
    false
}

fn enclosing_function(node: Node, source: &[u8]) -> String {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind() == "function_item" {
            return function_name(parent, source).unwrap_or_else(|| "<unknown>".to_string());
        }
        current = parent.parent();
    }
    "<module>".to_string()
}

fn walk_nodes<'a>(node: Node<'a>, visit: &mut impl FnMut(Node<'a>)) {
    visit(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_nodes(child, visit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> ParsedFile {
        ParsedFile::parse_rust(source).expect("valid Rust fixture")
    }

    #[test]
    fn undocumented_unsafe_block_is_reported() {
        let facts = extract_unsafe_without_safety(&parse("fn f(p: *const u8) { unsafe { *p; } }"));
        assert_eq!(facts, vec!["f"]);
    }

    #[test]
    fn safety_comment_before_or_inside_block_is_accepted() {
        let before = parse(
            "fn f(p: *const u8) {\n// SAFETY: caller guarantees validity.\nunsafe { *p; }\n}",
        );
        let inside = parse("fn g(p: *const u8) { unsafe { // SAFETY: checked above.\n*p; } }");
        assert!(extract_unsafe_without_safety(&before).is_empty());
        assert!(extract_unsafe_without_safety(&inside).is_empty());
    }

    #[test]
    fn blocking_call_in_async_function_is_reported() {
        let facts = extract_async_blocking_calls(&parse(
            "async fn load() { let _ = std::fs::read_to_string(\"x\"); }",
        ));
        assert_eq!(
            facts,
            vec![("load".to_string(), "std::fs::read_to_string".to_string())]
        );
    }

    #[test]
    fn sync_function_and_spawn_blocking_are_accepted() {
        let sync = parse("fn load() { let _ = std::fs::read(\"x\"); }");
        let spawned =
            parse("async fn load() { tokio::task::spawn_blocking(|| std::fs::read(\"x\")); }");
        assert!(extract_async_blocking_calls(&sync).is_empty());
        assert!(extract_async_blocking_calls(&spawned).is_empty());
    }

    #[test]
    fn std_lock_guard_crossing_await_is_reported() {
        let parsed = parse(
            "use std::sync::Mutex;\nasync fn update(m: &Mutex<u8>) { let guard = m.lock().expect(\"lock\"); work().await; drop(guard); }",
        );
        assert_eq!(
            extract_sync_lock_guards_across_await(&parsed),
            vec![("update".to_string(), "guard".to_string())]
        );
    }

    // Pins the false positive that dogfooding found in `phronesis::network`:
    // a lock acquired inside a nested block belongs to that block, and the
    // outer binding is not a guard. Text matching reported this as a hazard.
    #[test]
    fn a_lock_inside_a_nested_block_is_not_the_outer_bindings_guard() {
        let parsed = parse(
            "async fn f(m: &M) { let acts = { let g = m.lock().unwrap(); g.len() }; step().await; }",
        );
        assert!(
            extract_sync_lock_guards_across_await(&parsed).is_empty(),
            "the guard drops at the inner block, before the await"
        );
    }

    // Pins D6-case-2 parity: an explicit drop of *this* guard ends its scope.
    #[test]
    fn an_explicit_drop_of_the_guard_before_the_await_is_accepted() {
        let parsed =
            parse("async fn f(m: &M) { let g = m.lock().unwrap(); drop(g); step().await; }");
        assert!(
            extract_sync_lock_guards_across_await(&parsed).is_empty(),
            "drop(g) releases before the await"
        );
    }

    // The companion: dropping a *different* binding must not excuse this one.
    #[test]
    fn dropping_some_other_binding_does_not_excuse_the_guard() {
        let parsed =
            parse("async fn f(m: &M) { let g = m.lock().unwrap(); drop(other); step().await; }");
        assert_eq!(
            extract_sync_lock_guards_across_await(&parsed).len(),
            1,
            "only a drop of the guard itself ends its scope"
        );
    }

    // The true positive must survive every exclusion above.
    #[test]
    fn a_guard_genuinely_live_across_the_await_is_still_reported() {
        let parsed =
            parse("async fn f(m: &M) { let g = m.lock().unwrap(); step().await; g.len(); }");
        assert_eq!(
            extract_sync_lock_guards_across_await(&parsed).len(),
            1,
            "this one really does hold the lock across suspension"
        );
    }

    // `.await` inside a comment or string is not a suspension point.
    #[test]
    fn an_await_in_a_comment_or_string_is_not_a_suspension_point() {
        let parsed = parse(
            "async fn f(m: &M) { let g = m.lock().unwrap(); let s = \"x .await y\"; // .await\n g.len(); }",
        );
        assert!(
            extract_sync_lock_guards_across_await(&parsed).is_empty(),
            "structural detection ignores comments and strings"
        );
    }

    // Test bodies are fixture text, not production latency defects (D14 parity).
    #[test]
    fn blocking_calls_inside_test_code_are_not_reported() {
        let in_test_fn = parse("#[test]\nfn t() { let _ = std::fs::read(\"x\"); }");
        let in_test_mod = parse(
            "#[cfg(test)]\nmod tests { async fn helper() { let _ = std::fs::read(\"x\"); } }",
        );
        assert!(extract_async_blocking_calls(&in_test_fn).is_empty());
        assert!(
            extract_async_blocking_calls(&in_test_mod).is_empty(),
            "a helper under #[cfg(test)] is test code even without #[test]"
        );
    }

    // A justification that does not use the blessed token still counts.
    #[test]
    fn a_safety_justification_worded_differently_is_accepted() {
        let parsed = parse(
            "fn f(p: *const u8) {\n// The caller upholds the invariant that p is valid.\nunsafe { *p; }\n}",
        );
        assert!(
            extract_unsafe_without_safety(&parsed).is_empty(),
            "the rule asks whether the author explained themselves"
        );
    }

    #[test]
    fn scoped_std_lock_and_tokio_lock_are_accepted() {
        let scoped = parse(
            "use std::sync::Mutex;\nasync fn update(m: &Mutex<u8>) { { let guard = m.lock().expect(\"lock\"); drop(guard); } work().await; }",
        );
        let tokio = parse(
            "use tokio::sync::Mutex;\nasync fn update(m: &Mutex<u8>) { let guard = m.lock().await; work().await; drop(guard); }",
        );
        assert!(extract_sync_lock_guards_across_await(&scoped).is_empty());
        assert!(extract_sync_lock_guards_across_await(&tokio).is_empty());
    }
}
