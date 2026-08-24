//! Durable context capsules emitted by rules.
//!
//! A capsule is a bounded piece of context that a rule can emit to guide
//! future LLM interactions. Capsules survive across interactions according
//! to their lifecycle policy.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::info;

const SCHEMA_VERSION: u8 = 1;
const MAX_RECORDS: usize = 128;
const LOW_PRIORITY_RECORDS: usize = 96;
const GOVERNANCE_PRIORITY: i32 = 50;
const MAX_AGGREGATE_BYTES: usize = 256 * 1024;
const MAX_RECORD_BYTES: usize = 8 * 1024;
const MAX_PROVENANCE_ITEMS: usize = 32;
pub const LEASE_SECS: u64 = 300;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapsuleFile {
    version: u8,
    capsules: HashMap<String, EmittedCapsule>,
}

/// Lifecycle policy for a capsule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapsuleLifecycle {
    /// Offered on the next interaction-context pass, then removed.
    NextInteraction,
    /// Offered only in the session in which it was emitted.
    Session,
    /// Offered across sessions until expiry or explicit retraction.
    Persistent,
}

impl std::fmt::Display for CapsuleLifecycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapsuleLifecycle::NextInteraction => write!(f, "next_interaction"),
            CapsuleLifecycle::Session => write!(f, "session"),
            CapsuleLifecycle::Persistent => write!(f, "persistent"),
        }
    }
}

/// Parse a bounded duration string like "7d", "2h", "30m", "60s".
/// Returns None for invalid formats or unbounded values.
pub fn parse_duration(s: &str) -> Option<Duration> {
    if s.is_empty() {
        return None;
    }
    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: u64 = num_str.parse().ok()?;
    if num == 0 {
        return None;
    }
    match unit {
        "s" => Some(Duration::from_secs(num)),
        "m" => num.checked_mul(60).map(Duration::from_secs),
        "h" => num.checked_mul(3600).map(Duration::from_secs),
        "d" => num.checked_mul(86400).map(Duration::from_secs),
        _ => None,
    }
}

/// A capsule emitted by a rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmittedCapsule {
    /// Unique capsule ID (validated static string, no variable substitution).
    pub id: String,
    /// The body text (may contain variable substitutions).
    pub body: String,
    /// Lifecycle policy.
    pub lifecycle: CapsuleLifecycle,
    /// Priority for ordering (higher = more important).
    pub priority: i32,
    /// Absolute expiry timestamp (UNIX epoch seconds), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// The rule ID that emitted this capsule.
    pub emitted_by: String,
    /// Timestamp when emitted (UNIX epoch seconds).
    pub emitted_at: u64,
    /// Session ID in which this was emitted.
    pub session_id: String,
    /// Bounded provenance: fact IDs that triggered emission.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bound_facts: Vec<String>,
    /// Variable bindings at emission time.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub bindings: HashMap<String, String>,
    /// Bounded `fact-id=source` attribution entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fact_sources: Vec<String>,
    /// Bounded decision references attached to the firing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    /// For next_interaction lifecycle: lease expiry timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_until: Option<u64>,
    /// Acknowledgement status for next_interaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged: Option<bool>,
    /// Opaque delivery token. Selection is not delivery; the host must ack it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_token: Option<String>,
}

/// Storage for emitted capsules.
#[derive(Debug, Default)]
pub struct CapsuleStorage {
    capsules: HashMap<String, EmittedCapsule>,
    storage_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum CapsuleError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Capsule ID conflict: {0} emitted by {1}, attempted by {2}")]
    IdConflict(String, String, String),
    #[error("Storage capacity exceeded: {0}")]
    CapacityExceeded(String),
    #[error("Invalid capsule: {0}")]
    Invalid(String),
    #[error("Lock error: {0}")]
    Lock(String),
}

/// Result type for capsule operations.
pub type CapsuleResult<T> = Result<T, CapsuleError>;

impl CapsuleStorage {
    /// Create a new storage at the given project root.
    pub fn new(project_root: &Path) -> Self {
        let phronesis_dir = project_root.join(".phronesis");
        Self {
            capsules: HashMap::new(),
            storage_path: phronesis_dir.join("emitted-capsules.json"),
        }
    }

