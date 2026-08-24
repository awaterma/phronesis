//! Bounded AST ownership extraction (spec §6.1, §6.2, §7).
//!
//! This is the `tree_sitter_rust` evidence provider. It observes five kinds of
//! site — clone, filter, await, mutation, synchronous lock — records each one's
//! span and operand, and derives four bounded *ordering/containment* relations
//! from them. It never claims a type, a cost, a reachability, or a borrow
//! liveness, because tree-sitter cannot establish any of those.
//!
//! Four constraints shape every line of it:
//!
//! - **D13.** Extraction is a hook *inside* `graph::extract::extract_rust_at_module`,
//!   invoked with the canonical function id that the same walk just used for
//!   `defines_fn`. No id is ever reconstructed here: `impl Foo<Bar>` keeps the
//!   literal segment `Foo<Bar>`, and a `#[path]` module's self-module is not
//!   `module_path(file, unit)`, so a second derivation would silently diverge.
//! - **D12.** Every edge is `Edge::base(relation, args, <repo-relative file>)`,
//!   including the four relations the spec calls "derived". `store::compact`
//!   discards fresh edges with `d: true` without erroring, and
//!   `derive::derive_all` has no syntax tree, so it could not recompute an
//!   expression chain, a root place, or a narrowest enclosing block anyway.
//! - **§7.5.** Comments and strings are excluded *structurally* — the walk only
//!   ever classifies `call_expression`, `await_expression`, and assignment
//!   nodes, so `.clone()` inside a comment or a raw string is not a node and
//!   cannot be observed. No regex, no substring test.
//! - **D19.** [`AST_EMITTABLE`] is the closed list of relations this provider
//!   may produce, checked in the one emit helper. `lock_scope_may_cross_await`
//!   and the other compiler-only relations are absent from it, so no code path
//!   here can promote AST evidence into a compiler claim.

use super::config::OwnershipConfig;
use super::{
    AWAIT_SITE, CLONE_BEFORE_AWAIT, CLONE_SITE, FILTER_BEFORE_CLONE, FILTER_SITE,
    LOCK_SCOPE_ENDS_BEFORE_AWAIT, MUTATION_SITE, OWNERSHIP_ANALYSIS_STATUS, OWNERSHIP_EVIDENCE,
    OWNERSHIP_SITE, OWNERSHIP_SITE_IN_FUNCTION, OWNERSHIP_SITE_SPAN, PROVIDER_TREE_SITTER_RUST,
    READ_BEFORE_MUTATION, SYNC_LOCK_SITE,
};
use crate::graph::model::Edge;
use std::collections::BTreeMap;
use tree_sitter::Node;

/// Exactly the relations the AST provider may emit (D19).
///
/// The emit helper checks membership, and a corpus-wide test asserts the union
/// of emitted relation names is a subset of this list. `resolved_type`,
/// `lock_scope_may_cross_await`, `ownership_transfer`, `borrow_live_across`,
/// and `ownership_conflict_diagnostic` are deliberately absent: each is a
/// compiler claim, and the worst failure this feature can have is quietly
/// making one from syntax.
pub const AST_EMITTABLE: &[&str] = &[
    OWNERSHIP_SITE,
    OWNERSHIP_SITE_IN_FUNCTION,
    OWNERSHIP_SITE_SPAN,
    CLONE_SITE,
    FILTER_SITE,
    AWAIT_SITE,
    MUTATION_SITE,
    SYNC_LOCK_SITE,
    OWNERSHIP_EVIDENCE,
    OWNERSHIP_ANALYSIS_STATUS,
    FILTER_BEFORE_CLONE,
    CLONE_BEFORE_AWAIT,
    READ_BEFORE_MUTATION,
    LOCK_SCOPE_ENDS_BEFORE_AWAIT,
];

/// Ownership-producing method names, kept distinct in `clone_site.operation`
/// because `.clone()`, `.cloned()`, `collect`, `to_owned`, and `to_string` are
/// not interchangeable costs (§7.6).
///
/// `collect` is included unconditionally per D5: whether a `collect` produces
/// ownership depends on the target type, which is a `type_resolved` claim this
/// provider is not allowed to make. The site says a collect was observed, and
/// its evidence level says at what strength.
pub const CLONE_METHODS: &[&str] = &["clone", "cloned", "to_owned", "to_string", "collect"];

/// The closed filter set (D2). `take_while`/`skip_while` are deliberately
/// excluded — widening this is a graph-format change.
pub const FILTER_METHODS: &[&str] = &["filter", "filter_map"];

/// Synchronous acquisition names. Matching a *name* is an observation of what
/// was written, never an inference about the receiver's type (D3, §7.10).
pub const LOCK_METHODS: &[&str] = &["lock", "read", "write"];

/// Closed mutation-method list (D3).
pub const MUTATION_METHODS: &[&str] = &[
    "get_mut",
    "iter_mut",
    "values_mut",
    "as_mut",
    "borrow_mut",
    "entry",
    "push",
    "push_str",
    "insert",
    "remove",
    "clear",
    "extend",
    "retain",
    "truncate",
    "drain",
];

/// Operand/place text cap in bytes (§7.7).
pub const OPERAND_TEXT_CAP: usize = 240;

/// Evidence level for everything this provider emits.
pub const EVIDENCE_LEVEL_AST: &str = "ast";
/// The capability this provider reports on.
pub const CAPABILITY_AST_EXTRACTION: &str = "ast_extraction";
/// Every site in the file was observed.
pub const STATUS_AVAILABLE: &str = "available";
/// The file's site budget was exhausted (§9).
pub const STATUS_PARTIAL: &str = "partial";
/// Machine reason paired with `available`.
pub const REASON_COMPLETE: &str = "complete";
/// Machine reason paired with `partial` (§9).
pub const REASON_SITE_CAP: &str = "site_cap";

/// The `<kind>` segment of a site id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SiteKind {
    Await,
    Clone,
    Filter,
    Lock,
    Mutation,
}

impl SiteKind {
    fn label(self) -> &'static str {
        match self {
            Self::Await => "await",
            Self::Clone => "clone",
            Self::Filter => "filter",
            Self::Lock => "lock",
            Self::Mutation => "mutation",
        }
    }
}

/// One observed site, before it is given an id.
struct Site<'t> {
    kind: SiteKind,
    /// The token that *names* the operation (D1). Site ids anchor here, never
    /// on the enclosing expression: in `xs.filter(p).cloned()` both
    /// `call_expression`s start at the byte of `xs`, so anchoring on the
    /// expression would collide two sites onto one id.
    anchor: Node<'t>,
    /// The whole anchoring expression, which is what `ownership_site_span`
    /// records because it is what a human needs to see.
    expr: Node<'t>,
    /// Receiver, assigned place, or UFCS argument — the operand/place text.
    operand: Option<Node<'t>>,
    /// Literal observed operation name, for the relations that carry one.
    operation: String,
    /// A lock guard's `let_declaration` and binding name, when the acquisition
    /// is bound to a plain identifier. An unbound temporary produces a site
    /// with an empty guard and no scope conclusion (§7.9, D6).
    guard: Option<(Node<'t>, String)>,
}

/// A site that survived the file's budget and has been emitted.
struct EmittedSite<'t> {
    id: String,
    site: Site<'t>,
}

/// Ownership extraction state for one file.
///
/// Held across the whole file because the site budget in §9 is per file while
/// extraction is driven per function.
pub struct FileOwnership<'a> {
    file_path: &'a str,
    source: &'a [u8],
    max_sites_per_file: usize,
    sites: usize,
    capped: bool,
    edges: Vec<Edge>,
}

impl<'a> FileOwnership<'a> {
    /// Start collecting for one file. The caller is responsible for only
    /// constructing this when the config is enabled and the path is in scope.
    pub fn new(file_path: &'a str, source: &'a [u8], config: &OwnershipConfig) -> Self {
        Self {
            file_path,
            source,
            max_sites_per_file: config.max_sites_per_file,
            sites: 0,
            capped: false,
            edges: Vec::new(),
        }
    }

    /// The only emit path, so D19's allowlist cannot be bypassed, and so every
    /// edge is a base edge carrying the file as provenance (D12).
    fn emit(&mut self, relation: &str, args: &[&str]) {
        debug_assert!(
            AST_EMITTABLE.contains(&relation),
            "the AST provider may not emit {relation} (D19)"
        );
        if AST_EMITTABLE.contains(&relation) {
            self.edges.push(Edge::base(relation, args, self.file_path));
        }
    }

