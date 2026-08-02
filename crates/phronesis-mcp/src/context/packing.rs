use std::collections::BTreeSet;

use serde::Serialize;

use super::config::ContextConfig;

pub const OMISSION_FOOTER: &str = "Context items omitted; run `phr-mcp context inspect`.";

/// Identifier for one packable context unit — `kernel:0`,
/// `activity:<ts>:<rule>:<n>`, `nudge:<capsule>`, `state:subject`, and so on.
///
/// Defined in the engine crate alongside [`phr::RuleId`] so the two share one
/// generated shape; see `phronesis::ids`. Serialization is transparent, so
/// `context inspect --json` and the `kind:"context"` observations keep their
/// bare-string shape on the wire.
pub use phr::StableId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Activity,
    /// The always-on core, injected at every event.
    Kernel,
    /// The session-level project document. Rendered at a charter event only —
    /// it is orientation material, and making it compete for per-turn budget
    /// is what starved the kernel before.
    Charter,
    Nudge,
    State,
    Rule,
    Orientation,
    Footer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    None,
    Warning,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextItem {
    pub kind: ItemKind,
    pub stable_id: StableId,
    pub priority: i32,
    pub severity: Severity,
    pub body: String,
    pub max_bytes: Option<usize>,
}

impl ContextItem {
    pub fn new(kind: ItemKind, id: impl Into<StableId>, body: impl Into<String>) -> Self {
        Self {
            kind,
            stable_id: id.into(),
            priority: 0,
            severity: Severity::None,
            body: body.into(),
            max_bytes: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OmissionReason {
    KindCeiling,
    ByteCapacity,
    TokenCapacity,
    DisplacedByNudge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OmittedItem {
    pub kind: ItemKind,
    pub stable_id: StableId,
    pub reason: OmissionReason,
}

/// The result of one packing pass.
///
/// Deliberately carries no `raw_truncation` flag: whether the last-resort
/// guard would fire is a question about the body against the configured hard
/// limit, and `RenderResult::raw_truncation` answers it in one place rather
/// than letting two sources of truth drift.
#[derive(Debug, Clone, Default)]
pub struct PackedContext {
    pub body: String,
    /// Stable ids of the admitted items, in admission order. Keeping the type
    /// tag here is what lets [`super::render`] decide a candidate's verdict by
    /// comparing two `StableId`s directly, rather than comparing a tagged id
    /// against a bare string.
    pub selected: Vec<StableId>,
    pub omitted: Vec<OmittedItem>,
}

impl PackedContext {
    pub fn bytes(&self) -> usize {
        self.body.len()
    }

    pub fn estimated_tokens(&self) -> usize {
        estimate_tokens(self.body.len())
    }
}

pub fn estimate_tokens(bytes: usize) -> usize {
    bytes.div_ceil(3)
}

fn heading(kind: ItemKind) -> Option<&'static str> {
    match kind {
        ItemKind::Activity => Some("## Recent phronesis activity"),
        ItemKind::Kernel => Some("## Durable directives"),
        // The charter carries the project's own headings, so it gets a rule
        // rather than a competing `##` of its own.
        ItemKind::Charter => Some("---"),
        ItemKind::Nudge => Some("## Situational guidance"),
        ItemKind::State => Some("## Current phronesis state"),
        ItemKind::Rule => Some("## Active phronesis rules"),
        ItemKind::Orientation | ItemKind::Footer => None,
    }
}

/// Is this kind rendered as a standalone Markdown block (blank line before it)
/// rather than as a list line?
fn paragraph_shaped(kind: ItemKind) -> bool {
    matches!(
        kind,
        ItemKind::Kernel
            | ItemKind::Charter
            | ItemKind::Nudge
            | ItemKind::Orientation
            | ItemKind::Footer
    )
}

#[derive(Debug)]
struct Packer<'a> {
    config: &'a ContextConfig,
    out: PackedContext,
    kinds: BTreeSet<ItemKind>,
    kind_bytes: std::collections::BTreeMap<ItemKind, usize>,
}

impl<'a> Packer<'a> {
    fn new(config: &'a ContextConfig) -> Self {
        Self {
            config,
            out: PackedContext::default(),
            kinds: BTreeSet::new(),
            kind_bytes: std::collections::BTreeMap::new(),
        }
    }

    /// Exactly the bytes appending `item` would add, headings and separators
    /// included. Selection measures this, never the bare body, so no later
    /// formatting step can grow an already-admitted item.
    fn rendered_increment(&self, item: &ContextItem) -> String {
        let mut out = String::new();
        if !self.out.body.is_empty() {
            out.push('\n');
            // Prose items are Markdown paragraphs: without the blank line two
            // kernel paragraphs would reflow into one. Bullet-shaped items
            // (activity, rules, state) are consecutive list lines and must
            // not have one.
            if paragraph_shaped(item.kind) {
                out.push('\n');
            }
        }
        if !self.kinds.contains(&item.kind)
            && let Some(h) = heading(item.kind)
        {
            out.push_str(h);
            out.push('\n');
        }
        out.push_str(&item.body);
        out
    }

    fn reason(&self, item: &ContextItem, kind_ceiling: Option<usize>) -> Option<OmissionReason> {
        let increment = self.rendered_increment(item);
        let used_kind = self.kind_bytes.get(&item.kind).copied().unwrap_or(0);
        let ceiling = item.max_bytes.or(kind_ceiling);
        if ceiling.is_some_and(|max| used_kind + increment.len() > max) {
            return Some(OmissionReason::KindCeiling);
        }
        if self.out.body.len() + increment.len() > self.config.hard_max_bytes {
            return Some(OmissionReason::ByteCapacity);
        }
        if self
            .config
            .estimated_max_tokens
            .is_some_and(|max| estimate_tokens(self.out.body.len() + increment.len()) > max)
        {
            return Some(OmissionReason::TokenCapacity);
        }
        None
    }

    fn commit(&mut self, item: &ContextItem, increment: &str) {
        *self.kind_bytes.entry(item.kind).or_default() += increment.len();
        self.kinds.insert(item.kind);
        self.out.body.push_str(increment);
        self.out.selected.push(item.stable_id.clone());
    }

    fn try_pack(&mut self, item: &ContextItem, kind_ceiling: Option<usize>) -> bool {
        if let Some(reason) = self.reason(item, kind_ceiling) {
            self.out.omitted.push(OmittedItem {
                kind: item.kind,
                stable_id: item.stable_id.clone(),
                reason,
            });
            return false;
        }
        let increment = self.rendered_increment(item);
        self.commit(item, &increment);
        true
    }

    /// Bytes admitted so far for one kind, including the kind's heading and
    /// the separators that were charged to its items.
    fn bytes_for(&self, kind: ItemKind) -> usize {
        self.kind_bytes.get(&kind).copied().unwrap_or(0)
    }

    /// Would a body of `bytes` still satisfy both shared budgets?
    fn fits_shared(&self, bytes: usize) -> bool {
        bytes <= self.config.hard_max_bytes
            && self
                .config
                .estimated_max_tokens
                .is_none_or(|max| estimate_tokens(bytes) <= max)
    }

    /// Reconsider one activity item that overflowed its reserve, against what
    /// is left of the shared capacity. An item that would have fit had the
    /// admitted nudges not consumed shared capacity is attributed to them —
    /// proven by re-checking the budgets with the nudge bytes given back, not
    /// inferred from the mere presence of a nudge.
    fn reconsider_overflow(&mut self, item: &ContextItem) {
        let increment = self.rendered_increment(item);
        match self.reason(item, None) {
            None => self.commit(item, &increment),
            Some(reason) => {
                let displaced = {
                    let nudge_bytes = self.bytes_for(ItemKind::Nudge);
                    nudge_bytes > 0
                        && matches!(
                            reason,
                            OmissionReason::ByteCapacity | OmissionReason::TokenCapacity
                        )
                        && self.fits_shared(
                            (self.out.body.len() + increment.len()).saturating_sub(nudge_bytes),
                        )
                };
                self.out.omitted.push(OmittedItem {
                    kind: item.kind,
                    stable_id: item.stable_id.clone(),
                    reason: if displaced {
                        OmissionReason::DisplacedByNudge
                    } else {
                        reason
                    },
                });
            }
        }
    }

    fn finish(mut self) -> PackedContext {
        if !self.out.omitted.is_empty() {
            let footer = ContextItem::new(ItemKind::Footer, "omission-footer", OMISSION_FOOTER);
            if !self.try_pack(&footer, None) {
                self.out.omitted.pop();
            }
        }
        // No assertion here. A violated invariant is a renderer bug that the
        // caller's last-resort envelope guard must absorb and report as
        // `raw_truncation`; it must never panic a hook and fail the turn.
        self.out
    }
}

/// Stateless interaction packing. `activity` must already be in the normative
/// order; kernel remains in file order. Capsules are sorted here.
pub fn pack_interaction(
    config: &ContextConfig,
    activity: &[ContextItem],
    kernel: &[ContextItem],
    nudges: &[ContextItem],
) -> PackedContext {
    let mut p = Packer::new(config);
    let mut overflow = Vec::new();
    let reserve = config.interaction.activity_reserve_bytes;
    for item in activity {
        if !p.try_pack(item, Some(reserve)) {
            p.out.omitted.pop();
            overflow.push(item);
        }
    }
    for item in kernel {
        p.try_pack(item, Some(config.interaction.kernel_max_bytes));
    }
    for item in nudge_order(nudges) {
        p.try_pack(item, Some(config.interaction.nudges_max_bytes));
    }
    // Step 6: reconsider activity that overflowed its reserve.
    for item in overflow {
        p.reconsider_overflow(item);
    }
    p.finish()
}

/// The order nudges compete in: highest priority first, then smallest body,
/// then stable id — a total order, so identical inputs pack identically.
fn nudge_order(nudges: &[ContextItem]) -> Vec<&ContextItem> {
    let mut sorted: Vec<&ContextItem> = nudges.iter().collect();
    sorted.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.body.len().cmp(&b.body.len()))
            .then_with(|| a.stable_id.cmp(&b.stable_id))
    });
    sorted
}

