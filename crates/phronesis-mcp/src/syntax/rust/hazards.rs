use super::super::parsed::ParsedFile;
use super::walk::{function_name, in_test_code, walk_function_items};
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

/// Structural association for safety justifications.
///
/// A comment explains an `unsafe` block when it is part of the contiguous run
/// of comment nodes that ends at the statement holding the block — checked at
/// every ancestor level from the block out to its enclosing statement head,
/// because `let fd = unsafe { .. };` and `check(unsafe { .. })?` wrap the
/// block in nodes whose head the comment sits above — or when it is a direct
/// child of the block's own body. Only comment nodes are read, so `SAFETY:`
/// text inside string or raw-string literals never counts (the old
/// previous-line-plus-200-byte-body scan accepted both, and missed a `SAFETY:`
/// comment that wrapped onto a further `//` line — all eight audit hits on
/// Phronesis v0.41.0 were such documented blocks). The ancestor search stops
/// at statement-body scope: a `block` ends the climb, so a comment above a
/// construct that owns a body — a `fn`, a closure body, an `if`, or an outer
/// `unsafe` — never justifies an `unsafe` block inside that body, while
/// wrapped values and call arguments keep associating because no block lies
/// between them and their statement head. Within a level, any intervening
/// sibling — another statement, an attribute, or a neighboring `unsafe`
/// block — ends the search and a justification for one block can never
/// excuse the next. Adjacent means adjacent lines: a blank line between run
/// members or between the run and the anchor breaks association, matching
/// the old single-previous-line scan.
fn has_safety_comment(node: Node, source: &[u8]) -> bool {
    run_above_any_level(node, source) || block_body_comments_explain(node, source)
}

fn is_comment(kind: &str) -> bool {
    kind == "line_comment" || kind == "block_comment"
}

/// Checks the comment run above the block, then above each enclosing wrapper
/// (statement, call arguments, `let` value) — until a statement body `block`
/// is reached, which ends the search so enclosing items, closures, outer
/// blocks, and modules cannot donate their comments to an inner `unsafe`.
fn run_above_any_level(node: Node, source: &[u8]) -> bool {
    let mut level = Some(node);
    while let Some(current) = level {
        if run_above_explains(current, source) {
            return true;
        }
        let Some(parent) = current.parent() else {
            return false;
        };
        if parent.kind() == "block" {
            return false;
        }
        level = Some(parent);
    }
    false
}

/// The contiguous run of comments ending directly above `anchor`: walk
/// previous siblings while they are comments; any other sibling ends the run.
/// Any member that explains safety accepts the block.
fn run_above_explains(anchor: Node, source: &[u8]) -> bool {
    let mut run_end = anchor.start_byte();
    let mut current = anchor.prev_sibling();
    while let Some(sibling) = current {
        if !is_comment(sibling.kind()) {
            return false;
        }
        if newlines_between(sibling.end_byte(), run_end, source) > 1 {
            return false;
        }
        if explains_safety(sibling.utf8_text(source).unwrap_or("")) {
            return true;
        }
        run_end = sibling.start_byte();
        current = sibling.prev_sibling();
    }
    false
}

fn newlines_between(from: usize, to: usize, source: &[u8]) -> usize {
    let to = to.min(source.len());
    let from = from.min(to);
    source[from..to]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
}

/// Comments that are direct children of the block's own body — leading,
/// interleaved, or trailing at that level — explain this block. Comments
/// nested inside inner statements belong to that inner code and do not count.
fn block_body_comments_explain(node: Node, source: &[u8]) -> bool {
    let mut cursor = node.walk();
    let Some(body) = node
        .children(&mut cursor)
        .find(|child| child.kind() == "block")
    else {
        return false;
    };
    let mut body_cursor = body.walk();
    body.children(&mut body_cursor).any(|child| {
        is_comment(child.kind()) && explains_safety(child.utf8_text(source).unwrap_or(""))
    })
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
            let released_at = explicit_drop_byte(function, guard, src, node.end_byte()..scope_end)
                .unwrap_or(scope_end);
            if awaits_between(function, node.end_byte(), released_at) {
                out.push((name.to_string(), guard.to_string()));
            }
        });
    });
    out
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