    /// Load existing capsules from disk.
    pub fn load(&mut self) -> CapsuleResult<()> {
        if !self.storage_path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&self.storage_path)?;
        let file: CapsuleFile = serde_json::from_str(&content)?;
        if file.version != SCHEMA_VERSION {
            return Err(CapsuleError::Invalid(format!(
                "unsupported emitted capsule schema version {}",
                file.version
            )));
        }
        self.capsules = file.capsules;
        Ok(())
    }

    /// Save capsules to disk atomically.
    pub fn save(&self) -> CapsuleResult<()> {
        if let Some(parent) = self.storage_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(&CapsuleFile {
            version: SCHEMA_VERSION,
            capsules: self.capsules.clone(),
        })?;
        let tmp_path = self.storage_path.with_extension(format!(
            "json.tmp.{}.{}.{}",
            std::process::id(),
            unix_nanos_now(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut tmp = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        tmp.write_all(&json)?;
        tmp.sync_all()?;
        std::fs::rename(&tmp_path, &self.storage_path)?;
        if let Some(parent) = self.storage_path.parent()
            && let Ok(dir) = File::open(parent)
        {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    /// Emit a new capsule. Returns true if emitted, false if rejected.
    ///
    /// Same rule re-emission upserts; different rule same ID is rejected.
    pub fn emit(&mut self, capsule: EmittedCapsule) -> CapsuleResult<bool> {
        // Validate size limits
        if capsule.body.len() > MAX_RECORD_BYTES {
            return Err(CapsuleError::Invalid(
                "Capsule body exceeds 8 KiB limit".to_string(),
            ));
        }

        // Check capacity
        if !self.capsules.contains_key(&capsule.id) && self.capsules.len() >= MAX_RECORDS {
            return Err(CapsuleError::CapacityExceeded(format!(
                "Maximum {MAX_RECORDS} capsules allowed"
            )));
        }
        if !self.capsules.contains_key(&capsule.id)
            && capsule.priority < GOVERNANCE_PRIORITY
            && self
                .capsules
                .values()
                .filter(|record| record.priority < GOVERNANCE_PRIORITY)
                .count()
                >= LOW_PRIORITY_RECORDS
        {
            return Err(CapsuleError::CapacityExceeded(
                "low-priority capsule pool is full; 32 records are reserved for priority >= 50 governance capsules".into(),
            ));
        }

        // Check for ID conflict
        if let Some(existing) = self.capsules.get(&capsule.id) {
            if existing.emitted_by != capsule.emitted_by {
                // Different rule, same ID = conflict
                return Err(CapsuleError::IdConflict(
                    capsule.id.clone(),
                    existing.emitted_by.clone(),
                    capsule.emitted_by.clone(),
                ));
            }
            // Same rule re-emission = upsert
            info!(
                "Capsule {} upserted by rule {}",
                capsule.id, capsule.emitted_by
            );
        }

        let record_bytes = serde_json::to_vec(&capsule)?.len();
        if record_bytes > MAX_RECORD_BYTES {
            return Err(CapsuleError::Invalid(
                "capsule record exceeds 8 KiB limit".into(),
            ));
        }
        let capsule_id = capsule.id.clone();
        let previous = self.capsules.insert(capsule_id.clone(), capsule);
        let aggregate = serde_json::to_vec(&self.capsules)?.len();
        if aggregate > MAX_AGGREGATE_BYTES {
            if let Some(previous) = previous {
                self.capsules.insert(previous.id.clone(), previous);
            } else {
                self.capsules.remove(&capsule_id);
            }
            return Err(CapsuleError::CapacityExceeded(
                "256 KiB aggregate emitted capsule limit exceeded".into(),
            ));
        }
        Ok(true)
    }

    /// Retract a capsule by ID.
    pub fn retract(&mut self, id: &str) -> Option<EmittedCapsule> {
        self.capsules.remove(id)
    }

    /// Acknowledge a capsule (for next_interaction lifecycle).
    pub fn acknowledge(&mut self, id: &str) -> Option<EmittedCapsule> {
        if let Some(capsule) = self.capsules.get_mut(id) {
            capsule.acknowledged = Some(true);
            Some(capsule.clone())
        } else {
            None
        }
    }

    /// Get all capsules eligible for display (not expired, not acknowledged).
    pub fn eligible_capsules(&self, session_id: &str, now: u64) -> Vec<&EmittedCapsule> {
        let mut eligible: Vec<_> = self
            .capsules
            .values()
            .filter(|c| {
                // Check expiry
                if let Some(expires_at) = c.expires_at
                    && now > expires_at
                {
                    return false;
                }
                // Check acknowledgement
                if c.acknowledged == Some(true) {
                    return false;
                }
                // Check lifecycle eligibility
                match c.lifecycle {
                    CapsuleLifecycle::Session => c.session_id == session_id,
                    CapsuleLifecycle::Persistent | CapsuleLifecycle::NextInteraction => true,
                }
            })
            .collect();

        // Sort by priority (higher first), then by emitted_at (older first)
        eligible.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.emitted_at.cmp(&b.emitted_at))
        });

        eligible
    }

    /// Get capsules for next interaction (with lease handling).
    pub fn next_interaction_capsules(&mut self, now: u64, lease_duration: Duration) -> Vec<String> {
        let mut leased_ids = Vec::new();

        for capsule in self.capsules.values() {
            if capsule.lifecycle != CapsuleLifecycle::NextInteraction {
                continue;
            }

            // Check expiry
            if let Some(expires_at) = capsule.expires_at
                && now > expires_at
            {
                continue;
            }

            // Check acknowledgement
            if capsule.acknowledged == Some(true) {
                continue;
            }

            // Check lease - if leased and not expired, skip
            if let Some(lease_until) = capsule.lease_until
                && now < lease_until
            {
                continue;
            }

            leased_ids.push(capsule.id.clone());
        }

        // Mark as leased
        for id in &leased_ids {
            if let Some(capsule) = self.capsules.get_mut(id) {
                capsule.lease_until = Some(now + lease_duration.as_secs());
                capsule.lease_token = Some(format!("{}-{now}-{}", id, std::process::id()));
            }
        }

        leased_ids
    }

    /// Remove expired capsules.
    pub fn remove_expired(&mut self, now: u64) -> Vec<String> {
        let mut removed = Vec::new();
        self.capsules.retain(|id, capsule| {
            if let Some(expires_at) = capsule.expires_at
                && now > expires_at
            {
                removed.push(id.clone());
                return false;
            }
            true
        });
        removed
    }

    /// Remove acknowledged next_interaction capsules.
    pub fn remove_acknowledged(&mut self) -> Vec<String> {
        let mut removed = Vec::new();
        self.capsules.retain(|id, capsule| {
            if capsule.lifecycle == CapsuleLifecycle::NextInteraction
                && capsule.acknowledged == Some(true)
            {
                removed.push(id.clone());
                return false;
            }
            true
        });
        removed
    }

    /// Get all capsules (for listing).
    pub fn all_capsules(&self) -> &HashMap<String, EmittedCapsule> {
        &self.capsules
    }

    /// Get a single capsule by ID.
    pub fn get_capsule(&self, id: &str) -> Option<&EmittedCapsule> {
        self.capsules.get(id)
    }

    /// Get capsule count.
    pub fn len(&self) -> usize {
        self.capsules.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.capsules.is_empty()
    }

    /// Clear all capsules.
    pub fn clear(&mut self) {
        self.capsules.clear();
    }
}

/// Provenance attached to a capsule built from an `emit_capsule` action.
#[derive(Debug, Clone, Copy)]
pub struct CapsuleOrigin<'a> {
    /// Rule ID (or "validation") that emitted the capsule.
    pub emitted_by: &'a str,
    /// Session the capsule belongs to.
    pub session_id: &'a str,
    /// Emission timestamp (unix seconds).
    pub now: u64,
    /// Facts bound when the rule fired.
    pub bound_facts: &'a [String],
    /// Variable bindings when the rule fired.
    pub bindings: &'a HashMap<String, String>,
}

