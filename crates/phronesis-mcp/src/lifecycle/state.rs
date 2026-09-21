//! Correlation state under `.phronesis/journey/`: session, agents, inflight,
//! turn, kalpa. Every read-modify-write holds an exclusive advisory lock on a
//! sibling `<name>.lock` (stable inode; the data file may be truncated). All
//! public writers are best-effort: errors are logged to stderr and swallowed.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

/// How long an `inflight` entry is evidence of an interrupt. Applies to
/// **classification only**: `pop_inflight` pops by key regardless of age, so a
/// twenty-minute build still gets its commit detected.
pub const INFLIGHT_TTL_SECS: u64 = 900;

fn dir(root: &Path) -> PathBuf {
    root.join(".phronesis").join("journey")
}

pub fn with_locked<T>(
    root: &Path,
    name: &str,
    f: impl FnOnce(String) -> (Option<String>, T),
) -> std::io::Result<T> {
    let d = dir(root);
    std::fs::create_dir_all(&d)?;
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(d.join(format!("{name}.lock")))?;
    lock.lock_exclusive()?;
    let path = d.join(name);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    let mut cur = String::new();
    file.read_to_string(&mut cur)?;
    let (next, out) = f(cur);
    if let Some(next) = next {
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        file.write_all(next.as_bytes())?;
    }
    let _ = FileExt::unlock(&lock);
    Ok(out)
}

fn swallow<T>(r: std::io::Result<T>, what: &str) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("phronesis: lifecycle state {what}: {e}");
            None
        }
    }
}

// ---- session ----
/// Which `SessionStart` sources begin a *new* session. `compact` and `fork`
/// continue the current one, so they must leave `session`, `agents`,
/// `inflight` and `turn` alone: a mid-session compaction that orphaned an open
/// sub-agent or discarded an in-flight tool would be worse than no hook at all
/// (spec §Correlation state).
///
/// An absent or unrecognized source is a begin: that is what every host that
/// omits the field means, and resetting is the state the classifier already
/// copes with.
pub fn is_session_begin(source: Option<&str>) -> bool {
    !matches!(source.unwrap_or_default(), "compact" | "fork")
}

/// Overwrite the session id with an **atomic replace** — write a sibling temp
/// file, then rename over the target. `current_sid` reads this file without
/// taking the lock (`journey/mod.rs:42-48`), so a truncate-then-write would
/// give it a window in which the file is empty and it mints a phantom sid that
/// every record written in that millisecond then carries.
///
/// The lock is still held, against other writers; the rename is what protects
/// the lock-free reader.
pub fn set_session(root: &Path, sid: &str) {
    let d = dir(root);
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&d)?;
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(d.join("session.lock"))?;
        lock.lock_exclusive()?;
        // Same directory, so the rename is atomic and cannot cross a mount.
        let tmp = d.join(format!("session.{}.tmp", std::process::id()));
        std::fs::write(&tmp, sid.as_bytes())?;
        let result = std::fs::rename(&tmp, d.join("session"));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        let _ = FileExt::unlock(&lock);
        result
    };
    swallow(write(), "set_session");
}

/// Record the detected host for the current session. Gemini-specific event
/// names (`BeforeAgent`, `AfterAgent`, `BeforeTool`, `AfterTool`) identify
/// Gemini unambiguously, but `SessionStart` and `SessionEnd` are shared with
/// Claude. When a Gemini-specific event fires, the adapter stamps `host` so a
/// later `SessionEnd` can inherit it rather than defaulting to Claude.
pub fn set_host(root: &Path, host: &str) {
    let d = dir(root);
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&d)?;
        let tmp = d.join(format!("host.{}.tmp", std::process::id()));
        std::fs::write(&tmp, host)?;
        let result = std::fs::rename(&tmp, d.join("host"));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    };
    swallow(write(), "set_host");
}

