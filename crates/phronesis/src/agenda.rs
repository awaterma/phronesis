use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use uuid::Uuid;

use crate::engine_types::Rule;
use crate::variable_binding::Bindings;
use crate::wme::WorkingMemoryElement;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgendaItem {
    pub rule: Rule,
    pub wme_list: Vec<WorkingMemoryElement>,
    pub bindings: Bindings, // Variable bindings from pattern matching
    pub salience: i32,      // Priority value
    pub id: String,
    /// Monotonic insertion sequence — tiebreak so same-salience items
    /// fire in insertion (FIFO) order instead of BinaryHeap-arbitrary
    /// order. Deterministic firing is part of the engine's contract.
    #[serde(default)]
    pub seq: u64,
}

impl PartialEq for AgendaItem {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for AgendaItem {}

impl PartialOrd for AgendaItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AgendaItem {
    fn cmp(&self, other: &Self) -> Ordering {
        // Max-heap: higher salience pops first; on ties, the lower seq
        // (earlier insertion) must pop first, so it compares as greater.
        self.salience
            .cmp(&other.salience)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

#[derive(Debug)]
pub struct Agenda {
    pub items: BinaryHeap<AgendaItem>,
    /// Next insertion sequence number for FIFO tie-breaking.
    next_seq: u64,
}

impl Default for Agenda {
    fn default() -> Self {
        Self::new()
    }
}

impl Agenda {
    pub fn new() -> Self {
        Agenda {
            items: BinaryHeap::new(),
            next_seq: 0,
        }
    }

    /// Add an agenda item to the agenda
    pub fn add_item(
        &mut self,
        rule: Rule,
        wme_list: Vec<WorkingMemoryElement>,
        bindings: Bindings,
        salience: i32,
    ) {
        let seq = self.next_seq;
        self.next_seq += 1;
        let agenda_item = AgendaItem {
            rule,
            wme_list,
            bindings,
            salience,
            id: Uuid::new_v4().to_string(),
            seq,
        };
        self.items.push(agenda_item);
    }

    /// Get the next highest priority agenda item
    pub fn pop_next(&mut self) -> Option<AgendaItem> {
        self.items.pop()
    }

    /// Get the next highest priority item without removing it
    pub fn peek_next(&self) -> Option<&AgendaItem> {
        self.items.peek()
    }

    /// Get all items in firing order (salience descending, FIFO on ties)
    pub fn get_all_items(&self) -> Vec<&AgendaItem> {
        let mut items: Vec<&AgendaItem> = self.items.iter().collect();
        items.sort_by(|a, b| b.cmp(a)); // same total order pop_next uses
        items
    }

    /// Clear all items from the agenda
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Get the number of items in the agenda
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Check if the agenda is empty
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Remove items based on a condition
    pub fn remove_by_condition<F>(&mut self, condition: F) -> Vec<AgendaItem>
    where
        F: Fn(&AgendaItem) -> bool,
    {
        let mut to_remove = Vec::new();
        let mut remaining_items = BinaryHeap::new();

        // Extract all items
        while let Some(item) = self.items.pop() {
            if condition(&item) {
                to_remove.push(item);
            } else {
                remaining_items.push(item);
            }
        }

        // Put remaining items back
        self.items = remaining_items;

        to_remove
    }

    /// Pop the best matching item, preserving every other item's sequence.
    /// The caller holds the agenda lock for selection and heap reconstruction.
    ///
    /// Fast path: the heap root is already the overall maximum element by
    /// `Ord` (salience, then FIFO). If it also satisfies `predicate`, it is
    /// necessarily the maximum *matching* element too — no other element
    /// can outrank it — so it can be removed with the heap's native
    /// `pop` (O(log n), heap invariant preserved) instead of the fallback's
    /// linear scan and full rebuild. Each matching heap root costs O(log n)
    /// to select rather than O(n). An all-matching agenda uses this path
    /// throughout; a higher-priority nonmatching root still forces the linear
    /// fallback, even when most remaining items match. This changes selection
    /// cost, not which activation is consumed or the survivors' sequences.
    ///
    /// Fallback: when the root does not match (or the agenda is empty),
    /// behavior is unchanged from before this fast path existed — scan
    /// every item, select the `Ord`-maximum among matches, remove it by
    /// index, and rebuild the heap from what remains.
    #[cfg(feature = "embedding-host")]
    pub(crate) fn pop_matching<F>(&mut self, mut predicate: F) -> Option<AgendaItem>
    where
        F: FnMut(&AgendaItem) -> bool,
    {
        if self.items.peek().is_some_and(&mut predicate) {
            return self.items.pop();
        }

        let mut items = std::mem::take(&mut self.items).into_vec();
        let index = items
            .iter()
            .enumerate()
            .filter(|(_, item)| predicate(item))
            .max_by(|(_, a), (_, b)| a.cmp(b))
            .map(|(index, _)| index);
        let selected = index.map(|index| items.swap_remove(index));
        self.items = BinaryHeap::from(items);
        selected
    }
}

#[cfg(all(test, feature = "embedding-host"))]
mod pop_matching_tests {
    use super::*;
    use crate::Fact;
    use crate::engine_types::Rule;
    use crate::wme::WorkingMemoryElement;

    fn item(agenda: &mut Agenda, id: &str, salience: i32) {
        let fact = Fact {
            id: id.to_string(),
            predicate: "p".to_string(),
            args: vec![],
            timestamp: 0,
            source: None,
        };
        let wme = WorkingMemoryElement::new(fact);
        agenda.add_item(
            Rule {
                id: format!("r-{id}"),
                priority: salience,
                conditions: vec![],
                actions: vec![],
            },
            vec![wme],
            Bindings::new(),
            salience,
        );
    }

    /// Matches come back in normal firing order; non-matching items stay
    /// on the agenda with their original relative order.
    #[test]
    fn partitions_in_firing_order_and_leaves_the_rest_untouched() {
        let mut agenda = Agenda::new();
        item(&mut agenda, "a", 5);
        item(&mut agenda, "b", 1);
        item(&mut agenda, "c", 5);

        let matches: Vec<_> =
            std::iter::from_fn(|| agenda.pop_matching(|i| i.wme_list[0].id != "b")).collect();
        let match_ids: Vec<&str> = matches.iter().map(|i| i.wme_list[0].id.as_str()).collect();
        assert_eq!(match_ids, vec!["a", "c"]);

        let remaining = agenda.get_all_items();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].wme_list[0].id, "b");
    }