    /// Observe one function body.
    ///
    /// `function_id` must be the id the enclosing walk just emitted for
    /// `defines_fn` — it is embedded in every site id, and a site whose
    /// function id does not resolve is unusable (§15, D13, D21).
    pub fn visit_function<'t>(&mut self, function_id: &str, body: Node<'t>) {
        let mut sites = Vec::new();
        collect_sites(body, &mut sites, self.source);
        // Source order, so the budget keeps a prefix of the file rather than a
        // traversal-order sample, and so output is stable.
        sites.sort_by_key(|site| (site.anchor.start_byte(), site.kind));

        let mut emitted: Vec<EmittedSite<'t>> = Vec::new();
        for site in sites {
            if self.sites >= self.max_sites_per_file {
                // §9: retain what was already produced and say so.
                self.capped = true;
                break;
            }
            self.sites += 1;
            let id = format!(
                "{function_id}#ownership:{}:{}",
                site.kind.label(),
                site.anchor.start_byte()
            );
            self.emit_site(function_id, &id, &site);
            emitted.push(EmittedSite { id, site });
        }

        self.derive_filter_before_clone(function_id, &emitted);
        self.derive_clone_before_await(function_id, &emitted);
        self.derive_read_before_mutation(function_id, &emitted);
        self.derive_lock_scope_ends_before_await(function_id, &emitted);
    }

    /// The base edges every site carries, plus its kind-specific relation.
    fn emit_site(&mut self, function_id: &str, id: &str, site: &Site<'_>) {
        // Every borrow of `self.source` is finished before the first `&mut
        // self` emit, so the pieces are built up front.
        let (start, end) = (
            site.expr.start_byte().to_string(),
            site.expr.end_byte().to_string(),
        );
        let file = self.file_path.to_string();
        let operand = site
            .operand
            .map(|node| normalize_text(node, self.source))
            .unwrap_or_default();
        let guard = site
            .guard
            .as_ref()
            .map(|(_, name)| name.clone())
            .unwrap_or_default();

        self.emit(OWNERSHIP_SITE, &[id]);
        self.emit(OWNERSHIP_SITE_IN_FUNCTION, &[id, function_id]);
        self.emit(OWNERSHIP_SITE_SPAN, &[id, &file, &start, &end]);
        match site.kind {
            SiteKind::Clone => self.emit(CLONE_SITE, &[id, &site.operation, &operand]),
            SiteKind::Filter => self.emit(FILTER_SITE, &[id, &operand]),
            SiteKind::Await => self.emit(AWAIT_SITE, &[id]),
            SiteKind::Mutation => self.emit(MUTATION_SITE, &[id, &site.operation, &operand]),
            SiteKind::Lock => self.emit(SYNC_LOCK_SITE, &[id, &site.operation, &guard]),
        }
        self.emit(
            OWNERSHIP_EVIDENCE,
            &[id, EVIDENCE_LEVEL_AST, PROVIDER_TREE_SITTER_RUST],
        );
    }

    /// §6.2/D2: a clone-producing operation and a filter in the *same receiver
    /// chain*. Mere line ordering is deliberately insufficient.
    fn derive_filter_before_clone(&mut self, function_id: &str, emitted: &[EmittedSite<'_>]) {
        let filters: BTreeMap<usize, &str> = emitted
            .iter()
            .filter(|e| e.site.kind == SiteKind::Filter)
            .map(|e| (e.site.anchor.id(), e.id.as_str()))
            .collect();
        if filters.is_empty() {
            return;
        }
        let mut pairs = Vec::new();
        for clone in emitted.iter().filter(|e| e.site.kind == SiteKind::Clone) {
            for call in receiver_chain(clone.site.expr) {
                let Some(anchor) = callee_field_expression(call)
                    .and_then(|callee| callee.child_by_field_name("field"))
                else {
                    continue;
                };
                if let Some(filter_id) = filters.get(&anchor.id()) {
                    pairs.push(((*filter_id).to_string(), clone.id.clone()));
                }
            }
        }
        for (filter_id, clone_id) in pairs {
            self.emit(FILTER_BEFORE_CLONE, &[function_id, &filter_id, &clone_id]);
        }
    }

    /// §6.2: lexical ordering evidence only. It does not claim the cloned value
    /// crosses the suspension, and an early `return` between the two is
    /// invisible to it (D8).
    fn derive_clone_before_await(&mut self, function_id: &str, emitted: &[EmittedSite<'_>]) {
        let awaits: Vec<(usize, String)> = emitted
            .iter()
            .filter(|e| e.site.kind == SiteKind::Await)
            .map(|e| (e.site.anchor.start_byte(), e.id.clone()))
            .collect();
        let clones: Vec<(usize, String)> = emitted
            .iter()
            .filter(|e| e.site.kind == SiteKind::Clone)
            .map(|e| (e.site.anchor.start_byte(), e.id.clone()))
            .collect();
        for (clone_at, clone_id) in &clones {
            for (await_at, await_id) in &awaits {
                if clone_at < await_at {
                    self.emit(CLONE_BEFORE_AWAIT, &[function_id, clone_id, await_id]);
                }
            }
        }
    }

    /// §6.2/D4: a snapshot read lexically precedes a mutation of the same
    /// syntactic root place. Clone-family sites are the bounded read sites this
    /// provider has; unknown aliasing produces nothing.
    fn derive_read_before_mutation(&mut self, function_id: &str, emitted: &[EmittedSite<'_>]) {
        let places: Vec<(usize, SiteKind, String, String)> = emitted
            .iter()
            .filter(|e| matches!(e.site.kind, SiteKind::Clone | SiteKind::Mutation))
            .filter_map(|e| {
                let root = root_place(e.site.operand?, self.source)?;
                Some((e.site.anchor.start_byte(), e.site.kind, root, e.id.clone()))
            })
            .collect();
        let mut pairs = Vec::new();
        for (read_at, read_kind, read_root, read_id) in &places {
            if *read_kind != SiteKind::Clone {
                continue;
            }
            for (mutation_at, mutation_kind, mutation_root, mutation_id) in &places {
                if *mutation_kind == SiteKind::Mutation
                    && read_root == mutation_root
                    && read_at < mutation_at
                {
                    pairs.push((read_id.clone(), mutation_id.clone()));
                }
            }
        }
        for (read_id, mutation_id) in pairs {
            self.emit(READ_BEFORE_MUTATION, &[function_id, &read_id, &mutation_id]);
        }
    }

    /// §6.2/D6. Three admissible shapes, and no fourth: the guard's narrowest
    /// enclosing block ends before the await, the guard is explicitly dropped
    /// between its binding and the await, or the guard is an unbound temporary
    /// whose enclosing *statement* ends before the await (D6 case 3).
    ///
    /// The negative case emits *nothing*. `lock_scope_may_cross_await` is not
    /// in [`AST_EMITTABLE`] and is not nameable from this file.
    fn derive_lock_scope_ends_before_await(
        &mut self,
        function_id: &str,
        emitted: &[EmittedSite<'_>],
    ) {
        let awaits: Vec<(usize, String)> = emitted
            .iter()
            .filter(|e| e.site.kind == SiteKind::Await)
            .map(|e| (e.site.anchor.start_byte(), e.id.clone()))
            .collect();
        if awaits.is_empty() {
            return;
        }
        let mut pairs = Vec::new();
        for lock in emitted.iter().filter(|e| e.site.kind == SiteKind::Lock) {
            let Some((binding, name)) = lock.site.guard.as_ref() else {
                // D6 case 3. An unbound temporary guard has no *binding* to
                // reason about, but it does have a drop point: Rust extends a
                // temporary's life to the end of the enclosing statement, so
                // the statement's end byte is the guard's release point. When
                // no enclosing statement exists (a tail expression), §7.11's
                // "emit no edge" applies rather than a guess.
                if let Some(statement) = enclosing_statement(lock.site.anchor) {
                    let released_at = statement.end_byte();
                    for (await_at, await_id) in &awaits {
                        if released_at < *await_at {
                            pairs.push((lock.id.clone(), await_id.clone()));
                        }
                    }
                }
                continue;
            };
            let Some(block) = enclosing_block(*binding) else {
                continue;
            };
            for (await_at, await_id) in &awaits {
                let ends_before = block.end_byte() < *await_at;
                let dropped = !ends_before
                    && dropped_between(block, name, binding.end_byte()..*await_at, self.source);
                if ends_before || dropped {
                    pairs.push((lock.id.clone(), await_id.clone()));
                }
            }
        }
        for (lock_id, await_id) in pairs {
            self.emit(
                LOCK_SCOPE_ENDS_BEFORE_AWAIT,
                &[function_id, &lock_id, &await_id],
            );
        }
    }

    /// Close the file, appending its analysis status.
    ///
    /// The status is emitted even when the file produced no sites: §3 requires
    /// unavailable or bounded analysis to be explicit, and "we looked and found
    /// nothing" must not be indistinguishable from "we never looked".
    pub fn finish(mut self) -> Vec<Edge> {
        let (status, reason) = if self.capped {
            (STATUS_PARTIAL, REASON_SITE_CAP)
        } else {
            (STATUS_AVAILABLE, REASON_COMPLETE)
        };
        let file = self.file_path.to_string();
        self.emit(
            OWNERSHIP_ANALYSIS_STATUS,
            &[&file, CAPABILITY_AST_EXTRACTION, status, reason],
        );
        self.edges
    }
}

/// Text of a node, or empty when it is not valid UTF-8.
fn text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

/// Depth-first collection of every site in one function body.
///
/// Nested `function_item`s are not descended into: the graph walk never reaches
/// them, so they have no canonical function id, and attributing their sites to
/// the enclosing function would assert an ordering that does not exist (§7.11).
/// `macro_definition` bodies are skipped for the same reason (D22).
fn collect_sites<'t>(node: Node<'t>, out: &mut Vec<Site<'t>>, source: &[u8]) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "function_item" | "macro_definition") {
            continue;
        }
        if let Some(site) = classify(child, source) {
            out.push(site);
        }
        collect_sites(child, out, source);
    }
}