/// Read the host stamped by a prior Gemini-specific event, or `None` when no
/// host has been recorded (Claude-only session, fresh project).
pub fn read_host(root: &Path) -> Option<String> {
    let path = dir(root).join("host");
    let body = std::fs::read_to_string(&path).ok()?;
    let trimmed = body.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Truncate `agents` and `inflight` and reset `turn` to closed, for the current
/// session. Called only on a **session-begin** `SessionStart`
/// (`is_session_begin`). `session` itself is not touched here: the caller
/// overwrote it first, and nothing ever truncates it.
pub fn reset_for_session_start(root: &Path) {
    swallow(
        with_locked(root, "agents", |_| (Some(String::new()), ())),
        "reset agents",
    );
    swallow(
        with_locked(root, "inflight", |_| (Some(String::new()), ())),
        "reset inflight",
    );
    // A session-begin SessionStart clears the host stamp: the next
    // Gemini-specific event re-stamps it, and a Claude-only session leaves it
    // absent so SessionEnd defaults to Claude.
    let _ = std::fs::write(dir(root).join("host"), "");
    let fresh = Turn {
        sid: crate::journey::current_sid(root),
        open: false,
        turn_id: None,
        last_prompt_ts: 0,
        last_event: String::new(),
    };
    swallow(
        with_locked(root, "turn", |_| (serde_json::to_string(&fresh).ok(), ())),
        "reset turn",
    );
}

// ---- agents ----
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OpenAgent {
    pub agent_id: String,
    pub agent_type: Option<String>,
    pub ts: u64,
    pub seq: u64,
}

fn parse_lines<T: for<'de> Deserialize<'de>>(s: &str) -> Vec<T> {
    s.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}
fn to_lines<T: Serialize>(v: &[T]) -> String {
    let mut out = String::new();
    for x in v {
        if let Ok(l) = serde_json::to_string(x) {
            out.push_str(&l);
            out.push('\n');
        }
    }
    out
}

pub fn push_agent(root: &Path, a: OpenAgent) {
    swallow(
        with_locked(root, "agents", |cur| {
            let mut v: Vec<OpenAgent> = parse_lines(&cur);
            v.push(a);
            (Some(to_lines(&v)), ())
        }),
        "push_agent",
    );
}
pub fn pop_agent(root: &Path, agent_id: Option<&str>) -> Option<OpenAgent> {
    swallow(
        with_locked(root, "agents", |cur| {
            let mut v: Vec<OpenAgent> = parse_lines(&cur);
            let idx = match agent_id {
                Some(id) => v.iter().rposition(|a| a.agent_id == id),
                None => {
                    if v.is_empty() {
                        None
                    } else {
                        Some(v.len() - 1)
                    }
                }
            };
            match idx {
                Some(i) => {
                    let a = v.remove(i);
                    (Some(to_lines(&v)), Some(a))
                }
                None => (None, None),
            }
        }),
        "pop_agent",
    )
    .flatten()
}

// ---- inflight ----
/// One in-flight tool call. A **multiset** keyed by `key`: two concurrent calls
/// with the same key are two lines, not one (spec §Correlation state).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Inflight {
    pub key: String,
    pub tool: String,
    pub ts: u64,
    pub agent_id: Option<String>,
    pub head_before: Option<String>,
    /// Why this call has no HEAD baseline, when it has none: `"timeout"` (the
    /// pre-time `git rev-parse` timed out). Copied onto the record, or named
    /// on stderr when nothing is recorded, so the miss is auditable rather
    /// than silent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detection: Option<String>,
}

fn visible(e: &Inflight, scope: Option<&str>) -> bool {
    e.agent_id.is_none() || e.agent_id.as_deref() == scope
}
fn live(e: &Inflight, now: u64) -> bool {
    now.saturating_sub(e.ts) <= INFLIGHT_TTL_SECS
}

/// Append. Never de-duplicates by key: two concurrent calls sharing a key must
/// push two lines, or the second pre-check erases the first's `head_before` and
/// the first post-check finds nothing.
pub fn push_inflight(root: &Path, e: Inflight) {
    swallow(
        with_locked(root, "inflight", |cur| {
            let mut v: Vec<Inflight> = parse_lines(&cur);
            v.push(e);
            (Some(to_lines(&v)), ())
        }),
        "push_inflight",
    );
}

/// Remove the **last** line with a matching key and return it, **regardless of
/// age**: the TTL is a classification rule, not a retention rule, and a
/// twenty-minute build must still get its commit detected.
pub fn pop_inflight(root: &Path, key: &str) -> Option<Inflight> {
    swallow(
        with_locked(root, "inflight", |cur| {
            let mut v: Vec<Inflight> = parse_lines(&cur);
            match v.iter().rposition(|x| x.key == key) {
                Some(i) => {
                    let e = v.remove(i);
                    (Some(to_lines(&v)), Some(e))
                }
                None => (None, None),
            }
        }),
        "pop_inflight",
    )
    .flatten()
}