impl<'a> CapsuleOrigin<'a> {
    /// Origin used when only validating a spec (no real session/provenance).
    pub fn validation(emitted_by: &'a str) -> Self {
        static EMPTY_BINDINGS: std::sync::LazyLock<HashMap<String, String>> =
            std::sync::LazyLock::new(HashMap::new);
        Self {
            emitted_by,
            session_id: "validation",
            now: 0,
            bound_facts: &[],
            bindings: &EMPTY_BINDINGS,
        }
    }
}

/// Build an EmittedCapsule from an action's data field.
///
/// Thin shim over [`build_capsule`] kept for existing callers.
pub fn build_capsule_from_action(
    action_data: &serde_json::Value,
    emitted_by: &str,
    session_id: &str,
    now: u64,
    bound_facts: &[String],
    bindings: &HashMap<String, String>,
) -> CapsuleResult<EmittedCapsule> {
    build_capsule(
        action_data,
        &CapsuleOrigin {
            emitted_by,
            session_id,
            now,
            bound_facts,
            bindings,
        },
    )
}

/// Build an EmittedCapsule from an action's data field and its origin.
pub fn build_capsule(
    action_data: &serde_json::Value,
    origin: &CapsuleOrigin<'_>,
) -> CapsuleResult<EmittedCapsule> {
    let CapsuleOrigin {
        emitted_by,
        session_id,
        now,
        bound_facts,
        bindings,
    } = *origin;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct CapsuleSpec {
        id: String,
        body: String,
        lifecycle: String,
        #[serde(default)]
        priority: i32,
        #[serde(default)]
        max_bytes: Option<u64>,
        #[serde(default)]
        expires_after: Option<String>,
    }

    let spec: CapsuleSpec = serde_json::from_value(action_data.clone())
        .map_err(|e| CapsuleError::Invalid(format!("Invalid capsule spec: {}", e)))?;

    // Validate ID (static string, no variable substitution)
    if spec.id.is_empty()
        || spec.id.len() > 128
        || spec.id.contains('?')
        || !spec
            .id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(CapsuleError::Invalid(
            "Capsule ID must not contain variable substitutions".to_string(),
        ));
    }

    // Validate lifecycle
    let lifecycle = match spec.lifecycle.as_str() {
        "next_interaction" => CapsuleLifecycle::NextInteraction,
        "session" => CapsuleLifecycle::Session,
        "persistent" => CapsuleLifecycle::Persistent,
        other => {
            return Err(CapsuleError::Invalid(format!(
                "Invalid lifecycle: {}",
                other
            )));
        }
    };

    // Validate body size
    let max_bytes = spec.max_bytes.unwrap_or(MAX_RECORD_BYTES as u64);
    if max_bytes == 0 || max_bytes > MAX_RECORD_BYTES as u64 {
        return Err(CapsuleError::Invalid(
            "max_bytes must be between 1 and 8192".into(),
        ));
    }
    if spec.body.len() as u64 > max_bytes {
        return Err(CapsuleError::Invalid(format!(
            "Capsule body exceeds max_bytes limit of {}",
            max_bytes
        )));
    }

    // Parse expiry
    let expires_at = match spec.expires_after.as_deref() {
        None => None,
        Some(raw) => Some(
            now.checked_add(
                parse_duration(raw)
                    .ok_or_else(|| CapsuleError::Invalid(format!("invalid expires_after: {raw}")))?
                    .as_secs(),
            )
            .ok_or_else(|| CapsuleError::Invalid("expires_after overflows timestamp".into()))?,
        ),
    };

    Ok(EmittedCapsule {
        id: spec.id,
        body: spec.body,
        lifecycle,
        priority: spec.priority,
        expires_at,
        emitted_by: emitted_by.to_string(),
        emitted_at: now,
        session_id: session_id.to_string(),
        bound_facts: bound_facts.to_vec(),
        bindings: bindings.clone(),
        fact_sources: Vec::new(),
        decisions: Vec::new(),
        lease_until: None,
        acknowledged: None,
        lease_token: None,
    })
}