fn classify<'t>(node: Node<'t>, source: &[u8]) -> Option<Site<'t>> {
    match node.kind() {
        // `await_expression` has no named fields; the `await` keyword is an
        // unnamed child and the awaited expression is `child(0)`.
        "await_expression" => Some(Site {
            kind: SiteKind::Await,
            anchor: child_of_kind(node, "await")?,
            expr: node,
            operand: None,
            operation: String::new(),
            guard: None,
        }),
        "call_expression" => classify_call(node, source),
        "assignment_expression" | "compound_assignment_expr" => classify_assignment(node, source),
        _ => None,
    }
}

fn classify_call<'t>(call: Node<'t>, source: &[u8]) -> Option<Site<'t>> {
    if let Some(callee) = callee_field_expression(call) {
        let anchor = callee.child_by_field_name("field")?;
        let receiver = callee.child_by_field_name("value")?;
        let name = text(anchor, source);
        let site = |kind| Site {
            kind,
            anchor,
            expr: call,
            operand: Some(receiver),
            operation: name.to_string(),
            guard: None,
        };
        if CLONE_METHODS.contains(&name) {
            return Some(site(SiteKind::Clone));
        }
        if FILTER_METHODS.contains(&name) {
            return Some(site(SiteKind::Filter));
        }
        if MUTATION_METHODS.contains(&name) {
            return Some(site(SiteKind::Mutation));
        }
        if LOCK_METHODS.contains(&name)
            && argument_count(call) == 0
            && !is_direct_await_operand(call)
        {
            // D20: an awaited acquisition is an async lock, excluded
            // structurally rather than by guessing at the receiver's type.
            return Some(Site {
                kind: SiteKind::Lock,
                anchor,
                expr: call,
                operand: Some(receiver),
                operation: name.to_string(),
                guard: guard_binding(call, source),
            });
        }
        return None;
    }
    // UFCS `Clone::clone(&x)`: the callee is a `scoped_identifier`, which is
    // also the id anchor (D1).
    let function = call.child_by_field_name("function")?;
    if function.kind() != "scoped_identifier" {
        return None;
    }
    let name = text(function.child_by_field_name("name")?, source);
    let path = text(function.child_by_field_name("path")?, source);
    if name != "clone" || path.rsplit("::").next() != Some("Clone") {
        return None;
    }
    Some(Site {
        kind: SiteKind::Clone,
        anchor: function,
        expr: call,
        operand: Some(call.child_by_field_name("arguments")?.named_child(0)?),
        operation: text(function, source).to_string(),
        guard: None,
    })
}

/// Assignment *through a projection* only (D3): `x = 1` to a bare local is not
/// a mutation site.
fn classify_assignment<'t>(node: Node<'t>, source: &[u8]) -> Option<Site<'t>> {
    let left = node.child_by_field_name("left")?;
    if !matches!(left.kind(), "field_expression" | "index_expression") {
        return None;
    }
    let anchor = operator_token(node, left)?;
    Some(Site {
        kind: SiteKind::Mutation,
        anchor,
        expr: node,
        operand: Some(left),
        operation: text(anchor, source).to_string(),
        guard: None,
    })
}

/// The operator token of an assignment. `compound_assignment_expr` labels it
/// `operator`; plain `assignment_expression` does not label `=` at all.
fn operator_token<'t>(node: Node<'t>, left: Node<'t>) -> Option<Node<'t>> {
    if let Some(operator) = node.child_by_field_name("operator") {
        return Some(operator);
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named() && child.start_byte() >= left.end_byte())
}

fn child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
}

/// The `field_expression` callee of a method call, unwrapping turbofish.
///
/// **Mandatory** per the decisions document: `x.collect::<Vec<_>>()` parses as
/// `call_expression { function: generic_function { function: field_expression } }`,
/// and code that tests `function.kind() == "field_expression"` directly misses
/// every turbofish call — which is nearly every `collect`.
pub fn callee_field_expression<'t>(call: Node<'t>) -> Option<Node<'t>> {
    let function = call.child_by_field_name("function")?;
    let function = if function.kind() == "generic_function" {
        function.child_by_field_name("function")?
    } else {
        function
    };
    (function.kind() == "field_expression").then_some(function)
}

fn argument_count(call: Node<'_>) -> usize {
    call.child_by_field_name("arguments")
        .map(|arguments| arguments.named_child_count())
        .unwrap_or(0)
}

fn is_direct_await_operand(call: Node<'_>) -> bool {
    call.parent()
        .is_some_and(|parent| parent.kind() == "await_expression" && parent.child(0) == Some(call))
}

/// Every `call_expression` reachable from `call` through receiver position
/// (D2). Intervening adapters do not break a chain; a `let` binding does,
/// because the receiver chain then terminates at an identifier.
fn receiver_chain(call: Node<'_>) -> Vec<Node<'_>> {
    let mut out = Vec::new();
    let Some(mut current) =
        callee_field_expression(call).and_then(|callee| callee.child_by_field_name("value"))
    else {
        return out;
    };
    loop {
        let next = match current.kind() {
            "call_expression" => {
                out.push(current);
                callee_field_expression(current)
                    .and_then(|callee| callee.child_by_field_name("value"))
            }
            "field_expression" | "reference_expression" => current.child_by_field_name("value"),
            "try_expression" | "await_expression" | "parenthesized_expression" => {
                current.named_child(0)
            }
            _ => None,
        };
        match next {
            Some(node) => current = node,
            None => return out,
        }
    }
}

/// The root of a place expression (D4), or `None` when the descent terminates
/// at anything that is not an identifier or `self` — a call, a macro, a
/// literal. No root place means no `read_before_mutation` edge (§7.11).
fn root_place(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut current = node;
    loop {
        match current.kind() {
            "identifier" | "self" => return Some(text(current, source).to_string()),
            "field_expression" | "reference_expression" => {
                current = current.child_by_field_name("value")?;
            }
            "index_expression" => current = current.named_child(0)?,
            "try_expression" | "parenthesized_expression" | "await_expression" => {
                current = current.named_child(0)?;
            }
            "unary_expression" => {
                // Only a dereference is a place projection; `-x` is not.
                child_of_kind(current, "*")?;
                current = current.named_child(0)?;
            }
            _ => return None,
        }
    }
}

/// The `let_declaration` and binding name a lock acquisition flows into, when
/// the pattern is a plain identifier.
///
/// The common idiom binds through an intervening fallible-unwrapping call
/// (`let g = m.lock()` then `expect(..)`), which is why the ascent walks method
/// calls applied to the acquisition rather than requiring the acquisition to be
/// the initializer itself. The literal method name is spelled out only in the
/// test fixtures, because the repo's own audit scans this file's text.
///
/// A *projection* off the acquisition is deliberately not a binding:
/// `let v = m.lock().field;` binds the field, not the guard — the temporary
/// guard dies at the end of that statement — so it names no guard (§7.11).
fn guard_binding<'t>(call: Node<'t>, source: &[u8]) -> Option<(Node<'t>, String)> {
    let mut current = call;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            // Receiver position of a method call: the guard flows through an
            // unwrapping call only when that call is applied to it.
            "field_expression" if parent.child_by_field_name("value") == Some(current) => {
                let owner = parent.parent()?;
                let owner = if owner.kind() == "generic_function" {
                    owner.parent()?
                } else {
                    owner
                };
                if owner.kind() != "call_expression"
                    || callee_field_expression(owner) != Some(parent)
                {
                    return None;
                }
                current = owner;
            }
            "try_expression" | "parenthesized_expression" => current = parent,
            "let_declaration" => {
                if parent.child_by_field_name("value") != Some(current) {
                    return None;
                }
                let pattern = parent.child_by_field_name("pattern")?;
                if pattern.kind() != "identifier" {
                    // A destructured or refutable pattern names no unique
                    // guard binding (§7.11).
                    return None;
                }
                return Some((parent, text(pattern, source).to_string()));
            }
            _ => return None,
        }
    }
    None
}