/// Drop every entry. Every path that records an `interrupt` calls this, so one
/// Esc cannot yield two interrupts and a lingering entry cannot fake a third
/// for the next 900 s (spec §Classification step 2, §"Host adapters / Codex").
pub fn clear_inflight(root: &Path) {
    swallow(
        with_locked(root, "inflight", |_| (Some(String::new()), ())),
        "clear_inflight",
    );
}
pub fn live_inflight(root: &Path, now: u64, agent_scope: Option<&str>) -> Vec<Inflight> {
    swallow(
        with_locked(root, "inflight", |cur| {
            let all: Vec<Inflight> = parse_lines(&cur);
            let kept: Vec<Inflight> = all.iter().filter(|e| live(e, now)).cloned().collect();
            let out: Vec<Inflight> = kept
                .iter()
                .filter(|e| visible(e, agent_scope))
                .cloned()
                .collect();
            let changed = kept.len() != all.len();
            (changed.then(|| to_lines(&kept)), out)
        }),
        "live_inflight",
    )
    .unwrap_or_default()
}
pub fn take_inflight_for_scope(root: &Path, now: u64, agent_scope: Option<&str>) -> Vec<Inflight> {
    swallow(
        with_locked(root, "inflight", |cur| {
            let all: Vec<Inflight> = parse_lines(&cur);
            let (taken, kept): (Vec<Inflight>, Vec<Inflight>) = all
                .into_iter()
                .filter(|e| live(e, now))
                .partition(|e| visible(e, agent_scope));
            (Some(to_lines(&kept)), taken)
        }),
        "take_inflight_for_scope",
    )
    .unwrap_or_default()
}
/// FNV-1a, 64-bit, inline. **Not** `std::hash::DefaultHasher`: its algorithm is
/// explicitly unspecified across Rust releases, so a pre/post pair split across
/// a binary upgrade would stop matching and leak an entry that then fakes an
/// interrupt for 900 s. Inline rather than a dependency: the whole algorithm is
/// four lines and the constants are part of the on-disk contract.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// `tool_use_id` when the host supplies one (Claude, Codex); otherwise the hex
/// of FNV-1a over `tool_name` followed by the canonical `tool_input` JSON
/// (Gemini, whose `BeforeTool`/`AfterTool` carry identical `tool_input`).
pub fn inflight_key_for(
    tool_use_id: Option<&str>,
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> String {
    if let Some(id) = tool_use_id.filter(|s| !s.is_empty()) {
        return id.to_string();
    }
    // `serde_json::Value::Object` is a BTreeMap unless the `preserve_order`
    // feature is on, so `to_string` already emits keys in sorted order. This
    // crate does not enable it; the sorted-key canonicalization the spec asks
    // for is therefore what `to_string` produces.
    let canon = serde_json::to_string(tool_input).unwrap_or_default();
    let mut bytes = tool_name.as_bytes().to_vec();
    bytes.extend_from_slice(canon.as_bytes());
    format!("h{:016x}", fnv1a_64(&bytes))
}

// ---- turn ----
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Turn {
    /// The session that opened this turn. Carried so a turn left open by a
    /// crashed session cannot leak into the next one: a reader whose
    /// `current_sid` differs treats the file as absent.
    #[serde(default)]
    pub sid: String,
    pub open: bool,
    pub turn_id: Option<String>,
    pub last_prompt_ts: u64,
    /// `"prompt"` | `"stop"` | `"interrupt"`, or empty when absent. This is the
    /// single interrupt-already-recorded signal the classifier reads (spec
    /// §Classification step 1) — never a journal scan, so a concurrent
    /// `SubagentStop` record cannot hide it and compaction cannot erase it.
    pub last_event: String,
}

/// Read the turn for the **current** session. A missing, unparseable or
/// foreign-`sid` file reads as `Turn::default()` — absent, i.e. closed, i.e.
/// the next prompt is `fresh`. That is the conservative answer: a false `fresh`
/// undercounts an intervention, a false `open` invents one.
pub fn read_turn(root: &Path) -> Turn {
    let sid = crate::journey::current_sid(root);
    std::fs::read_to_string(dir(root).join("turn"))
        .ok()
        .and_then(|s| serde_json::from_str::<Turn>(&s).ok())
        .filter(|t| t.sid == sid)
        .unwrap_or_default()
}

/// Open the turn. Called only for a **top-level** prompt: a prompt carrying an
/// `agent_id` never writes this file, or a sub-agent's prompt would move the
/// parent's turn state and its `last_prompt_ts` (spec §Correlation state).
pub fn open_turn(root: &Path, turn_id: Option<&str>, ts: u64) {
    let t = Turn {
        sid: crate::journey::current_sid(root),
        open: true,
        turn_id: turn_id.map(str::to_string),
        last_prompt_ts: ts,
        last_event: "prompt".into(),
    };
    swallow(
        with_locked(root, "turn", |_| (serde_json::to_string(&t).ok(), ())),
        "open_turn",
    );
}

/// Close the turn and record what closed it. Returns `false` when the write
/// failed, so the caller can degrade to the conservative answer: a failed close
/// leaves `open: true` on disk, and the classifier would otherwise read a
/// finished turn as running and report a false intervention.
pub fn close_turn(root: &Path, last_event: &str) -> bool {
    let sid = crate::journey::current_sid(root);
    swallow(
        with_locked(root, "turn", |cur| {
            let mut t: Turn = serde_json::from_str(&cur).unwrap_or_default();
            if t.sid != sid {
                t = Turn::default();
            }
            t.sid = sid.clone();
            t.open = false;
            t.last_event = last_event.to_string();
            (serde_json::to_string(&t).ok(), ())
        }),
        "close_turn",
    )
    .is_some()
}

// ---- kalpa ----
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Kalpa {
    pub name: String,
    pub started_ts: u64,
}

/// The name pattern is a property of the value, not of the CLI: a stale or
/// hand-edited file whose `name` fails it reads as absent (with one stderr
/// warning), so it can never reach a journal tag, a log field, or the
/// model-visible context header (spec §"Naming the kalpa").
pub fn read_kalpa(root: &Path) -> Option<Kalpa> {
    let k: Kalpa = std::fs::read_to_string(dir(root).join("kalpa"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())?;
    if !valid_kalpa_name(&k.name) {
        eprintln!(
            "phronesis: ignoring .phronesis/journey/kalpa: name does not match [a-z0-9][a-z0-9-]{{0,63}}"
        );
        return None;
    }
    Some(k)
}
pub fn write_kalpa(root: &Path, k: &Kalpa) {
    swallow(
        with_locked(root, "kalpa", |_| (serde_json::to_string(k).ok(), ())),
        "write_kalpa",
    );
}
pub fn clear_kalpa(root: &Path) {
    let _ = std::fs::remove_file(dir(root).join("kalpa"));
}
/// `^[a-z0-9][a-z0-9-]{0,63}$`. Written with explicit early returns rather
/// than a chained boolean: `&&` binds tighter than `||`, and the obvious
/// one-expression form silently accepts `-lead`.
pub fn valid_kalpa_name(name: &str) -> bool {
    let b = name.as_bytes();
    if !(1..=64).contains(&b.len()) {
        return false;
    }
    let first_ok = b[0].is_ascii_lowercase() || b[0].is_ascii_digit();
    first_ok
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

// ---- classification ----
use crate::lifecycle::event::{Host, Mode};

pub struct PromptContext<'a> {
    pub host: Host,
    pub now: u64,
    /// `Some` when this prompt was delivered **inside** a sub-agent. Such a
    /// prompt is not the human speaking: it classifies `fresh` and touches no
    /// state at all.
    pub agent_id: Option<&'a str>,
    pub turn_id: Option<&'a str>,
    pub transcript_path: Option<&'a Path>,
}

/// How an interrupt was inferred, for the log's `inferred_from`. No `Hook`
/// variant: the Codex `Interrupt` hook writes its own record and sets
/// `turn.last_event`, which step 1 reads without inferring anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptSource {
    Inflight,
    Transcript,
    OpenTurn,
}
impl InterruptSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inflight => "inflight",
            Self::Transcript => "transcript",
            Self::OpenTurn => "open_turn",
        }
    }
}