fn lock_path(root: &Path) -> PathBuf {
    root.join(".phronesis/emitted-capsules.lock")
}

/// Exclusive read-modify-write transaction. The stable lock file is separate
/// from the atomically replaced data inode.
pub fn transaction<T>(
    root: &Path,
    operation: impl FnOnce(&mut CapsuleStorage) -> CapsuleResult<T>,
) -> CapsuleResult<T> {
    let dir = root.join(".phronesis");
    std::fs::create_dir_all(&dir)?;
    let data_path = dir.join("emitted-capsules.json");
    if std::fs::symlink_metadata(&data_path).is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(CapsuleError::Invalid(
            "emitted capsule data path must not be a symlink".into(),
        ));
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path(root))?;
    lock.lock_exclusive()
        .map_err(|error| CapsuleError::Lock(error.to_string()))?;
    let mut storage = CapsuleStorage::new(root);
    storage.load()?;
    let result = operation(&mut storage)?;
    storage.save()?;
    fs2::FileExt::unlock(&lock).map_err(|error| CapsuleError::Lock(error.to_string()))?;
    Ok(result)
}

/// Read a consistent snapshot without rewriting data or changing leases.
pub fn read_snapshot(root: &Path) -> CapsuleResult<Vec<EmittedCapsule>> {
    let dir = root.join(".phronesis");
    std::fs::create_dir_all(&dir)?;
    let data_path = dir.join("emitted-capsules.json");
    if std::fs::symlink_metadata(&data_path).is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(CapsuleError::Invalid(
            "emitted capsule data path must not be a symlink".into(),
        ));
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path(root))?;
    lock.lock_shared()
        .map_err(|error| CapsuleError::Lock(error.to_string()))?;
    let mut storage = CapsuleStorage::new(root);
    storage.load()?;
    let records = storage.all_capsules().values().cloned().collect();
    fs2::FileExt::unlock(&lock).map_err(|error| CapsuleError::Lock(error.to_string()))?;
    Ok(records)
}

