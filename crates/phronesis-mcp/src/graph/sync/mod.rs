//! The per-save pipeline and the staleness index.
//!
//! Two tiers, deliberately separated (spec §4.5): **parse** touches only the
//! edited file, while **derive** runs over the whole edge set. Derived facts
//! are therefore correct after every save without reparsing the repository.
//!
//! The index exists because edits can bypass the hook entirely — `git
//! checkout`, `git mv`, a rebase, a plain shell edit. A graph that has
//! silently drifted must not be allowed to block work, so drift is detected
//! and downgrades enforcement to warn.

mod extract;
mod index;
mod rebuild;
mod rules;

pub use index::{TRACKED_EXTENSIONS, check_freshness, load_index, save_index};
pub use rebuild::{on_save, rebuild, record_from_disk};
pub use rules::{deprecated_graph_rule_predicates, reconcile_rules};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Location of the staleness index, relative to project root.
pub const INDEX_REL_PATH: &str = ".phronesis/graph.index";

/// Version of what the extractor writes. Bumped whenever entity naming *or*
/// the closed relation set changes, because either invalidates edges already
/// on disk while leaving file contents — and therefore content hashes —
/// untouched. Without it, an upgrade silently yields a graph half in the old
/// vocabulary and half in the new, whose `imports` never join to its
/// `declares_module`, and whose missing relations read as clean results.
///
/// A graph whose index records a different number is `Freshness::Outdated`,
/// which only `rebuild` resolves; content hashes prove nothing there.
///
/// Identity scheme since format 5: `<lang>:<package>[#<target>]::<module
/// path>`. Format 5 also introduced `graph_definition`, `defines`,
/// `element_in_file`, `element_in_module`, `graph_module`, `graph_function`,
/// `graph_test`, `graph_file` and multilingual dialect support; format 4 is
/// the same scheme without those relation names. Anything earlier is recorded
/// as 0: pre-versioning, bare `crate::…`.
///
/// 18 — adds the opt-in Rust ownership evidence relations
/// (`graph::ownership::OWNERSHIP_RELATIONS`, see
/// `docs/specs/SPEC-rust-ownership-evidence.md` §6). A format-17 graph lacks
/// the whole set, and absence of ownership evidence must never be readable as
/// absence of an ownership concern, so those graphs rebuild rather than mix
/// generations.
pub const GRAPH_FORMAT: u32 = 19;

/// Header line stamping the format into the index file.
const FORMAT_KEY: &str = "# format";
const GENERATION_KEY: &str = "# generation";

/// Content hashes of every file the graph was built from.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Index {
    /// Identity scheme the graph was built under; 0 for a pre-versioning or
    /// absent index. Only meaningful on load — writes always stamp the
    /// current format, because what we write is by definition current.
    pub format: u32,
    /// Monotonic graph-write generation shared with `bindings.json`.
    pub generation: u64,
    pub entries: BTreeMap<String, u64>,
}

/// Whether the graph still reflects what is on disk.
#[derive(Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    /// Files whose content no longer matches the index, sorted.
    Stale(Vec<String>),
    /// The graph was built under a different identity scheme. Content hashes
    /// prove nothing here: the files are untouched and every edge is still
    /// wrong. Only a rebuild resolves it.
    Outdated {
        found: u32,
        expected: u32,
    },
}

/// Outcome of a single-file save.
#[derive(Debug, PartialEq, Eq)]
/// Additive-safe: callers receive this from `rebuild`/`on_save` rather than
/// constructing it, so future counters can be added without another semver
/// break. Marked after `diagnostics` triggered `constructible_struct_adds_field`
/// in 0.29.0.
#[non_exhaustive]
pub struct SaveOutcome {
    /// Base edges for the whole project after compaction.
    pub base: usize,
    /// Derived edges regenerated this pass.
    pub derived: usize,
    /// Items the extractor declined to name.
    pub skipped: usize,
    /// Rules whose deprecated graph predicates were migrated during rebuild.
    pub migrated_rules: usize,
    /// Stable machine strings naming analysis this run did **not** perform.
    ///
    /// Spec §8.2 requires the compiler provider to record that build scripts
    /// and procedural macros were disabled rather than let a caller assume the
    /// analysis was macro-complete. They are run properties, not graph edges:
    /// encoding them per function would either contradict that function's real
    /// capability status or multiply edges by the function count. Empty on
    /// every path that runs no provider, which is every path except an
    /// explicit rebuild with `provider = "rust-analyzer"`.
    pub diagnostics: Vec<String>,
}

/// Deterministic content hash (FNV-1a, 64-bit).
///
/// Not `DefaultHasher`: that is explicitly not stable across Rust releases,
/// and a hash that changes under the reader would mark every file stale after
/// a toolchain upgrade.
pub fn hash_content(content: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

pub fn index_path(root: &Path) -> PathBuf {
    root.join(INDEX_REL_PATH)
}

#[cfg(test)]
mod tests;