/// `interrupt: Some(source)` means the caller must write an `interrupt` record
/// with that `inferred_from` before the prompt record. `interrupt: None` with
/// `mode: Correction` means the record already exists (step 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub mode: Mode,
    pub interrupt: Option<InterruptSource>,
}

/// Spec §"Classification at prompt time", in the spec's own order.
pub fn classify_prompt(root: &Path, ctx: &PromptContext<'_>) -> Classification {
    // A prompt carrying an agent_id is a sub-agent's prompt, not the human's.
    // It reads no state and writes none.
    if ctx.agent_id.is_some() {
        return Classification {
            mode: Mode::Fresh,
            interrupt: None,
        };
    }

    // Step 1. `read_turn` already treats a missing, unparseable or foreign-sid
    // file as absent, which is `Turn::default()`: closed, last_event empty.
    let turn = read_turn(root);
    if turn.last_event == "interrupt" {
        // The single interrupt-already-recorded path, on every host. Read from
        // `turn` and not from the journal tail, so a concurrent SubagentStop
        // record cannot hide it and compaction cannot erase it.
        return Classification {
            mode: Mode::Correction,
            interrupt: None,
        };
    }
    if !turn.open {
        return Classification {
            mode: Mode::Fresh,
            interrupt: None,
        };
    }

    // Step 2. Look for evidence that the human aborted the running turn.
    match detect_interrupt(root, ctx) {
        Some(source) => Classification {
            mode: Mode::Correction,
            interrupt: Some(source),
        },
        // Step 3. Reachable on Claude and Codex only; the Gemini branch inside
        // `detect_interrupt` always fires for an open turn.
        None => Classification {
            mode: Mode::MidTurn,
            interrupt: None,
        },
    }
}

