//! Pure `emit_capsule` payload validation, extracted from the MCP capsule
//! module. Storage, capture, expiry, leases, and session handling stay in
//! MCP; this module holds only the value types and the exact validation the
//! strict rules loader applies to an `emit_capsule` action's structured
//! body, with identical message strings.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

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

/// Byte cap applied to capsule bodies when the action omits `max_bytes`;
/// matches `MAX_RECORD_BYTES` in the MCP capsule store.
pub const MAX_RECORD_BYTES: u64 = 8 * 1024;

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

/// A validated `emit_capsule` action payload. This is the pure spec part of
/// the MCP `EmittedCapsule`: identity, body, lifecycle, ordering, and
/// expiry. Emission-time provenance (session, bindings, timestamps) is
/// attached by the MCP storage layer.
#[derive(Debug, Clone, PartialEq)]
pub struct CapsuleSpec {
    /// Unique capsule ID (validated static string, no variable substitution).
    pub id: String,
    /// The body text (may contain variable substitutions).
    pub body: String,
    /// Lifecycle policy.
    pub lifecycle: CapsuleLifecycle,
    /// Priority for ordering (higher = more important).
    pub priority: i32,
    /// Body cap in bytes; defaults to [`MAX_RECORD_BYTES`].
    pub max_bytes: u64,
    /// Bounded relative expiry requested by the payload, if any. Emission
    /// time (absolute `expires_at`) is added by the storage layer.
    pub expires_after: Option<Duration>,
}

/// Validation failure for an `emit_capsule` payload. `Display` carries the
/// exact message strings the former loader produced (wrapped by callers
/// with the "Invalid capsule: " prefix the MCP error type added).
#[derive(Debug, Error, Clone, PartialEq)]
#[error("{0}")]
pub struct CapsulePayloadError(pub String);