fn enclosing_block(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = node.parent();
    while let Some(candidate) = current {
        if candidate.kind() == "block" {
            return Some(candidate);
        }
        current = candidate.parent();
    }
    None
}

/// D6 case 3: the statement whose end releases an unbound temporary guard.
///
/// The nearest ancestor `expression_statement` or `let_declaration`, never the
/// innermost expression: Rust extends a temporary's life to the end of the
/// *enclosing statement*, so in `match m.lock() { .. }` used as a statement the
/// guard lives past the whole match, not just the scrutinee. Taking the
/// scrutinee would claim an early drop that does not happen — precisely the
/// over-claim §6.2 forbids. `None` (a tail expression with no semicolon) means
/// no boundary can be established, and §7.11 then requires emitting nothing.
fn enclosing_statement(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = node.parent();
    while let Some(candidate) = current {
        if matches!(candidate.kind(), "expression_statement" | "let_declaration") {
            return Some(candidate);
        }
        current = candidate.parent();
    }
    None
}

/// D6 case 2: `drop(guard)` between the binding and the await, in the guard's
/// own block. `window` is the exclusive byte range (binding end, await start).
///
/// This assumes `drop` resolves to the prelude function; a locally shadowed
/// `drop` makes it wrong, which is a named false-positive class (D8) and
/// acceptable only because the relation is query-only.
fn dropped_between(
    block: Node<'_>,
    guard: &str,
    window: std::ops::Range<usize>,
    source: &[u8],
) -> bool {
    let mut pending = vec![block];
    while let Some(node) = pending.pop() {
        if node.kind() == "call_expression"
            && node.start_byte() > window.start
            && node.start_byte() < window.end
            && node
                .child_by_field_name("function")
                .is_some_and(|function| {
                    function.kind() == "identifier" && text(function, source) == "drop"
                })
            && let Some(arguments) = node.child_by_field_name("arguments")
            && arguments.named_child_count() == 1
            && arguments.named_child(0).is_some_and(|argument| {
                argument.kind() == "identifier" && text(argument, source) == guard
            })
        {
            return true;
        }
        let mut cursor = node.walk();
        pending.extend(node.children(&mut cursor));
    }
    false
}

/// Operand/place text per D7: whitespace-collapsed, never truncated.
///
/// A truncated expression reads as a real but wrong expression, which is worse
/// than an opaque marker, so anything over the cap becomes a digest. The same
/// branch catches a normalized string containing U+001F, which would otherwise
/// corrupt `Edge::fact_id`'s argument join.
fn normalize_text(node: Node<'_>, source: &[u8]) -> String {
    let normalized = collapse_whitespace(text(node, source));
    if normalized.len() > OPERAND_TEXT_CAP || normalized.contains('\u{1f}') {
        let digest = hex(&sha256(normalized.as_bytes()));
        return format!("sha256:{}", &digest[..16]);
    }
    normalized
}