/// Lease only selected next-interaction records. Budget-omitted records remain
/// immediately eligible, so inspection and packing pressure cannot consume them.
pub fn lease_selected(
    root: &Path,
    ids: &[String],
    now: u64,
) -> CapsuleResult<Vec<(String, String)>> {
    transaction(root, |storage| {
        let mut leased = Vec::new();
        for id in ids {
            if let Some(capsule) = storage.capsules.get_mut(id)
                && capsule.lifecycle == CapsuleLifecycle::NextInteraction
                && capsule.lease_until.is_none_or(|until| now >= until)
            {
                let token = format!("{}-{now}-{}", id, std::process::id());
                capsule.lease_until = Some(now.saturating_add(LEASE_SECS));
                capsule.lease_token = Some(token.clone());
                leased.push((id.clone(), token));
            }
        }
        Ok(leased)
    })
}

/// Capture all emit_capsule consequences. Individual invalid/conflicting
/// emissions are fail-open and returned as visible diagnostics.
pub fn capture_consequences(
    root: &Path,
    consequences: &[phr::Consequence],
    session_id: &str,
    now: u64,
) -> Vec<String> {
    let mut diagnostics = Vec::new();
    for consequence in consequences {
        if consequence
            .payload
            .get("action_type")
            .and_then(serde_json::Value::as_str)
            != Some("emit_capsule")
        {
            continue;
        }
        let Some(data) = consequence
            .payload
            .get("data")
            .filter(|value| !value.is_null())
        else {
            diagnostics.push("emit_capsule consequence has no structured data".into());
            continue;
        };
        let phr::Provenance::RuleFiring {
            rule_id,
            bound_facts,
            bindings,
            fact_sources,
            decisions,
            ..
        } = &consequence.provenance
        else {
            diagnostics.push("emit_capsule consequence lacks rule-firing provenance".into());
            continue;
        };
        let bounded_facts = bound_facts
            .iter()
            .take(MAX_PROVENANCE_ITEMS)
            .map(|value| bounded(value))
            .collect::<Vec<_>>();
        let bounded_bindings = bindings
            .iter()
            .take(MAX_PROVENANCE_ITEMS)
            .map(|(key, value)| (bounded(key), bounded(value)))
            .collect::<HashMap<_, _>>();
        let mut capsule = match build_capsule(
            data,
            &CapsuleOrigin {
                emitted_by: rule_id.as_ref(),
                session_id,
                now,
                bound_facts: &bounded_facts,
                bindings: &bounded_bindings,
            },
        ) {
            Ok(capsule) => capsule,
            Err(error) => {
                diagnostics.push(error.to_string());
                continue;
            }
        };
        capsule.fact_sources = fact_sources
            .iter()
            .take(MAX_PROVENANCE_ITEMS)
            .map(|(id, source)| format!("{}={}", bounded(id), bounded(source)))
            .collect();
        capsule.decisions = decisions
            .iter()
            .take(MAX_PROVENANCE_ITEMS)
            .map(|decision| bounded(decision))
            .collect();
        if let Err(error) = transaction(root, |storage| storage.emit(capsule)) {
            diagnostics.push(error.to_string());
        }
    }
    diagnostics
}