/// Byte offset of an explicit `drop(<guard>)` strictly inside `window`.
///
/// Mirrors the ownership extractor's D6 case 2 without sharing its code: the
/// two subsystems make different claims and must stay independent (D11).
/// Assumes `drop` resolves to the prelude; a locally shadowed `drop` makes
/// this wrong, which is acceptable for an advisory rule.
fn explicit_drop_byte(
    function: Node,
    guard: &str,
    source: &[u8],
    window: std::ops::Range<usize>,
) -> Option<usize> {
    let mut pending = vec![function];
    while let Some(node) = pending.pop() {
        if node.kind() == "call_expression"
            && node.start_byte() > window.start
            && node.start_byte() < window.end
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

    // Pins the eight audit hits on Phronesis v0.41.0 (language_packs/assets.rs,
    // language_packs/install.rs, properties/process.rs): a `SAFETY:` comment
    // that wraps onto further `//` lines directly above the block was missed
    // because the old detector read only the single previous line.
    #[test]
    fn multiline_safety_comment_above_the_block_is_accepted() {
        let parsed = parse(
            "fn f(dir: i32, name: &CStr, st: &mut Stat) {\n// SAFETY: `dir` is an open descriptor, `name` is NUL-terminated, and\n// `st` is a writable `stat` that outlives the call.\nunsafe { libc::fstatat(dir, name.as_ptr(), st, 0); }\n}",
        );
        assert!(
            extract_unsafe_without_safety(&parsed).is_empty(),
            "a SAFETY comment wrapping to a second line still documents this block"
        );
    }

    // The install.rs `lock` shape: the comment sits above a multi-line
    // statement whose `unsafe` block starts on a later line.
    #[test]
    fn safety_comment_above_a_multiline_let_wrapped_block_is_accepted() {
        let parsed = parse(
            "fn lock(store: i32, name: &CStr) {\n// SAFETY: `name` is a valid NUL-terminated string and `store` is an\n// open directory descriptor owned by the caller.\nlet fd = unsafe {\nlibc::openat(store, name.as_ptr(), flags, 0o644)\n};\n}",
        );
        assert!(extract_unsafe_without_safety(&parsed).is_empty());
    }

    // The process.rs `kill_group` shape: a block statement preceded by a
    // multi-line comment run inside an `if let`.
    #[test]
    fn safety_comment_run_inside_an_if_let_is_accepted() {
        let parsed = parse(
            "fn kill_group(child: &mut Child) {\nif let Ok(pgid) = libc::pid_t::try_from(child.id()) {\n// SAFETY: killpg takes plain integers and has no memory effects; the\n// group id is our own unreaped child's pid (set by `process_group(0)`),\n// so it cannot name an unrelated group. Errors (ESRCH: group already\n// gone) are expected and ignored.\nunsafe {\nlibc::killpg(pgid, libc::SIGKILL);\n}\n}\n}",
        );
        assert!(extract_unsafe_without_safety(&parsed).is_empty());
    }

    // The install.rs `is_dir_at` shape: one block behind a wrapped multiline
    // comment (above a plain let) and one behind a call wrapper.
    #[test]
    fn safety_comment_above_a_call_wrapped_block_is_accepted() {
        let parsed = parse(
            "fn is_dir_at(dir: &OwnedFd, name: &CStr) -> io::Result<bool> {\n// SAFETY: zeroed is a valid bit pattern for `stat`, which `fstatat`\n// fills in before it is read.\nlet mut st: libc::stat = unsafe { std::mem::zeroed() };\n// SAFETY: valid descriptor, NUL-terminated name, and a writable stat.\ncheck(unsafe {\nlibc::fstatat(dir.as_raw_fd(), name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW)\n})?;\nOk((st.st_mode & libc::S_IFMT) == libc::S_IFDIR)\n}",
        );
        assert!(extract_unsafe_without_safety(&parsed).is_empty());
    }

    // A multi-line justification inside the block's own body counts too.
    #[test]
    fn multiline_safety_comment_leading_inside_the_block_is_accepted() {
        let parsed = parse(
            "fn f(p: *const u8) { unsafe {\n// SAFETY: the pointer is valid for the call's duration.\n// The caller upholds it.\n*p;\n} }",
        );
        assert!(extract_unsafe_without_safety(&parsed).is_empty());
    }

    // An attribute between a comment and the block breaks adjacency, exactly
    // as any other non-comment sibling does (old behavior preserved).
    #[test]
    fn an_attribute_between_comment_and_block_breaks_association() {
        let parsed = parse(
            "fn f(p: *const u8) {\n// SAFETY: the caller guarantees validity.\n#[allow(unused_unsafe)]\nunsafe { *p; }\n}",
        );
        assert_eq!(extract_unsafe_without_safety(&parsed), vec!["f"]);
    }

    // String and raw-string contents are data, not justifications: the old
    // 200-byte body scan accepted a marker inside either.
    #[test]
    fn a_marker_inside_a_string_or_raw_string_does_not_excuse_the_block() {
        let raw =
            parse("fn f(p: *const u8) { unsafe { let s = r\"SAFETY: not a comment\"; *p; } }");
        let literal =
            parse("fn g(p: *const u8) { unsafe { let s = \"// SAFETY: not a comment\"; *p; } }");
        assert_eq!(extract_unsafe_without_safety(&raw), vec!["f"]);
        assert_eq!(extract_unsafe_without_safety(&literal), vec!["g"]);
    }

    // A comment above an intervening statement belongs to that statement.
    #[test]
    fn a_comment_detached_by_a_statement_does_not_excuse_the_block() {
        let parsed = parse(
            "fn f(p: *const u8) {\n// SAFETY: explains the statement below, not the block.\nlet x = 1;\nunsafe { *p; }\n}",
        );
        assert_eq!(extract_unsafe_without_safety(&parsed), vec!["f"]);
    }

    // Each block needs its own justification: a comment directly above one
    // block says nothing about the next.
    #[test]
    fn a_comment_for_a_neighboring_unsafe_block_does_not_excuse_this_one() {
        let parsed = parse(
            "fn f(p: *const u8) {\n// SAFETY: the caller guarantees validity for the first block.\nunsafe { *p; }\nunsafe { *p; }\n}",
        );
        assert_eq!(extract_unsafe_without_safety(&parsed), vec!["f"]);
    }

    // The old line scan treated a blank previous line as no comment; the
    // structural run keeps that break.
    #[test]
    fn a_blank_line_between_comment_and_block_breaks_association() {
        let parsed = parse(
            "fn f(p: *const u8) {\n// SAFETY: the caller guarantees validity.\n\nunsafe { *p; }\n}",
        );
        assert_eq!(extract_unsafe_without_safety(&parsed), vec!["f"]);
    }

    // A SAFETY comment naming the module does not reach an unsafe block
    // nested in a function inside that module: statement-body scope ends the
    // ancestor search.
    #[test]
    fn a_module_level_safety_comment_does_not_excuse_an_inner_block() {
        let parsed = parse(
            "// SAFETY: this comment describes module metadata, not function internals.\nmod container {\nfn f(p: *const u8) {\nunsafe { *p; }\n}\n}",
        );
        assert_eq!(extract_unsafe_without_safety(&parsed), vec!["f"]);
    }

    // An outer block's justification does not excuse a nested block: each
    // unsafe block is justified at its own level.
    #[test]
    fn an_outer_blocks_comment_does_not_excuse_a_nested_block() {
        let parsed = parse(
            "fn f(p: *const u8) {\n// SAFETY: justifies the outer block only.\nunsafe { unsafe { *p; } }\n}",
        );
        assert_eq!(extract_unsafe_without_safety(&parsed), vec!["f"]);
    }

    // A comment about creating a closure says nothing about what the closure
    // body later does; the closure's body block is a statement-body boundary.
    #[test]
    fn a_closure_creation_comment_does_not_excuse_the_closure_body() {
        let parsed = parse(
            "fn f(p: *const u8) {\n// SAFETY: creating the closure is safe; nothing justifies its body.\nlet g = || { unsafe { *p; } };\nlet _ = g;\n}",
        );
        assert_eq!(extract_unsafe_without_safety(&parsed), vec!["f"]);
    }

    // Control: a trailing comment belongs to the statement it follows, not
    // to the unsafe block below.
    #[test]
    fn a_trailing_comment_on_a_preceding_statement_does_not_excuse_the_block() {
        let parsed =
            parse("fn f(p: *const u8) {\nlet x = 1; // note about the let\nunsafe { *p; }\n}");
        assert_eq!(extract_unsafe_without_safety(&parsed), vec!["f"]);
    }

    // A genuine multi-line `/* SAFETY: ... */` block comment directly above
    // the block is one comment node and counts.
    #[test]
    fn a_multiline_block_comment_above_the_block_is_accepted() {
        let parsed = parse(
            "fn f(p: *const u8) {\n/* SAFETY: the pointer stays valid for the duration\nof the call, as the caller guarantees. */\nunsafe { *p; }\n}",
        );
        assert!(extract_unsafe_without_safety(&parsed).is_empty());
    }

    // Multibyte comment text must not break byte-offset adjacency.
    #[test]
    fn a_non_ascii_safety_comment_is_accepted() {
        let parsed = parse(
            "fn f(p: *const u8) {\n// SAFETY: gültig für die Dauer des Aufrufs, der Aufrufer garantiert es.\nunsafe { *p; }\n}",
        );
        assert!(extract_unsafe_without_safety(&parsed).is_empty());
    }

    // CRLF line endings keep comments adjacent (gap counting is LF-based).
    #[test]
    fn a_crlf_terminated_safety_comment_is_accepted() {
        let parsed = parse(
            "fn f(p: *const u8) {\r\n// SAFETY: the caller guarantees validity.\r\nunsafe { *p; }\r\n}",
        );
        assert!(extract_unsafe_without_safety(&parsed).is_empty());
    }

    // Body-level comments count wherever they sit inside the block: the old
    // body scan accepted trailing and interleaved comments, and that accepted
    // semantics is kept (only nested-statement comments are excluded).
    #[test]
    fn a_trailing_body_level_comment_is_accepted() {
        let parsed =
            parse("fn f(p: *const u8) { unsafe { *p; /* SAFETY: caller upholds validity. */ } }");
        assert!(extract_unsafe_without_safety(&parsed).is_empty());
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