    #[test]
    fn empty_predicate_match_leaves_the_agenda_fully_intact() {
        let mut agenda = Agenda::new();
        item(&mut agenda, "a", 1);
        item(&mut agenda, "b", 1);

        let matched = agenda.pop_matching(|_| false);
        assert!(matched.is_none());
        assert_eq!(agenda.len(), 2);
    }

    /// When the heap root already matches, it is the overall `Ord` maximum,
    /// so it is also the maximum among matches — no other item can ever
    /// outrank it. The fast path must take it via the heap's native `pop`
    /// without visiting the rest of the agenda; a predicate call count of 1
    /// is the deterministic signal (not wall-clock) that the O(n) scan was
    /// skipped. This is the dense case a fact-scoped drain hits when many
    /// (here, all) pending items legitimately match one just-asserted fact.
    #[test]
    fn root_match_takes_native_pop_without_scanning_the_rest() {
        let mut agenda = Agenda::new();
        for i in 0..5 {
            item(&mut agenda, &format!("item-{i}"), 1);
        }

        let calls = std::cell::Cell::new(0usize);
        let popped = agenda.pop_matching(|_| {
            calls.set(calls.get() + 1);
            true
        });

        assert!(popped.is_some());
        assert_eq!(
            calls.get(),
            1,
            "root already matches: the fast path must not scan the other 4 items"
        );
        assert_eq!(agenda.len(), 4);
    }

    /// Draining an agenda where every item matches must select items in
    /// exactly the same order repeated `pop_matching` calls always have
    /// (salience, then FIFO) — the fast path changes *how* an item is
    /// removed, never *which* one or the order of removal.
    #[test]
    fn dense_all_matching_drain_preserves_firing_order() {
        let mut agenda = Agenda::new();
        item(&mut agenda, "a", 5);
        item(&mut agenda, "b", 5);
        item(&mut agenda, "c", 1);
        item(&mut agenda, "d", 5);

        let drained: Vec<&str> = std::iter::from_fn(|| agenda.pop_matching(|_| true))
            .map(|item| {
                // Leak the id out of the popped item via its single wme.
                Box::leak(item.wme_list[0].id.clone().into_boxed_str()) as &str
            })
            .collect();

        assert_eq!(drained, vec!["a", "b", "d", "c"]);
        assert!(agenda.is_empty());
    }

    /// The root-peek probe itself must never match an item the caller's
    /// predicate would reject, and when the root does not match, behavior
    /// must fall back to the full scan exactly as before this fast path —
    /// unrelated items keep their relative order (same assertion as
    /// `partitions_in_firing_order_and_leaves_the_rest_untouched`, repeated
    /// here with the root deliberately excluded so the fallback path is
    /// what's exercised).
    #[test]
    fn root_non_match_falls_back_to_full_scan() {
        let mut agenda = Agenda::new();
        item(&mut agenda, "a", 5); // root: highest salience, earliest.
        item(&mut agenda, "b", 1);
        item(&mut agenda, "c", 5);

        // Excludes "a" (the root), so this call must fall back to scanning.
        let popped = agenda.pop_matching(|i| i.wme_list[0].id != "a" && i.wme_list[0].id != "b");
        assert_eq!(popped.unwrap().wme_list[0].id, "c");

        let remaining: Vec<&str> = agenda
            .get_all_items()
            .iter()
            .map(|i| i.wme_list[0].id.as_str())
            .collect();
        assert_eq!(remaining, vec!["a", "b"]);
    }
}