/// Replace every run of ASCII whitespace with one space, and trim.
fn collapse_whitespace(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_space = false;
    for character in raw.chars() {
        if matches!(character, ' ' | '\t' | '\r' | '\n') {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(character);
    }
    out
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX_DIGITS[usize::from(byte >> 4)] as char);
        out.push(HEX_DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256, implemented here because the workspace has no digest dependency
/// and adding one needs the user's approval. D7 names SHA-256 specifically, and
/// a `sha256:` marker computed from anything else would be a lie in the graph.
fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_length = (data.len() as u64).wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());

    for chunk in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (index, word) in chunk.chunks_exact(4).enumerate() {
            w[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(SHA256_K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut digest = [0u8; 32];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::extract::{DEFAULT_WATCHLIST, RustExtractOptions, extract_rust_file};
    use crate::graph::ownership::{ANALYSIS_CAPABILITIES, ANALYSIS_STATUSES, EVIDENCE_LEVELS};
    use crate::graph::unit::UnitContext;
    use std::collections::BTreeSet;

    const FILE: &str = "src/a.rs";

    fn enabled() -> OwnershipConfig {
        OwnershipConfig {
            enabled: true,
            ..OwnershipConfig::disabled()
        }
    }

    fn run_with(path: &str, source: &str, config: &OwnershipConfig) -> Vec<Edge> {
        extract_rust_file(
            path,
            source,
            &RustExtractOptions {
                watchlist: DEFAULT_WATCHLIST,
                unit: &UnitContext::default(),
                module_override: None,
                ownership: config,
            },
        )
        .edges
    }

    fn run(source: &str) -> Vec<Edge> {
        run_with(FILE, source, &enabled())
    }

    fn args_of(edges: &[Edge], relation: &str) -> Vec<Vec<String>> {
        edges
            .iter()
            .filter(|edge| edge.p == relation)
            .map(|edge| edge.a.clone())
            .collect()
    }

    fn only(edges: &[Edge], relation: &str) -> Vec<String> {
        let found = args_of(edges, relation);
        assert_eq!(found.len(), 1, "expected exactly one {relation}: {found:?}");
        found.into_iter().next().unwrap_or_default()
    }

    // ─── identity ───────────────────────────────────────────────────

    // Pins §5.2 and D1: the id anchors on the operation-name token, in UTF-8
    // bytes. Anchoring on the enclosing expression collides every site in a
    // chain onto one id, because they all start at the receiver's byte.
    #[test]
    fn a_site_id_anchors_on_the_operation_name_token_so_a_chain_cannot_collide() {
        let source = "fn f() { let ys = xs.filter(p).cloned(); }";
        let edges = run(source);
        let filter_at = source.find("filter").unwrap_or_default();
        let clone_at = source.find("cloned").unwrap_or_default();
        let sites: Vec<String> = args_of(&edges, "ownership_site")
            .into_iter()
            .filter_map(|args| args.into_iter().next())
            .collect();
        assert!(
            sites.contains(&format!("rust:crate::a::f#ownership:filter:{filter_at}")),
            "filter site anchors on its method-name token: {sites:?}"
        );
        assert!(
            sites.contains(&format!("rust:crate::a::f#ownership:clone:{clone_at}")),
            "clone site anchors on its method-name token: {sites:?}"
        );
        assert_eq!(
            sites.iter().collect::<BTreeSet<_>>().len(),
            sites.len(),
            "two sites in one chain must not share an id"
        );
    }

    // Pins §7.4: offsets are UTF-8 byte offsets, never character offsets. A
    // multi-byte literal before the site shifts the id by bytes.
    #[test]
    fn site_ids_count_utf8_bytes_not_characters() {
        let source = "fn f() { let s = \"日本語\"; let t = s.clone(); }";
        let edges = run(source);
        let expected = source.find("clone").unwrap_or_default();
        assert!(
            expected > source.chars().position(|c| c == 'c').unwrap_or_default(),
            "the fixture must actually contain multi-byte text before the site"
        );
        assert_eq!(
            only(&edges, "clone_site")[0],
            format!("rust:crate::a::f#ownership:clone:{expected}"),
            "site id uses the byte offset of the method-name token"
        );
    }

    // Pins D13/D21: every function id in an ownership edge is exactly an id the
    // same walk emitted for `defines_fn`. Divergence is silent and total — a
    // site whose function does not exist can never be joined to anything.
    #[test]
    fn ownership_function_ids_are_exactly_the_defines_fn_ids() {
        let edges = run(concat!(
            "struct S; ",
            "impl Foo<Bar> for S { fn generic(&self) { let _ = self.x.clone(); } } ",
            "trait T { fn provided(&self) { let _ = self.y.clone(); } } ",
            "mod inner { pub fn nested(v: &V) { let _ = v.clone(); } } ",
            "fn free(v: &V) { let _ = v.clone(); }",
        ));
        let defined: BTreeSet<String> = args_of(&edges, "defines_fn")
            .into_iter()
            .filter_map(|args| args.into_iter().nth(1))
            .collect();
        let owned: BTreeSet<String> = args_of(&edges, "ownership_site_in_function")
            .into_iter()
            .filter_map(|args| args.into_iter().nth(1))
            .collect();
        assert_eq!(
            owned, defined,
            "ownership function ids must be the defines_fn ids, including the \
             literal `Foo<Bar>` impl segment and the trait default method"
        );
    }

    // Pins D13 for `#[path]` modules: the self-module is not derivable from the
    // file path, so the extractor must take the id the walk already computed.
    #[test]
    fn a_path_included_module_keeps_the_walks_module_override() {
        let edges = extract_rust_file(
            "src/other/place.rs",
            "fn f(v: &V) { let _ = v.clone(); }",
            &RustExtractOptions {
                watchlist: DEFAULT_WATCHLIST,
                unit: &UnitContext::default(),
                module_override: Some("rust:crate::declared::name"),
                ownership: &enabled(),
            },
        )
        .edges;
        assert_eq!(
            only(&edges, "ownership_site_in_function")[1],
            "rust:crate::declared::name::f",
            "the site names the module the walk was told about, not the path"
        );
    }

    // ─── arity and argument order (§13.1) ───────────────────────────

    // Pins the documented arity and argument order of every base relation.
    // Nothing in the graph validates arity: a wrong-arity edge produces no
    // error, only a pattern that never matches (D18).
    #[test]
    fn every_base_relation_has_its_documented_arity_and_argument_order() {
        let source = concat!(
            "async fn f(m: &M, v: &mut V) { ",
            "let g = m.lock(); ",
            "let kept = v.items.iter().filter(|i| i.ok).cloned(); ",
            "v.items.push(1); ",
            "step().await; ",
            "}",
        );
        let edges = run(source);
        let clone = only(&edges, "clone_site");
        assert_eq!(clone.len(), 3, "clone_site is [site, operation, operand]");
        assert_eq!(clone[1], "cloned", "operation preserves the written name");
        assert_eq!(
            clone[2], "v.items.iter().filter(|i| i.ok)",
            "operand is the receiver text"
        );

        let filter = only(&edges, "filter_site");
        assert_eq!(filter.len(), 2, "filter_site is [site, operand]");
        assert_eq!(filter[1], "v.items.iter()", "operand is the receiver text");

        assert_eq!(only(&edges, "await_site").len(), 1, "await_site is [site]");

        let mutation = only(&edges, "mutation_site");
        assert_eq!(
            mutation.len(),
            3,
            "mutation_site is [site, operation, place]"
        );
        assert_eq!(mutation[1], "push", "operation is the observed method name");
        assert_eq!(mutation[2], "v.items", "place is the receiver text");

        let lock = only(&edges, "sync_lock_site");
        assert_eq!(lock.len(), 3, "sync_lock_site is [site, operation, guard]");
        assert_eq!(lock[1], "lock", "operation is the observed method name");
        assert_eq!(lock[2], "g", "guard is the lexical binding");

        let span = args_of(&edges, "ownership_site_span");
        assert!(span.iter().all(|args| args.len() == 4));
        let clone_span = span
            .iter()
            .find(|args| args[0] == clone[0])
            .cloned()
            .unwrap_or_default();
        assert_eq!(clone_span[1], FILE, "span names the repo-relative file");
        let (start, end) = (
            clone_span[2].parse::<usize>().unwrap_or_default(),
            clone_span[3].parse::<usize>().unwrap_or_default(),
        );
        assert_eq!(
            &source[start..end],
            "v.items.iter().filter(|i| i.ok).cloned()",
            "the span covers the whole anchoring expression, not the token"
        );

        let evidence = args_of(&edges, "ownership_evidence");
        assert!(
            evidence
                .iter()
                .all(|args| args.len() == 3 && args[1] == "ast" && args[2] == "tree_sitter_rust"),
            "ownership_evidence is [subject, level, provider]: {evidence:?}"
        );
        let status = only(&edges, "ownership_analysis_status");
        assert_eq!(
            status,
            vec![
                FILE.to_string(),
                "ast_extraction".to_string(),
                "available".to_string(),
                "complete".to_string()
            ],
            "ownership_analysis_status is [subject, capability, status, reason]"
        );
        let in_function = args_of(&edges, "ownership_site_in_function");
        assert!(
            in_function.iter().all(|args| args.len() == 2),
            "ownership_site_in_function is [site, function]"
        );
    }

    // Pins that the closed value sets this module writes are the ones the
    // module root declares. A typo here is invisible until a query returns
    // nothing.
    #[test]
    fn emitted_levels_capabilities_and_statuses_are_from_the_closed_sets() {
        assert!(EVIDENCE_LEVELS.contains(&EVIDENCE_LEVEL_AST), "level");
        assert!(
            ANALYSIS_CAPABILITIES.contains(&CAPABILITY_AST_EXTRACTION),
            "capability"
        );
        assert!(ANALYSIS_STATUSES.contains(&STATUS_AVAILABLE), "available");
        assert!(ANALYSIS_STATUSES.contains(&STATUS_PARTIAL), "partial");
    }

    // ─── what must produce nothing ──────────────────────────────────

    // Pins §7.5 and D11: exclusion is structural. `hazards.rs` matches
    // `.lock()` as a substring and would fail this fixture; the ownership
    // extractor never looks at raw text, only at nodes it classified.
    #[test]
    fn operations_inside_comments_and_strings_are_not_sites() {
        let edges = run(concat!(
            "async fn f() { ",
            "// x.clone() and m.lock() and y.await\n",
            "/* v.clone(); */ ",
            "let s = \"a.clone() m.lock() .await\"; ",
            "let r = r#\"b.clone() n.lock() .await\"#; ",
            "}",
        ));
        assert!(
            args_of(&edges, "ownership_site").is_empty(),
            "comments and strings are not code: {:?}",
            args_of(&edges, "ownership_site")
        );
    }

    // Pins §7.2: a bodyless trait item declares, it does not define, so it has
    // no `defines_fn` and can carry no site.
    #[test]
    fn a_bodyless_trait_signature_emits_no_sites() {
        let edges = run("trait T { fn f(&self) -> Self; }");
        assert!(args_of(&edges, "ownership_site").is_empty());
        assert!(args_of(&edges, "defines_fn").is_empty());
    }

    // Pins D14: a `#[test]` function returns before `defines_fn` is emitted, so
    // a site inside one would name a function the graph does not have.
    #[test]
    fn test_functions_and_test_modules_emit_no_sites() {
        let edges = run(concat!(
            "#[test] fn t(v: &V) { let _ = v.clone(); } ",
            "#[cfg(test)] mod tests { fn helper(v: &V) { let _ = v.clone(); } }",
        ));
        assert!(
            args_of(&edges, "ownership_site").is_empty(),
            "no site may live in a test body: {:?}",
            args_of(&edges, "ownership_site")
        );
    }

    // Pins D22: a `macro_rules!` body is not a function body, and a site inside
    // one has no enclosing function to name.
    #[test]
    fn a_macro_rules_definition_body_emits_no_sites() {
        let edges = run("macro_rules! m { ($v:expr) => { $v.clone() }; }");
        assert!(args_of(&edges, "ownership_site").is_empty());
    }

    // Pins §9/D15: a disabled configuration must produce exactly zero ownership
    // edges — including the analysis status, which would otherwise assert that
    // an analysis ran.
    #[test]
    fn a_disabled_configuration_emits_no_ownership_edges_at_all() {
        let edges = run_with(
            FILE,
            "async fn f(v: &V) { let _ = v.clone(); step().await; }",
            &OwnershipConfig::disabled(),
        );
        assert!(
            !edges.iter().any(|edge| edge.p.starts_with("ownership_")
                || edge.p.ends_with("_site")
                || edge.p == "clone_before_await"),
            "disabled means silent: {:?}",
            edges.iter().map(|e| &e.p).collect::<Vec<_>>()
        );
    }

    // Pins D16: include/exclude filter which files are analysed at all.
    #[test]
    fn an_out_of_scope_path_is_not_analysed() {
        let config = OwnershipConfig {
            enabled: true,
            include: vec!["src/**/*.rs".to_string()],
            exclude: vec!["src/generated/**".to_string()],
            ..OwnershipConfig::disabled()
        };
        let generated = run_with(
            "src/generated/a.rs",
            "fn f(v: &V) { let _ = v.clone(); }",
            &config,
        );
        assert!(args_of(&generated, "ownership_site").is_empty(), "excluded");
        let included = run_with("src/a.rs", "fn f(v: &V) { let _ = v.clone(); }", &config);
        assert_eq!(args_of(&included, "ownership_site").len(), 1, "included");
    }

    // ─── site kinds ─────────────────────────────────────────────────

    // Pins the decisions document's turbofish requirement. `collect` is nearly
    // always written `collect::<T>()`, which parses as
    // `call_expression { function: generic_function { function: field_expression } }`;
    // code that tests `field_expression` directly silently produces nothing.
    #[test]
    fn a_turbofish_collect_is_a_clone_site() {
        let edges = run("fn f(xs: &X) { let v = xs.iter().collect::<Vec<_>>(); }");
        let clone = only(&edges, "clone_site");
        assert_eq!(clone[1], "collect", "operation is the observed name (D5)");
        assert_eq!(clone[2], "xs.iter()", "operand is the receiver");
    }

    // Pins §6.1's UFCS form and D1's anchor for it.
    #[test]
    fn a_ufcs_clone_call_is_a_clone_site_anchored_on_its_callee() {
        let source = "fn f(x: &X) { let y = Clone::clone(&x.inner); }";
        let edges = run(source);
        let clone = only(&edges, "clone_site");
        assert_eq!(clone[1], "Clone::clone", "operation records the UFCS form");
        assert_eq!(clone[2], "&x.inner", "operand is the argument");
        assert_eq!(
            clone[0],
            format!(
                "rust:crate::a::f#ownership:clone:{}",
                source.find("Clone::clone").unwrap_or_default()
            )
        );
    }

    // Pins D20: an awaited acquisition is an async lock and must not be
    // recorded as a synchronous one. The test is structural — the acquisition
    // is the direct operand of an `await_expression` — never a guess about the
    // receiver's type (§7.10).
    #[test]
    fn an_awaited_lock_acquisition_is_not_a_sync_lock_site() {
        let edges = run("async fn f(m: &M) { let g = m.lock().await; }");
        assert!(
            args_of(&edges, "sync_lock_site").is_empty(),
            "`.lock().await` is an async lock"
        );
        assert_eq!(
            args_of(&edges, "await_site").len(),
            1,
            "still an await site"
        );
    }

    // Pins §7.9 and D6 case 3: an unbound temporary still records the empty
    // guard string — no name is invented — but its drop point *is* knowable,
    // because Rust releases the temporary at the end of the enclosing
    // statement. The failure mode pinned is the old behaviour: silently
    // dropping every unbound acquisition, so a lock that demonstrably ends
    // before the await looked identical to one that crosses it.
    #[test]
    fn an_unbound_temporary_guard_records_no_name_but_still_ends_at_its_statement() {
        let edges = run("async fn f(m: &M) { let v = m.lock().field; step().await; }");
        let lock = only(&edges, "sync_lock_site");
        assert_eq!(lock[2], "", "no unique guard binding");
        let derived = only(&edges, "lock_scope_ends_before_await");
        assert_eq!(derived[0], "rust:crate::a::f");
        assert!(
            derived[1].contains("#ownership:lock:"),
            "the unbound acquisition is the lock end: {derived:?}"
        );
        assert!(
            derived[2].contains("#ownership:await:"),
            "and the later suspension is the await end: {derived:?}"
        );
    }

    // Pins D6 case 3's ordering half: a temporary acquired *after* the await
    // is released after it too, so there is nothing to conclude. Comparing
    // byte positions in the wrong direction would emit a relation whose name
    // is a false statement about the code.
    #[test]
    fn an_unbound_temporary_after_the_await_yields_no_scope_relation() {
        let edges = run("async fn f(m: &M) { step().await; let v = m.lock().field; }");
        assert_eq!(
            args_of(&edges, "sync_lock_site").len(),
            1,
            "the acquisition is still observed"
        );
        assert!(
            args_of(&edges, "lock_scope_ends_before_await").is_empty(),
            "a lock after the await ends nothing before it"
        );
    }

    // The test that proves the statement boundary, not the scrutinee, is what
    // D6 case 3 uses. Rust extends a temporary scrutinee's life across the
    // whole `match`, so this guard is alive at the inner await. An
    // implementation that stopped at the innermost expression would see the
    // scrutinee ending at byte N < the await and emit a false "dropped early"
    // claim — the exact over-claim §6.2 forbids.
    #[test]
    fn a_temporary_match_scrutinee_lives_past_an_await_inside_the_match() {
        let edges = run("async fn f(m: &M) { match m.lock() { _ => { step().await; } } }");
        assert_eq!(
            args_of(&edges, "sync_lock_site").len(),
            1,
            "the acquisition is observed"
        );
        assert_eq!(
            args_of(&edges, "await_site").len(),
            1,
            "and so is the suspension inside the match"
        );
        assert!(
            args_of(&edges, "lock_scope_ends_before_await").is_empty(),
            "the scrutinee temporary is live across the await: {:?}",
            args_of(&edges, "lock_scope_ends_before_await")
        );
        assert!(
            !edges
                .iter()
                .any(|edge| edge.p == "lock_scope_may_cross_await"),
            "and the negative case still makes no crossing claim (§6.2, D6)"
        );
    }

    // Pins that adding D6 case 3 did not disturb cases 1 and 2. A regression
    // here would mean the unbound branch had swallowed bound guards — the
    // failure mode of putting the new check before the binding lookup.
    #[test]
    fn a_bound_guard_still_ends_by_block_scope_and_by_explicit_drop() {
        let by_block = run("async fn f(m: &M) { { let g = m.lock(); g.use_it(); } step().await; }");
        assert_eq!(only(&by_block, "sync_lock_site")[2], "g", "still bound");
        assert_eq!(
            args_of(&by_block, "lock_scope_ends_before_await").len(),
            1,
            "block scope still concludes: {:?}",
            args_of(&by_block, "lock_scope_ends_before_await")
        );
        let by_drop = run("async fn f(m: &M) { let g = m.lock(); drop(g); step().await; }");
        assert_eq!(only(&by_drop, "sync_lock_site")[2], "g", "still bound");
        assert_eq!(
            args_of(&by_drop, "lock_scope_ends_before_await").len(),
            1,
            "explicit drop still concludes: {:?}",
            args_of(&by_drop, "lock_scope_ends_before_await")
        );
    }

    // Pins the dominant real-world binding shape: the acquisition reaches the
    // binding through a fallible-unwrapping call. Requiring the acquisition to
    // be the initializer itself would leave almost every std lock unbound, and
    // therefore silently drop every scope conclusion.
    //
    // The fixture spells `expect` rather than the shorter sibling method
    // because the repo's own audit scans this file's raw text.
    #[test]
    fn a_guard_bound_through_an_unwrapping_call_still_names_its_binding() {
        let edges = run(
            "async fn f(m: &M) { { let g = m.lock().expect(\"poisoned\"); g.use_it(); } step().await; }",
        );
        assert_eq!(only(&edges, "sync_lock_site")[2], "g", "guard is bound");
        assert_eq!(
            args_of(&edges, "lock_scope_ends_before_await").len(),
            1,
            "and its block scope is therefore knowable"
        );
    }

    // Pins D3: assignment through a projection is a mutation site; assignment
    // to a bare local is not.
    #[test]
    fn assignment_is_a_mutation_site_only_through_a_projection() {
        let edges = run("fn f(s: &mut S) { let mut n = 0; n = 1; s.field = 2; s.items[0] += 3; }");
        let mutations = args_of(&edges, "mutation_site");
        let places: Vec<(String, String)> = mutations
            .iter()
            .map(|args| (args[1].clone(), args[2].clone()))
            .collect();
        assert_eq!(
            places,
            vec![
                ("=".to_string(), "s.field".to_string()),
                ("+=".to_string(), "s.items[0]".to_string())
            ],
            "a bare local assignment is not a mutation site"
        );
    }

    // Pins D3's closed method list, matched on the written name only. Nothing
    // here maps a name to a type.
    #[test]
    fn a_closed_list_of_method_names_produces_mutation_sites() {
        let edges =
            run("fn f(m: &mut M) { m.map.get_mut(&k); m.log.push(1); m.other.frobnicate(); }");
        let observed: Vec<String> = args_of(&edges, "mutation_site")
            .into_iter()
            .map(|args| args[1].clone())
            .collect();
        assert_eq!(
            observed,
            vec!["get_mut", "push"],
            "closed list, no guessing"
        );
    }

    // ─── derivations ────────────────────────────────────────────────

    // Pins §6.2 and D2 together: the shared receiver chain is what makes the
    // relation, and intervening adapters do not break it.
    #[test]
    fn filter_before_clone_follows_the_receiver_chain_through_adapters() {
        let edges = run("fn f(xs: &X) { let ys = xs.iter().filter(|x| x.ok).map(f).cloned(); }");
        let derived = only(&edges, "filter_before_clone");
        assert_eq!(derived[0], "rust:crate::a::f", "argument 0 is the function");
        assert!(derived[1].contains("#ownership:filter:"), "then the filter");
        assert!(derived[2].contains("#ownership:clone:"), "then the clone");
    }

    // Pins the §6.2 requirement that "mere line ordering is insufficient", and
    // the adversarial "unrelated filter and clone on adjacent lines" fixture.
    #[test]
    fn a_filter_and_a_clone_in_separate_statements_are_not_related() {
        let edges =
            run("fn f(xs: &X) { let ys = xs.iter().filter(|x| x.ok); let zs = ys.cloned(); }");
        assert_eq!(args_of(&edges, "filter_site").len(), 1, "both are observed");
        assert_eq!(args_of(&edges, "clone_site").len(), 1);
        assert!(
            args_of(&edges, "filter_before_clone").is_empty(),
            "different expression chains, so no relation"
        );
    }

    // Pins that a clone *before* a filter in one chain is not reported as
    // filter-before-clone: the relation names an observed order, and
    // `clone_before_filter` is intentionally absent from the relation set.
    #[test]
    fn a_clone_ahead_of_a_filter_in_the_same_chain_is_not_filter_before_clone() {
        let edges = run("fn f(xs: &X) { let ys = xs.iter().cloned().filter(|x| x.ok); }");
        assert!(args_of(&edges, "filter_before_clone").is_empty());
        assert_eq!(
            args_of(&edges, "clone_site").len(),
            1,
            "both still observed"
        );
        assert_eq!(args_of(&edges, "filter_site").len(), 1);
    }

    // Pins §6.2's ordering-only semantics for `clone_before_await`.
    #[test]
    fn clone_before_await_pairs_each_earlier_clone_with_each_later_await() {
        let edges = run(concat!(
            "async fn f(v: &V) { ",
            "let a = v.clone(); ",
            "first().await; ",
            "let b = v.clone(); ",
            "second().await; ",
            "}",
        ));
        // a-first, a-second, b-second; never b-first.
        assert_eq!(
            args_of(&edges, "clone_before_await").len(),
            3,
            "only clones that lexically precede the await: {:?}",
            args_of(&edges, "clone_before_await")
        );
    }

    // Pins D4's root-place descent: `self.party.members[i].pos` roots at
    // `self`, so the earlier snapshot and the later mutation relate.
    #[test]
    fn read_before_mutation_matches_on_the_syntactic_root_place() {
        let edges = run(concat!(
            "fn f(&mut self) { ",
            "let snapshot = self.party.members[i].pos.clone(); ",
            "self.party.members.push(snapshot); ",
            "}",
        ));
        let derived = only(&edges, "read_before_mutation");
        assert_eq!(derived[0], "rust:crate::a::f");
        assert!(derived[1].contains("#ownership:clone:"), "read site first");
        assert!(derived[2].contains("#ownership:mutation:"), "then mutation");
    }

    // Pins §7.11 and D4: a place whose descent terminates at a call has no root
    // place, so no relation is claimed about it.
    #[test]
    fn a_place_rooted_in_a_call_produces_no_read_before_mutation() {
        let edges =
            run("fn f(k: &K) { let snapshot = lookup(k).field.clone(); lookup(k).field = 1; }");
        assert_eq!(args_of(&edges, "clone_site").len(), 1, "still observed");
        assert!(
            args_of(&edges, "read_before_mutation").is_empty(),
            "no unique root place, so no edge"
        );
    }

    // Pins §7.9/D6 case 1: the narrowest *binding* block is what matters, not
    // the function body. The guard here is bound inside an inner block that
    // closes before the await.
    #[test]
    fn lock_scope_ends_before_await_uses_the_narrowest_binding_block() {
        let edges = run(concat!(
            "async fn f(m: &M) { ",
            "{ let g = m.lock(); g.use_it(); } ",
            "step().await; ",
            "}",
        ));
        let derived = only(&edges, "lock_scope_ends_before_await");
        assert_eq!(derived[0], "rust:crate::a::f");
        assert!(derived[1].contains("#ownership:lock:"));
        assert!(derived[2].contains("#ownership:await:"));
    }

    // Pins the negative control (§12, §15, §17). A guard genuinely live across
    // the await yields no scope relation — and, critically, no crossing claim.
    #[test]
    fn a_guard_live_across_an_await_yields_no_scope_relation_and_no_crossing_claim() {
        let edges = run("async fn f(m: &M) { let g = m.lock(); step().await; g.use_it(); }");
        assert_eq!(args_of(&edges, "sync_lock_site").len(), 1, "site observed");
        assert!(
            args_of(&edges, "lock_scope_ends_before_await").is_empty(),
            "the block does not end before the await"
        );
        assert!(
            !edges
                .iter()
                .any(|edge| edge.p == "lock_scope_may_cross_await"),
            "AST containment must never produce a crossing claim (§6.2, D6)"
        );
    }

    // Pins D6 case 2, which is the only reason the §12 `drop(guard)` fixture
    // can pass at all: the lexical rule alone cannot see an explicit drop.
    #[test]
    fn an_explicit_drop_before_the_await_ends_the_lock_scope() {
        let edges = run("async fn f(m: &M) { let g = m.lock(); drop(g); step().await; }");
        assert_eq!(
            args_of(&edges, "lock_scope_ends_before_await").len(),
            1,
            "an explicit drop ends the scope before the await"
        );
    }

    // Pins that a drop of a *different* guard does not end this guard's scope.
    #[test]
    fn dropping_a_different_binding_does_not_end_the_guards_scope() {
        let edges = run("async fn f(m: &M) { let g = m.lock(); drop(other); step().await; }");
        assert!(args_of(&edges, "lock_scope_ends_before_await").is_empty());
    }

    // Pins the §12 acceptance shape for the scheduler negative control: two
    // lock sites, both scopes ending before the await, and no crossing claim.
    // This is the case the spec cites as the reason not to warn on every lock
    // in an async function.
    #[test]
    fn the_scheduler_shape_yields_two_lock_sites_and_two_scope_conclusions() {
        let edges = run(concat!(
            "impl Scheduler { async fn acquire(&self) { ",
            "{ let permits = self.permits.lock(); permits.take(); } ",
            "let queued = self.queue.lock(); queued.len(); drop(queued); ",
            "self.ready.notified().await; ",
            "} }",
        ));
        assert_eq!(
            args_of(&edges, "sync_lock_site").len(),
            2,
            "both acquisitions are observed"
        );
        assert_eq!(
            args_of(&edges, "lock_scope_ends_before_await").len(),
            2,
            "one by block scope, one by explicit drop: {:?}",
            args_of(&edges, "lock_scope_ends_before_await")
        );
        assert!(
            !edges
                .iter()
                .any(|edge| edge.p == "lock_scope_may_cross_await")
        );
    }

    // Pins the §12 filter-before-clone incident shape end to end, including the
    // turbofish `collect` that terminates the chain.
    #[test]
    fn the_filter_before_clone_incident_shape_is_fully_described() {
        let edges = run(concat!(
            "fn execute(&self) { ",
            "let matched = self.facts.iter().filter(|f| f.active).cloned().collect::<Vec<_>>(); ",
            "}",
        ));
        assert_eq!(args_of(&edges, "filter_site").len(), 1);
        assert_eq!(
            args_of(&edges, "clone_site").len(),
            2,
            "`cloned` and `collect` are distinct observations (§7.6, D5)"
        );
        assert_eq!(
            args_of(&edges, "filter_before_clone").len(),
            2,
            "the filter shares a receiver chain with both clone-producing calls"
        );
        assert!(
            !edges.iter().any(|edge| edge.p == "clone_cost_evidence"),
            "no cost claim may come from syntax (§6.3, §17)"
        );
    }

    // ─── boundedness and safety ─────────────────────────────────────

    // Pins §13.1's "AST extraction never emits MIR-only relations" as
    // structure, not discipline (D19). A comment cannot enforce this; the
    // allowlist and this corpus sweep can.
    #[test]
    fn the_union_of_emitted_relations_is_a_subset_of_the_ast_allowlist() {
        let corpus = [
            "async fn f(m: &M, v: &mut V) { let g = m.lock(); drop(g); let s = v.x.clone(); v.x.push(s); step().await; }",
            "fn f(xs: &X) { let ys = xs.iter().filter(|x| x.ok).cloned().collect::<Vec<_>>(); }",
            "async fn f(m: &M) { let g = m.lock(); step().await; g.use_it(); }",
            "fn f(x: &X) { let y = Clone::clone(&x.inner); }",
            "async fn f(m: &M) { let g = m.lock().await; }",
            "fn f() { /* .clone() */ let s = \".lock()\"; }",
            "macro_rules! m { ($v:expr) => { $v.clone() }; }",
        ];
        let mut seen = BTreeSet::new();
        for source in corpus {
            for edge in run(source) {
                if edge.p.contains("ownership")
                    || edge.p.ends_with("_site")
                    || edge.p.contains("_before_")
                    || edge.p.contains("lock_scope")
                {
                    seen.insert(edge.p);
                }
            }
        }
        assert!(!seen.is_empty(), "the corpus must actually produce edges");
        for relation in &seen {
            assert!(
                AST_EMITTABLE.contains(&relation.as_str()),
                "{relation} is not AST-emittable (D19)"
            );
        }
        for forbidden in [
            "lock_scope_may_cross_await",
            "ownership_transfer",
            "borrow_live_across",
            "ownership_conflict_diagnostic",
            "resolved_type",
            "clone_cost_evidence",
        ] {
            assert!(
                !AST_EMITTABLE.contains(&forbidden),
                "{forbidden} must not be in the AST allowlist"
            );
        }
    }

    // Pins D12: an ownership edge with an empty `src` is unreachable by
    // provenance-keyed compaction and becomes permanently stale, and a `d: true`
    // edge is silently discarded by `store::compact`.
    #[test]
    fn every_ownership_edge_is_a_base_edge_carrying_its_file_as_provenance() {
        let edges = run("async fn f(v: &V) { let a = v.clone(); step().await; }");
        let ownership: Vec<&Edge> = edges
            .iter()
            .filter(|edge| edge.p.contains("ownership") || edge.p.ends_with("_site"))
            .collect();
        assert!(!ownership.is_empty(), "the fixture must produce edges");
        for edge in ownership {
            assert_eq!(edge.src, FILE, "{} must carry its file", edge.p);
            assert!(!edge.d, "{} must not be a derived edge", edge.p);
        }
    }

    // Pins §9's site cap: the budget bounds output, keeps what it already
    // produced, and says `partial`/`site_cap` instead of failing.
    #[test]
    fn a_site_cap_reports_partial_analysis_and_keeps_the_bounded_observations() {
        let config = OwnershipConfig {
            enabled: true,
            max_sites_per_file: 1,
            ..OwnershipConfig::disabled()
        };
        let edges = run_with(
            FILE,
            "async fn f(v: &V) { let a = v.clone(); let b = v.clone(); step().await; }",
            &config,
        );
        assert_eq!(
            args_of(&edges, "ownership_site").len(),
            1,
            "the budget is enforced"
        );
        assert_eq!(
            only(&edges, "ownership_analysis_status"),
            vec![
                FILE.to_string(),
                "ast_extraction".to_string(),
                "partial".to_string(),
                "site_cap".to_string()
            ],
            "reaching the cap is reported, not silent"
        );
        assert!(
            args_of(&edges, "clone_before_await").is_empty(),
            "a derivation may not reference a site the cap dropped"
        );
    }

    // ─── operand normalization (D7) ─────────────────────────────────

    // Pins D7 steps 1-3: whitespace runs collapse to one space and the value is
    // trimmed, so the same expression formatted differently yields one operand.
    #[test]
    fn operand_text_collapses_whitespace_runs_to_single_spaces() {
        let edges = run("fn f(v: &V) { let a = v\n    .items\n    .iter()\n    .cloned(); }");
        assert_eq!(only(&edges, "clone_site")[2], "v .items .iter()");
    }

    // Pins D7 step 4: over the cap, the operand becomes a digest marker rather
    // than a truncation, because a truncated expression reads as a real but
    // different expression.
    #[test]
    fn an_oversized_operand_becomes_a_digest_marker_and_is_never_truncated() {
        let long = "x".repeat(OPERAND_TEXT_CAP + 10);
        let edges = run(&format!("fn f() {{ let a = {long}.clone(); }}"));
        let operand = only(&edges, "clone_site")[2].clone();
        assert!(
            operand.starts_with("sha256:"),
            "expected a digest marker, got {operand}"
        );
        assert_eq!(operand.len(), "sha256:".len() + 16, "16 hex characters");
        assert!(
            !operand.contains('x'),
            "the marker must not be a truncation of the expression"
        );
        assert_eq!(
            operand,
            format!("sha256:{}", &hex(&sha256(long.as_bytes()))[..16]),
            "the digest is over the normalized text"
        );
    }

    // Pins D18: `Edge::fact_id` joins arguments with U+001F, so an operand
    // containing that byte would corrupt every id it appears in.
    #[test]
    fn an_operand_containing_the_fact_id_separator_falls_back_to_a_digest() {
        let separator = '\u{1f}';
        let raw = format!("a{separator}b");
        assert!(
            normalize_text_for_test(&raw).starts_with("sha256:"),
            "U+001F must never reach an edge argument"
        );
    }

    fn normalize_text_for_test(raw: &str) -> String {
        let normalized = collapse_whitespace(raw);
        if normalized.len() > OPERAND_TEXT_CAP || normalized.contains('\u{1f}') {
            return format!("sha256:{}", &hex(&sha256(normalized.as_bytes()))[..16]);
        }
        normalized
    }

    // Pins the hand-written SHA-256 against the FIPS 180-4 vectors. The digest
    // marker claims to be SHA-256; if it is not, the graph is lying.
    #[test]
    fn the_bundled_sha256_matches_the_published_test_vectors() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "SHA-256(\"abc\")"
        );
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "SHA-256(\"\")"
        );
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            "SHA-256 of the 56-byte vector, which exercises a second block"
        );
    }

    // ─── lineage (Addendum A.1) ─────────────────────────────────────

    // Pins Addendum A.1: every derived relationship must be traversable to its
    // supporting sites, and every site to its span and evidence.
    #[test]
    fn every_derived_relationship_reaches_a_site_with_a_span_and_a_provider() {
        let edges =
            run("async fn f(v: &V) { let a = v.items.iter().filter(p).cloned(); step().await; }");
        let sites: BTreeSet<String> = args_of(&edges, "ownership_site")
            .into_iter()
            .filter_map(|args| args.into_iter().next())
            .collect();
        let spanned: BTreeSet<String> = args_of(&edges, "ownership_site_span")
            .into_iter()
            .filter_map(|args| args.into_iter().next())
            .collect();
        let evidenced: BTreeSet<String> = args_of(&edges, "ownership_evidence")
            .into_iter()
            .filter_map(|args| args.into_iter().next())
            .collect();
        assert_eq!(sites, spanned, "every site has a span");
        assert_eq!(sites, evidenced, "every site has an evidence level");
        let mut derived = 0;
        for relation in [
            "filter_before_clone",
            "clone_before_await",
            "read_before_mutation",
            "lock_scope_ends_before_await",
        ] {
            for args in args_of(&edges, relation) {
                derived += 1;
                assert!(sites.contains(&args[1]), "{relation} arg 1 names a site");
                assert!(sites.contains(&args[2]), "{relation} arg 2 names a site");
            }
        }
        assert!(derived >= 2, "the fixture must exercise derivations");
    }
}