fn bounded(value: &str) -> String {
    const LIMIT: usize = 256;
    if value.len() <= LIMIT {
        return value.to_string();
    }
    let mut end = LIMIT;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

/// Get current session ID from environment or generate one.
pub fn get_session_id() -> String {
    std::env::var("PHRONESIS_SESSION_ID").unwrap_or_else(|_| format!("session-{}", unix_secs_now()))
}

pub fn unix_secs_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn unix_nanos_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

/// Hook convenience: capture operation identity once and surface advisory
/// persistence failures without changing the hook verdict.
pub fn capture_for_hook(root: &Path, consequences: &[phr::Consequence]) {
    let session_id = crate::journey::current_sid(root);
    let now = unix_secs_now();
    for diagnostic in capture_consequences(root, consequences, &session_id, now) {
        eprintln!("phronesis: WARNING — emitted capsule not persisted: {diagnostic}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_valid_formats() {
        assert_eq!(parse_duration("7d"), Some(Duration::from_secs(7 * 86400)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(2 * 3600)));
        assert_eq!(parse_duration("30m"), Some(Duration::from_secs(30 * 60)));
        assert_eq!(parse_duration("60s"), Some(Duration::from_secs(60)));
    }

    #[test]
    fn parse_duration_invalid_formats() {
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("7x"), None);
        assert_eq!(parse_duration("-1d"), None);
    }

    #[test]
    fn capsule_storage_basic() {
        let mut storage = CapsuleStorage::default();
        let capsule = EmittedCapsule {
            id: "test-capsule".to_string(),
            body: "Test body".to_string(),
            lifecycle: CapsuleLifecycle::Session,
            priority: 10,
            expires_at: None,
            emitted_by: "test-rule".to_string(),
            emitted_at: 1000,
            session_id: "session-1".to_string(),
            bound_facts: vec![],
            bindings: HashMap::new(),
            fact_sources: Vec::new(),
            decisions: Vec::new(),
            lease_until: None,
            acknowledged: None,
            lease_token: None,
        };

        assert!(storage.emit(capsule).unwrap());
        assert_eq!(storage.len(), 1);
        assert!(storage.get_capsule("test-capsule").is_some());
    }

    #[test]
    fn same_rule_reemission_upserts() {
        let mut storage = CapsuleStorage::default();
        let capsule1 = EmittedCapsule {
            id: "test".to_string(),
            body: "First".to_string(),
            lifecycle: CapsuleLifecycle::Session,
            priority: 10,
            expires_at: None,
            emitted_by: "rule-a".to_string(),
            emitted_at: 1000,
            session_id: "session-1".to_string(),
            bound_facts: vec![],
            bindings: HashMap::new(),
            fact_sources: Vec::new(),
            decisions: Vec::new(),
            lease_until: None,
            acknowledged: None,
            lease_token: None,
        };

        let capsule2 = EmittedCapsule {
            id: "test".to_string(),
            body: "Second".to_string(),
            lifecycle: CapsuleLifecycle::Session,
            priority: 20,
            expires_at: None,
            emitted_by: "rule-a".to_string(),
            emitted_at: 2000,
            session_id: "session-1".to_string(),
            bound_facts: vec![],
            bindings: HashMap::new(),
            fact_sources: Vec::new(),
            decisions: Vec::new(),
            lease_until: None,
            acknowledged: None,
            lease_token: None,
        };

        assert!(storage.emit(capsule1).unwrap());
        assert!(storage.emit(capsule2).unwrap());
        assert_eq!(storage.len(), 1);
        assert_eq!(storage.get_capsule("test").unwrap().body, "Second");
    }

    #[test]
    fn different_rule_same_id_conflicts() {
        let mut storage = CapsuleStorage::default();
        let capsule1 = EmittedCapsule {
            id: "test".to_string(),
            body: "First".to_string(),
            lifecycle: CapsuleLifecycle::Session,
            priority: 10,
            expires_at: None,
            emitted_by: "rule-a".to_string(),
            emitted_at: 1000,
            session_id: "session-1".to_string(),
            bound_facts: vec![],
            bindings: HashMap::new(),
            fact_sources: Vec::new(),
            decisions: Vec::new(),
            lease_until: None,
            acknowledged: None,
            lease_token: None,
        };

        let capsule2 = EmittedCapsule {
            id: "test".to_string(),
            body: "Second".to_string(),
            lifecycle: CapsuleLifecycle::Session,
            priority: 10,
            expires_at: None,
            emitted_by: "rule-b".to_string(),
            emitted_at: 2000,
            session_id: "session-1".to_string(),
            bound_facts: vec![],
            bindings: HashMap::new(),
            fact_sources: Vec::new(),
            decisions: Vec::new(),
            lease_until: None,
            acknowledged: None,
            lease_token: None,
        };

        assert!(storage.emit(capsule1).unwrap());
        assert!(matches!(
            storage.emit(capsule2),
            Err(CapsuleError::IdConflict(_, _, _))
        ));
        assert_eq!(storage.len(), 1);
    }

    fn record(id: &str, lifecycle: CapsuleLifecycle, session: &str) -> EmittedCapsule {
        EmittedCapsule {
            id: id.into(),
            body: format!("body {id}"),
            lifecycle,
            priority: 50,
            expires_at: None,
            emitted_by: format!("rule-{id}"),
            emitted_at: 100,
            session_id: session.into(),
            bound_facts: Vec::new(),
            bindings: HashMap::new(),
            fact_sources: Vec::new(),
            decisions: Vec::new(),
            lease_until: None,
            acknowledged: None,
            lease_token: None,
        }
    }

    #[test]
    fn transaction_is_schema_versioned_and_concurrent_writers_do_not_lose_records() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let threads = (0..12)
            .map(|index| {
                let root = root.clone();
                std::thread::spawn(move || {
                    transaction(&root, |storage| {
                        storage.emit(record(
                            &format!("c{index}"),
                            CapsuleLifecycle::Persistent,
                            "s",
                        ))
                    })
                    .unwrap()
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        let records = read_snapshot(&root).unwrap();
        assert_eq!(records.len(), 12);
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join(".phronesis/emitted-capsules.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(json["version"], SCHEMA_VERSION);
    }

    #[test]
    fn lease_ack_retry_and_session_eligibility_are_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        transaction(root, |storage| {
            storage.emit(record("next", CapsuleLifecycle::NextInteraction, "s1"))?;
            storage.emit(record("session", CapsuleLifecycle::Session, "s1"))?;
            storage.emit(record("persistent", CapsuleLifecycle::Persistent, "s1"))
        })
        .unwrap();
        let leased = lease_selected(root, &["next".into()], 1000).unwrap();
        assert_eq!(leased.len(), 1);
        let snapshot = read_snapshot(root).unwrap();
        let next = snapshot.iter().find(|record| record.id == "next").unwrap();
        assert_eq!(next.lease_until, Some(1000 + LEASE_SECS));
        assert!(next.lease_token.is_some());
        transaction(root, |storage| {
            assert!(
                !storage
                    .eligible_capsules("s2", 1001)
                    .iter()
                    .any(|record| record.id == "session")
            );
            assert!(
                storage
                    .eligible_capsules("s2", 1001)
                    .iter()
                    .any(|record| record.id == "persistent")
            );
            assert!(
                storage
                    .next_interaction_capsules(1001, Duration::from_secs(5))
                    .is_empty()
            );
            assert_eq!(
                storage.next_interaction_capsules(1000 + LEASE_SECS, Duration::from_secs(5)),
                ["next"]
            );
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn malformed_and_symlink_data_fail_without_overwriting_target() {
        let dir = tempfile::tempdir().unwrap();
        let phronesis = dir.path().join(".phronesis");
        std::fs::create_dir(&phronesis).unwrap();
        std::fs::write(phronesis.join("emitted-capsules.json"), "not json").unwrap();
        assert!(read_snapshot(dir.path()).is_err());
        #[cfg(unix)]
        {
            std::fs::remove_file(phronesis.join("emitted-capsules.json")).unwrap();
            let target = dir.path().join("target");
            std::fs::write(&target, "safe").unwrap();
            std::os::unix::fs::symlink(&target, phronesis.join("emitted-capsules.json")).unwrap();
            assert!(
                transaction(dir.path(), |storage| storage.emit(record(
                    "x",
                    CapsuleLifecycle::Persistent,
                    "s"
                )))
                .is_err()
            );
            assert_eq!(std::fs::read_to_string(target).unwrap(), "safe");
        }
    }
}