/// Step 2's evidence check, in the spec's order, stopping at the first hit.
/// Every hit drops the session's `inflight` entries, so one Esc cannot yield
/// two interrupts and a lingering entry cannot fake a third.
///
/// Exposed on its own because `SessionEnd` runs the same check to decide
/// whether a quit out of an aborted turn is an `interrupt` or a `stop`.
pub fn detect_interrupt(root: &Path, ctx: &PromptContext<'_>) -> Option<InterruptSource> {
    detect_interrupt_inner(root, ctx, true)
}

/// The same evidence check for `SessionEnd`, where the Gemini open-turn
/// heuristic must not fire: a session that exits with a turn still open is a
/// normal quit, not an abort, so only transcript and inflight evidence count.
/// Callers pass the real host; nothing here is faked to skip a branch.
pub fn detect_interrupt_at_session_end(
    root: &Path,
    ctx: &PromptContext<'_>,
) -> Option<InterruptSource> {
    detect_interrupt_inner(root, ctx, false)
}

fn detect_interrupt_inner(
    root: &Path,
    ctx: &PromptContext<'_>,
    allow_open_turn: bool,
) -> Option<InterruptSource> {
    let turn = read_turn(root);

    // Transcript marker (Claude only), FIRST: it is direct evidence. A queued
    // mid-turn message also leaves a live `inflight` entry, and only the marker
    // distinguishes the two.
    let source = if ctx.host == Host::Claude
        && let Some(p) = ctx.transcript_path
        && transcript_has_interrupt_after(p, turn.last_prompt_ts)
    {
        Some(InterruptSource::Transcript)
    // Inflight: a tool was still running when the human spoke. Not used on
    // Codex, where the `Interrupt` hook is authoritative and step 1 has already
    // spoken. `live_inflight` reads without consuming, so a `mid_turn` outcome
    // leaves a running tool's entry in place for its own post-check.
    } else if ctx.host != Host::Codex && !live_inflight(root, ctx.now, ctx.agent_id).is_empty() {
        Some(InterruptSource::Inflight)
    // Open turn (Gemini only): Gemini never delivers a prompt while a turn is
    // running, so an open turn here means AfterAgent was skipped, which only
    // happens on abort.
    } else if allow_open_turn && ctx.host == Host::Gemini && turn.open {
        Some(InterruptSource::OpenTurn)
    } else {
        None
    };

    if source.is_some() {
        clear_inflight(root);
    }
    source
}

const TRANSCRIPT_TAIL_BYTES: u64 = 64 * 1024;
const INTERRUPT_MARKER: &str = "[Request interrupted by user";

fn transcript_has_interrupt_after(path: &Path, after_ts: u64) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TRANSCRIPT_TAIL_BYTES);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return false;
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return false;
    }
    // Lossy rather than `read_to_string`: a 64 KiB seek can land mid-codepoint,
    // and a transcript that happens to contain one must not disable the branch.
    let buf = String::from_utf8_lossy(&buf);
    // Skip the first line when the read was seeked: it is probably a fragment.
    buf.lines().skip(usize::from(start > 0)).any(|line| {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return false;
        };
        let is_user = v.get("type").and_then(|t| t.as_str()) == Some("user")
            || v.pointer("/message/role").and_then(|r| r.as_str()) == Some("user");
        if !is_user {
            return false;
        }
        let text = match v.pointer("/message/content").or_else(|| v.get("content")) {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .filter_map(|i| i.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        if !text.trim_start().starts_with(INTERRUPT_MARKER) {
            return false;
        }
        match v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        {
            Some(dt) => dt.timestamp() as u64 > after_ts,
            None => true,
        }
    })
}