/// Validate an `emit_capsule` action's structured payload and return the
/// normalized spec. Fields outside the known set are rejected
/// (`deny_unknown_fields` semantics), the ID must be a static string of at
/// most 128 ASCII identifier characters, the lifecycle must be one of
/// `next_interaction`/`session`/`persistent`, `max_bytes` must be within
/// `1..=[MAX_RECORD_BYTES]`, the body must fit it, and `expires_after`
/// must parse via [`parse_duration`].
pub fn validate_emit_capsule_payload(
    action_data: &serde_json::Value,
) -> Result<CapsuleSpec, CapsulePayloadError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RawSpec {
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

    let invalid = |msg: String| CapsulePayloadError(msg);
    let spec: RawSpec = serde_json::from_value(action_data.clone())
        .map_err(|e| invalid(format!("Invalid capsule spec: {}", e)))?;

    // Validate ID (static string, no variable substitution)
    if spec.id.is_empty()
        || spec.id.len() > 128
        || spec.id.contains('?')
        || !spec
            .id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(invalid(
            "Capsule ID must not contain variable substitutions".to_string(),
        ));
    }

    // Validate lifecycle
    let lifecycle = match spec.lifecycle.as_str() {
        "next_interaction" => CapsuleLifecycle::NextInteraction,
        "session" => CapsuleLifecycle::Session,
        "persistent" => CapsuleLifecycle::Persistent,
        other => {
            return Err(invalid(format!("Invalid lifecycle: {}", other)));
        }
    };

    // Validate body size
    let max_bytes = spec.max_bytes.unwrap_or(MAX_RECORD_BYTES);
    if max_bytes == 0 || max_bytes > MAX_RECORD_BYTES {
        return Err(invalid("max_bytes must be between 1 and 8192".to_string()));
    }
    if spec.body.len() as u64 > max_bytes {
        return Err(invalid(format!(
            "Capsule body exceeds max_bytes limit of {}",
            max_bytes
        )));
    }

    // Parse expiry. The former loader performed this arithmetic at
    // emission time (`now + secs`); validation used now = 0, so the
    // overflow branch is unreachable here, but the addition is kept for
    // exactness.
    let expires_after = match spec.expires_after.as_deref() {
        None => None,
        Some(raw) => Some(
            parse_duration(raw).ok_or_else(|| invalid(format!("invalid expires_after: {raw}")))?,
        ),
    };
    let _ = 0u64
        .checked_add(expires_after.map(|d| d.as_secs()).unwrap_or(0))
        .ok_or_else(|| invalid("expires_after overflows timestamp".to_string()))?;

    Ok(CapsuleSpec {
        id: spec.id,
        body: spec.body,
        lifecycle,
        priority: spec.priority,
        max_bytes,
        expires_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_valid_formats() {
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("10m"), Some(Duration::from_secs(600)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_duration("7d"), Some(Duration::from_secs(604800)));
    }

    #[test]
    fn parse_duration_invalid_formats() {
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("0d"), None);
        assert_eq!(parse_duration("7x"), None);
        assert_eq!(parse_duration("d"), None);
        assert_eq!(parse_duration("-5m"), None);
        assert_eq!(
            parse_duration("18446744073709551615d"),
            None,
            "checked_mul overflow"
        );
    }

    #[test]
    fn lifecycle_display_matches_disk_strings() {
        assert_eq!(
            CapsuleLifecycle::NextInteraction.to_string(),
            "next_interaction"
        );
        assert_eq!(CapsuleLifecycle::Session.to_string(), "session");
        assert_eq!(CapsuleLifecycle::Persistent.to_string(), "persistent");
    }

    #[test]
    fn validate_accepts_full_spec() {
        let value = serde_json::json!({
            "id": "review-layering", "body": "why: ?reason", "lifecycle": "session",
            "priority": 50, "max_bytes": 512, "expires_after": "7d"
        });
        let spec = validate_emit_capsule_payload(&value).unwrap();
        assert_eq!(spec.id, "review-layering");
        assert_eq!(spec.body, "why: ?reason");
        assert_eq!(spec.lifecycle, CapsuleLifecycle::Session);
        assert_eq!(spec.priority, 50);
        assert_eq!(spec.max_bytes, 512);
        assert_eq!(spec.expires_after, Some(Duration::from_secs(604800)));
    }

    #[test]
    fn validate_applies_defaults() {
        let spec = validate_emit_capsule_payload(&serde_json::json!({
            "id": "c-1", "body": "b", "lifecycle": "next_interaction"
        }))
        .unwrap();
        assert_eq!(spec.priority, 0);
        assert_eq!(spec.max_bytes, MAX_RECORD_BYTES);
        assert_eq!(spec.expires_after, None);
        assert_eq!(spec.lifecycle, CapsuleLifecycle::NextInteraction);
    }

    #[test]
    fn validate_rejects_unknown_fields_and_shapes() {
        for (value, needle) in [
            (
                serde_json::json!({"id":"x","body":"x","lifecycle":"session","unknown":true}),
                "unknown field",
            ),
            (
                serde_json::json!({"id":"x","body":"x"}),
                "Invalid capsule spec",
            ),
            (
                serde_json::json!({"id":"x","body":5,"lifecycle":"session"}),
                "Invalid capsule spec",
            ),
            (
                serde_json::json!({"id":"x","body":"x","lifecycle":"forever"}),
                "Invalid lifecycle: forever",
            ),
        ] {
            let err = validate_emit_capsule_payload(&value).unwrap_err();
            assert!(err.0.contains(needle), "{value}: {err}");
        }
    }

    #[test]
    fn validate_rejects_bad_ids() {
        for id in [
            "",
            "?dynamic",
            "has space",
            "slash/",
            "unicode-é",
            &"x".repeat(129),
        ] {
            let value = serde_json::json!({"id": id, "body": "x", "lifecycle": "session"});
            let err = validate_emit_capsule_payload(&value).unwrap_err();
            assert_eq!(
                err.0, "Capsule ID must not contain variable substitutions",
                "id {id:?}"
            );
        }
        // 128 chars is exactly the cap.
        let ok = serde_json::json!({"id": "x".repeat(128), "body": "x", "lifecycle": "session"});
        assert!(validate_emit_capsule_payload(&ok).is_ok());
    }

    #[test]
    fn validate_rejects_bad_body_and_max_bytes() {
        let err = validate_emit_capsule_payload(&serde_json::json!({
            "id":"x","body":"x","lifecycle":"session","max_bytes":0
        }))
        .unwrap_err();
        assert_eq!(err.0, "max_bytes must be between 1 and 8192");
        let err = validate_emit_capsule_payload(&serde_json::json!({
            "id":"x","body":"x","lifecycle":"session","max_bytes":8193
        }))
        .unwrap_err();
        assert_eq!(err.0, "max_bytes must be between 1 and 8192");
        let err = validate_emit_capsule_payload(&serde_json::json!({
            "id":"x","body":"y".repeat(8193),"lifecycle":"session"
        }))
        .unwrap_err();
        assert_eq!(
            err.0, "Capsule body exceeds max_bytes limit of 8192",
            "default cap applies"
        );
        let err = validate_emit_capsule_payload(&serde_json::json!({
            "id":"x","body":"y".repeat(513),"lifecycle":"session","max_bytes":512
        }))
        .unwrap_err();
        assert_eq!(err.0, "Capsule body exceeds max_bytes limit of 512");
        assert!(
            validate_emit_capsule_payload(&serde_json::json!({
                "id":"x","body":"y".repeat(512),"lifecycle":"session","max_bytes":512
            }))
            .is_ok()
        );
    }

    #[test]
    fn validate_rejects_bad_expires_after() {
        for raw in ["0d", "", "5w", "abc"] {
            let value = serde_json::json!({
                "id":"x","body":"x","lifecycle":"persistent","expires_after": raw
            });
            let err = validate_emit_capsule_payload(&value).unwrap_err();
            assert!(
                err.0.contains("invalid expires_after") || err.0 == "invalid expires_after: 0d",
                "{raw}: {err}"
            );
            assert!(err.0.contains(raw), "{raw}: {err}");
        }
    }

    #[test]
    fn error_display_is_the_inner_message_only() {
        let err = validate_emit_capsule_payload(&serde_json::json!({
            "id":"?x","body":"x","lifecycle":"session"
        }))
        .unwrap_err();
        // The "Invalid capsule: " prefix is applied by policy.rs / MCP, not here.
        assert_eq!(
            err.to_string(),
            "Capsule ID must not contain variable substitutions"
        );
    }

    #[test]
    fn validate_matches_parse_then_action_wrapping() {
        // The exact string the strict loader produces for a bad payload.
        let json = r#"{ "rules": [ { "id": "c", "when": [ { "p": true } ],
            "then": { "emit_capsule": { "id": "?dynamic", "body": "x", "lifecycle": "session" } } } ] }"#;
        let err = crate::parse(json, "t").unwrap_err();
        // Capsule payload failures are serde errors inside the rule
        // deserializer, so they surface as Malformed — matching the MCP
        // source where the whole rule validation runs inside from_str.
        match &err {
            crate::PolicyError::Malformed { message, .. } => {
                assert!(message.contains(
                    "rule `c`: Invalid capsule: Capsule ID must not contain variable substitutions"
                ), "{message}");
            }
            other => panic!("wrong kind: {other}"),
        }
    }
}
