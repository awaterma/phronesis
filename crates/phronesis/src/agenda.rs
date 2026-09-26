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
}