/// The item groups a session pack draws from, named so the packing order is
/// not carried by argument position alone.
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionSections<'a> {
    pub state: &'a [ContextItem],
    pub kernel: &'a [ContextItem],
    pub charter: &'a [ContextItem],
    pub rules: &'a [ContextItem],
    pub orientation: Option<&'a ContextItem>,
}

/// Stateless session packing.
///
/// Order is state, kernel, charter, rules, orientation, then reconsidered
/// state overflow. The charter sits ahead of the rule listing because it is
/// the project's own guidance; the rule summary is recoverable over MCP, the
/// charter prose is not.
pub fn pack_session(config: &ContextConfig, sections: SessionSections<'_>) -> PackedContext {
    let mut p = Packer::new(config);
    let mut overflow = Vec::new();
    for item in sections.state {
        if !p.try_pack(item, Some(config.session.state_reserve_bytes)) {
            p.out.omitted.pop();
            overflow.push(item);
        }
    }
    for item in sections.kernel {
        p.try_pack(item, Some(config.session.kernel_max_bytes));
    }
    for item in sections.charter {
        p.try_pack(item, Some(config.session.charter_max_bytes));
    }
    for item in sections.rules {
        p.try_pack(item, Some(config.session.rules_max_bytes));
    }
    if let Some(item) = sections.orientation {
        p.try_pack(item, None);
    }
    for item in overflow {
        p.try_pack(item, None);
    }
    p.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::config::{InteractionConfig, SessionConfig};

    /// A config with generous byte room and no token ceiling, so a test that
    /// is about one limit is not silently decided by another.
    fn byte_only(hard: usize) -> ContextConfig {
        ContextConfig {
            hard_max_bytes: hard,
            estimated_max_tokens: None,
            interaction: InteractionConfig {
                kernel_max_bytes: hard,
                activity_reserve_bytes: hard,
                nudges_max_bytes: hard,
            },
            session: SessionConfig {
                kernel_max_bytes: hard,
                state_reserve_bytes: hard,
                charter_max_bytes: hard,
                rules_max_bytes: hard,
            },
            ..ContextConfig::default()
        }
    }

    fn item(kind: ItemKind, id: &str, body: &str) -> ContextItem {
        ContextItem::new(kind, id, body)
    }

    fn reason_for<'a>(out: &'a PackedContext, id: &str) -> Option<&'a OmissionReason> {
        out.omitted
            .iter()
            .find(|o| o.stable_id == id)
            .map(|o| &o.reason)
    }

    #[test]
    fn estimate_rounds_up() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(1), 1);
        assert_eq!(estimate_tokens(4), 2);
    }

    #[test]
    fn body_never_exceeds_hard_limit() {
        let config = byte_only(100);
        let items = (0..20)
            .map(|i| item(ItemKind::Activity, &format!("a{i}"), &"x".repeat(20)))
            .collect::<Vec<_>>();
        let out = pack_interaction(&config, &items, &[], &[]);
        assert!(out.bytes() <= 100, "got {} bytes", out.bytes());
        assert!(!out.omitted.is_empty());
    }

    #[test]
    fn nudge_sort_is_priority_then_size_then_id() {
        let config = ContextConfig {
            estimated_max_tokens: None,
            ..ContextConfig::default()
        };
        let mut low = item(ItemKind::Nudge, "low", "low");
        low.priority = 1;
        let mut z = item(ItemKind::Nudge, "z", "zz");
        z.priority = 9;
        let mut a = item(ItemKind::Nudge, "a", "aa");
        a.priority = 9;
        let out = pack_interaction(&config, &[], &[], &[low, z, a]);
        assert_eq!(&out.selected[..3], ["a", "z", "low"]);
    }

    #[test]
    fn measured_size_includes_heading_and_separators() {
        let config = byte_only(4096);
        let out = pack_interaction(&config, &[item(ItemKind::Activity, "a", "- one")], &[], &[]);
        // Heading + newline + body — nothing added after selection.
        assert_eq!(out.body, "## Recent phronesis activity\n- one");
        assert_eq!(out.bytes(), out.body.len());
    }

    #[test]
    fn kernel_never_borrows_beyond_its_ceiling() {
        // Plenty of shared room, but the kernel ceiling admits only the first
        // paragraph. The second must be dropped for `kind_ceiling`, not for
        // lack of shared capacity.
        let heading = "## Durable directives\n".len();
        let config = ContextConfig {
            hard_max_bytes: 4096,
            estimated_max_tokens: None,
            interaction: InteractionConfig {
                kernel_max_bytes: heading + 10,
                activity_reserve_bytes: 1024,
                nudges_max_bytes: 1024,
            },
            ..ContextConfig::default()
        };
        let kernel = vec![
            item(ItemKind::Kernel, "kernel:0", "0123456789"),
            item(ItemKind::Kernel, "kernel:1", "also short"),
        ];
        let out = pack_interaction(&config, &[], &kernel, &[]);
        assert!(out.selected.contains(&StableId::new("kernel:0")));
        assert!(!out.selected.contains(&StableId::new("kernel:1")));
        assert_eq!(
            reason_for(&out, "kernel:1"),
            Some(&OmissionReason::KindCeiling)
        );
    }

    #[test]
    fn activity_gets_its_reserve_before_kernel_and_nudges() {
        // Shared capacity fits activity plus exactly one of the other two.
        // Activity must win regardless of ordering in the input.
        let config = ContextConfig {
            hard_max_bytes: 120,
            estimated_max_tokens: None,
            interaction: InteractionConfig {
                kernel_max_bytes: 120,
                activity_reserve_bytes: 60,
                nudges_max_bytes: 120,
            },
            ..ContextConfig::default()
        };
        let activity = vec![item(ItemKind::Activity, "a", "- BLOCKED now")];
        let kernel = vec![item(ItemKind::Kernel, "kernel:0", &"k".repeat(300))];
        let nudges = vec![item(ItemKind::Nudge, "n", &"n".repeat(300))];
        let out = pack_interaction(&config, &activity, &kernel, &nudges);
        assert!(out.selected.contains(&StableId::new("a")));
        assert!(out.bytes() <= 120);
    }

    #[test]
    fn unused_activity_reserve_returns_to_shared_capacity() {
        // One tiny activity item leaves most of the reserve unspent; the
        // kernel must be able to use it.
        let config = ContextConfig {
            hard_max_bytes: 300,
            estimated_max_tokens: None,
            interaction: InteractionConfig {
                kernel_max_bytes: 300,
                activity_reserve_bytes: 200,
                nudges_max_bytes: 0,
            },
            ..ContextConfig::default()
        };
        let activity = vec![item(ItemKind::Activity, "a", "- x")];
        let kernel = vec![item(ItemKind::Kernel, "kernel:0", &"k".repeat(200))];
        let out = pack_interaction(&config, &activity, &kernel, &[]);
        assert!(
            out.selected.contains(&StableId::new("kernel:0")),
            "kernel should reach into the unspent reserve; selected={:?}",
            out.selected
        );
    }

    #[test]
    fn token_ceiling_can_exclude_an_item_that_fits_by_bytes() {
        let config = ContextConfig {
            hard_max_bytes: 4096,
            // 10 tokens ≈ 30 bytes.
            estimated_max_tokens: Some(10),
            interaction: InteractionConfig {
                kernel_max_bytes: 4096,
                activity_reserve_bytes: 4096,
                nudges_max_bytes: 4096,
            },
            ..ContextConfig::default()
        };
        let activity = vec![
            item(ItemKind::Activity, "a", "- short"),
            item(ItemKind::Activity, "b", &"y".repeat(200)),
        ];
        let out = pack_interaction(&config, &activity, &[], &[]);
        assert_eq!(
            reason_for(&out, "b"),
            Some(&OmissionReason::TokenCapacity),
            "byte-safe but token-excluded items must say so"
        );
        assert!(estimate_tokens(out.bytes()) <= 10);
    }

    #[test]
    fn overflow_activity_displaced_by_a_nudge_is_attributed_to_it() {
        // Sized so the second activity item fits only if the nudge is not
        // there. That is what makes the attribution a proof rather than a
        // guess.
        let config = ContextConfig {
            hard_max_bytes: 90,
            estimated_max_tokens: None,
            interaction: InteractionConfig {
                kernel_max_bytes: 0,
                activity_reserve_bytes: 34,
                nudges_max_bytes: 60,
            },
            ..ContextConfig::default()
        };
        let activity = vec![
            item(ItemKind::Activity, "a1", "- one"),
            item(ItemKind::Activity, "a2", "- two"),
        ];
        let nudges = vec![item(ItemKind::Nudge, "n", &"n".repeat(30))];
        let out = pack_interaction(&config, &activity, &[], &nudges);
        assert!(out.selected.contains(&StableId::new("n")), "nudge admitted");
        assert_eq!(
            reason_for(&out, "a2"),
            Some(&OmissionReason::DisplacedByNudge)
        );
    }

    #[test]
    fn overflow_activity_not_displaced_when_no_nudge_was_admitted() {
        let config = ContextConfig {
            hard_max_bytes: 40,
            estimated_max_tokens: None,
            interaction: InteractionConfig {
                kernel_max_bytes: 0,
                activity_reserve_bytes: 34,
                nudges_max_bytes: 0,
            },
            ..ContextConfig::default()
        };
        let activity = vec![
            item(ItemKind::Activity, "a1", "- one"),
            item(ItemKind::Activity, "a2", &"z".repeat(50)),
        ];
        let out = pack_interaction(&config, &activity, &[], &[]);
        assert_eq!(reason_for(&out, "a2"), Some(&OmissionReason::ByteCapacity));
    }

    #[test]
    fn footer_appears_once_when_anything_was_omitted() {
        let config = byte_only(200);
        let activity = vec![
            item(ItemKind::Activity, "a1", "- one"),
            item(ItemKind::Activity, "a2", &"z".repeat(500)),
        ];
        let out = pack_interaction(&config, &activity, &[], &[]);
        assert!(out.body.contains(OMISSION_FOOTER));
        assert_eq!(out.body.matches(OMISSION_FOOTER).count(), 1);
    }

    #[test]
    fn no_footer_when_nothing_was_omitted() {
        let config = byte_only(4096);
        let out = pack_interaction(&config, &[item(ItemKind::Activity, "a", "- one")], &[], &[]);
        assert!(!out.body.contains(OMISSION_FOOTER));
    }

    #[test]
    fn footer_that_does_not_fit_is_dropped_without_breaking_the_limit() {
        // Room for the first item and nothing else — not even the footer.
        let config = byte_only(40);
        let activity = vec![
            item(ItemKind::Activity, "a1", "- one"),
            item(ItemKind::Activity, "a2", &"z".repeat(500)),
        ];
        let out = pack_interaction(&config, &activity, &[], &[]);
        assert!(!out.body.contains(OMISSION_FOOTER));
        assert!(out.bytes() <= 40);
        assert!(
            !out.omitted.is_empty(),
            "the real omission must survive the footer's failed admission"
        );
    }

    #[test]
    fn item_exactly_at_the_boundary_is_admitted() {
        let body = "- exact";
        let increment = format!("## Recent phronesis activity\n{body}");
        let config = byte_only(increment.len());
        let out = pack_interaction(&config, &[item(ItemKind::Activity, "a", body)], &[], &[]);
        assert_eq!(out.selected, ["a"]);
        assert_eq!(out.bytes(), increment.len());
    }

    #[test]
    fn one_byte_under_the_boundary_is_omitted() {
        let body = "- exact";
        let increment = format!("## Recent phronesis activity\n{body}");
        let config = byte_only(increment.len() - 1);
        let out = pack_interaction(&config, &[item(ItemKind::Activity, "a", body)], &[], &[]);
        assert!(!out.selected.contains(&StableId::new("a")));
        assert_eq!(reason_for(&out, "a"), Some(&OmissionReason::ByteCapacity));
    }

    #[test]
    fn a_multibyte_item_is_never_split() {
        // A budget that lands mid-character must drop the whole item, not cut
        // it: complete-item packing is what keeps the body valid UTF-8
        // without involving the truncator.
        let body = "- é".repeat(20);
        let config = byte_only(body.len() - 1);
        let out = pack_interaction(&config, &[item(ItemKind::Activity, "a", &body)], &[], &[]);
        assert!(!out.selected.contains(&StableId::new("a")));
        assert!(!out.body.contains('é'), "no fragment of the item leaked");
        assert!(std::str::from_utf8(out.body.as_bytes()).is_ok());
    }

    #[test]
    fn kernel_paragraphs_keep_a_blank_line_between_them() {
        let config = byte_only(4096);
        let kernel = vec![
            item(ItemKind::Kernel, "kernel:0", "First paragraph."),
            item(ItemKind::Kernel, "kernel:1", "Second paragraph."),
        ];
        let out = pack_interaction(&config, &[], &kernel, &[]);
        assert!(
            out.body.contains("First paragraph.\n\nSecond paragraph."),
            "paragraphs must not reflow into one; got:\n{}",
            out.body
        );
    }

    #[test]
    fn bullet_items_do_not_gain_a_blank_line() {
        let config = byte_only(4096);
        let activity = vec![
            item(ItemKind::Activity, "a1", "- one"),
            item(ItemKind::Activity, "a2", "- two"),
        ];
        let out = pack_interaction(&config, &activity, &[], &[]);
        assert!(out.body.contains("- one\n- two"), "got:\n{}", out.body);
    }

    #[test]
    fn identical_inputs_produce_byte_identical_output() {
        let config = ContextConfig::default();
        let activity = vec![
            item(ItemKind::Activity, "a1", "- one"),
            item(ItemKind::Activity, "a2", "- two"),
        ];
        let kernel = vec![item(ItemKind::Kernel, "kernel:0", "Be careful.")];
        let mut n1 = item(ItemKind::Nudge, "n1", "first");
        n1.priority = 5;
        let mut n2 = item(ItemKind::Nudge, "n2", "second");
        n2.priority = 5;
        let nudges = vec![n1, n2];
        let first = pack_interaction(&config, &activity, &kernel, &nudges);
        let second = pack_interaction(&config, &activity, &kernel, &nudges);
        assert_eq!(first.body, second.body);
        assert_eq!(first.selected, second.selected);
    }

    #[test]
    fn a_tiny_budget_still_terminates_and_stays_in_range() {
        // Degenerate budgets must not panic, loop, or emit an oversized body.
        for hard in 0..40usize {
            let config = byte_only(hard);
            let activity = (0..5)
                .map(|i| item(ItemKind::Activity, &format!("a{i}"), "- some activity"))
                .collect::<Vec<_>>();
            let kernel = vec![item(ItemKind::Kernel, "kernel:0", "A kernel paragraph.")];
            let nudges = vec![item(ItemKind::Nudge, "n", "A nudge body.")];
            let out = pack_interaction(&config, &activity, &kernel, &nudges);
            assert!(out.bytes() <= hard, "hard={hard} produced {}", out.bytes());
        }
    }

    #[test]
    fn the_truncator_is_never_needed_for_in_schema_inputs() {
        // Property-ish sweep over the shapes the schema permits: whatever the
        // mix, packing alone must keep the body inside the hard limit, so the
        // last-resort guard stays unreachable.
        for hard in [64usize, 256, 1024, 4096] {
            for token_cap in [None, Some(1usize), Some(50), Some(900)] {
                let config = ContextConfig {
                    hard_max_bytes: hard,
                    estimated_max_tokens: token_cap,
                    interaction: InteractionConfig {
                        kernel_max_bytes: hard / 2,
                        activity_reserve_bytes: hard / 4,
                        nudges_max_bytes: hard / 2,
                    },
                    session: SessionConfig {
                        kernel_max_bytes: hard / 2,
                        state_reserve_bytes: hard / 8,
                        charter_max_bytes: hard / 2,
                        rules_max_bytes: hard,
                    },
                    ..ContextConfig::default()
                };
                let activity = (0..8)
                    .map(|i| {
                        item(
                            ItemKind::Activity,
                            &format!("a{i}"),
                            &"a".repeat(i * 17 + 1),
                        )
                    })
                    .collect::<Vec<_>>();
                let kernel = (0..4)
                    .map(|i| {
                        item(
                            ItemKind::Kernel,
                            &format!("kernel:{i}"),
                            &"ké".repeat(i * 13 + 1),
                        )
                    })
                    .collect::<Vec<_>>();
                let nudges = (0..3)
                    .map(|i| item(ItemKind::Nudge, &format!("n{i}"), &"n".repeat(i * 200 + 1)))
                    .collect::<Vec<_>>();
                let interaction = pack_interaction(&config, &activity, &kernel, &nudges);
                assert!(
                    interaction.bytes() <= hard,
                    "interaction hard={hard} tokens={token_cap:?} -> {}",
                    interaction.bytes()
                );
                if let Some(max) = token_cap {
                    assert!(estimate_tokens(interaction.bytes()) <= max);
                }
                let session = pack_session(
                    &config,
                    SessionSections {
                        state: &activity,
                        kernel: &kernel,
                        rules: &nudges,
                        ..SessionSections::default()
                    },
                );
                assert!(
                    session.bytes() <= hard,
                    "session hard={hard} tokens={token_cap:?} -> {}",
                    session.bytes()
                );
                if let Some(max) = token_cap {
                    assert!(estimate_tokens(session.bytes()) <= max);
                }
            }
        }
    }

    #[test]
    fn session_order_is_state_kernel_rules_then_orientation() {
        let config = byte_only(4096);
        let state = vec![item(ItemKind::State, "state:subject", "- Open subject: x")];
        let kernel = vec![item(ItemKind::Kernel, "kernel:0", "Kernel text.")];
        let rules = vec![item(ItemKind::Rule, "rule:r", "- r — do the thing")];
        let orientation = item(ItemKind::Orientation, "orientation:mcp", "MCP line.");
        let out = pack_session(
            &config,
            SessionSections {
                state: &state,
                kernel: &kernel,
                rules: &rules,
                orientation: Some(&orientation),
                ..SessionSections::default()
            },
        );
        assert_eq!(
            out.selected,
            ["state:subject", "kernel:0", "rule:r", "orientation:mcp"]
        );
    }

    #[test]
    fn session_overflow_state_is_reconsidered_after_the_rest() {
        // The second state line exceeds the reserve but fits the shared
        // capacity, so it must reappear at the end rather than vanish.
        let config = ContextConfig {
            hard_max_bytes: 4096,
            estimated_max_tokens: None,
            session: SessionConfig {
                kernel_max_bytes: 4096,
                state_reserve_bytes: 60,
                charter_max_bytes: 4096,
                rules_max_bytes: 4096,
            },
            ..ContextConfig::default()
        };
        let state = vec![
            item(ItemKind::State, "state:subject", "- Open subject: x"),
            item(
                ItemKind::State,
                "state:graph",
                &format!("- {}", "g".repeat(60)),
            ),
        ];
        let kernel = vec![item(ItemKind::Kernel, "kernel:0", "Kernel text.")];
        let out = pack_session(
            &config,
            SessionSections {
                state: &state,
                kernel: &kernel,
                ..SessionSections::default()
            },
        );
        assert_eq!(out.selected, ["state:subject", "kernel:0", "state:graph"]);
        assert!(out.omitted.is_empty(), "a re-admitted item is not omitted");
    }
}
