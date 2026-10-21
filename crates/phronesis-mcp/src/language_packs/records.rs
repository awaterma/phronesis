//! The per-file record vocabulary adapters produce, plus the host-side input
//! they receive. Record validation lives here, not in individual adapters.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{PackError, is_identifier, limits, relative_path_problem};

/// One record-contract violation, before it is attributed to a pack/file.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct RecordError(pub String);

/// Half-open UTF-8 byte range with one-based lines; `end_line` is the line
/// holding `end_byte` (tree-sitter's `end_position().row + 1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: u32,
    pub end_line: u32,
}

impl SourceSpan {
    /// The span of `source[start..end]`, or `None` when the range is out of
    /// bounds, reversed, or not on character boundaries.
    pub fn from_bytes(source: &str, start: usize, end: usize) -> Option<Self> {
        if start > end || !source.is_char_boundary(start) || !source.is_char_boundary(end) {
            return None;
        }
        let start_line = newlines(&source.as_bytes()[..start])?.checked_add(1)?;
        let inner = newlines(&source.as_bytes()[start..end])?;
        Some(Self {
            start_byte: start,
            end_byte: end,
            start_line,
            end_line: start_line.checked_add(inner)?,
        })
    }

    /// Verify bytes and lines correspond to `source`.
    pub fn check(&self, source: &str) -> Result<(), RecordError> {
        match Self::from_bytes(source, self.start_byte, self.end_byte) {
            Some(expected) if expected == *self => Ok(()),
            Some(expected) => Err(RecordError(format!(
                "span {}..{} has lines {}..{}, source has {}..{}",
                self.start_byte,
                self.end_byte,
                self.start_line,
                self.end_line,
                expected.start_line,
                expected.end_line
            ))),
            None => Err(RecordError(format!(
                "span {}..{} is not a valid range of the {}-byte source",
                self.start_byte,
                self.end_byte,
                source.len()
            ))),
        }
    }
}

fn newlines(bytes: &[u8]) -> Option<u32> {
    u32::try_from(bytes.iter().filter(|&&b| b == b'\n').count()).ok()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub name: String,
    pub kind: String,
    pub text: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryMatch {
    pub query: String,
    pub captures: Vec<Capture>,
}

/// The nearest manifest-declared context file between a source file's parent
/// and the repository root. Paths are host-owned and repository-relative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextFile {
    pub name: String,
    pub file_rel: String,
    pub directory_rel: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileContext {
    pub file_rel: String,
    pub context_files: Vec<ContextFile>,
}

/// A test invocation rendered for the user; pack processing never runs it.
/// `cwd` is a repository-relative directory (`.` for the root).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceMode {
    /// Eligible for cross-file resolution against unique free declarations.
    SameScope,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PackRecord {
    Declaration {
        name: String,
        scope: String,
        #[serde(default)]
        receiver: Option<String>,
        span: SourceSpan,
        #[serde(default)]
        test: bool,
        #[serde(default)]
        command: Option<TestCommand>,
    },
    Reference {
        owner_start_byte: usize,
        target: String,
        scope: String,
        resolution: ReferenceMode,
        span: SourceSpan,
    },
    /// A written dependency, not evidence of a resolved callable target.
    Import {
        scope: String,
        target: String,
        #[serde(default)]
        alias: Option<String>,
        span: SourceSpan,
    },
    /// `name` is local; the host prepends `pack_<id>_` and the file argument.
    Fact {
        name: String,
        args: Vec<String>,
        span: SourceSpan,
    },
}

impl PackRecord {
    pub fn span(&self) -> &SourceSpan {
        match self {
            Self::Declaration { span, .. }
            | Self::Reference { span, .. }
            | Self::Import { span, .. }
            | Self::Fact { span, .. } => span,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Declaration { .. } => "declaration",
            Self::Reference { .. } => "reference",
            Self::Import { .. } => "import",
            Self::Fact { .. } => "fact",
        }
    }
}

/// Record kinds a query template may emit.
pub const RECORD_KINDS: [&str; 4] = ["declaration", "reference", "import", "fact"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extraction {
    pub records: Vec<PackRecord>,
    pub diagnostics: Vec<PackDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackDiagnostic {
    pub pack: Option<String>,
    pub path: Option<String>,
    pub stage: String,
    pub message: String,
}

/// Check one file's records against the record contract and its source:
/// spans correspond to `source`, references name a declaration in this
/// file, commands belong to tests, and caps hold.
pub fn validate_records(
    pack: &str,
    file_rel: &str,
    records: &[PackRecord],
    source: &str,
) -> Result<(), PackError> {
    let fail = |message: String| PackError::Record {
        pack: Some(pack.to_string()),
        path: Some(file_rel.to_string()),
        message,
    };
    if records.len() > limits::MAX_OUTPUT_RECORDS {
        return Err(PackError::Limit {
            pack: Some(pack.to_string()),
            path: Some(file_rel.to_string()),
            what: "output records".to_string(),
            max: limits::MAX_OUTPUT_RECORDS,
        });
    }
    let owners: HashSet<usize> = records
        .iter()
        .filter_map(|r| match r {
            PackRecord::Declaration { span, .. } => Some(span.start_byte),
            _ => None,
        })
        .collect();
    for (index, record) in records.iter().enumerate() {
        check_record(record, source, &owners)
            .map_err(|e| fail(format!("record {index} ({}): {e}", record.kind())))?;
    }
    Ok(())
}

fn check_record(
    record: &PackRecord,
    source: &str,
    owners: &HashSet<usize>,
) -> Result<(), RecordError> {
    record.span().check(source)?;
    match record {
        PackRecord::Declaration {
            name,
            scope,
            receiver,
            test,
            command,
            ..
        } => {
            non_empty("name", name)?;
            no_nul("scope", scope)?;
            if let Some(receiver) = receiver {
                non_empty("receiver", receiver)?;
            }
            match command {
                Some(_) if !test => Err(RecordError("a command requires `test: true`".to_string())),
                Some(command) => check_command(command),
                None => Ok(()),
            }
        }
        PackRecord::Reference {
            owner_start_byte,
            target,
            scope,
            ..
        } => {
            non_empty("target", target)?;
            no_nul("scope", scope)?;
            if owners.contains(owner_start_byte) {
                Ok(())
            } else {
                Err(RecordError(format!(
                    "owner_start_byte {owner_start_byte} matches no declaration in this file"
                )))
            }
        }
        PackRecord::Import {
            scope,
            target,
            alias,
            ..
        } => {
            non_empty("target", target)?;
            no_nul("scope", scope)?;
            alias.as_deref().map_or(Ok(()), |a| non_empty("alias", a))
        }
        PackRecord::Fact { name, args, .. } => {
            if !is_identifier(name) {
                return Err(RecordError(format!(
                    "fact name `{name}` must match [a-z][a-z0-9_]{{0,47}}"
                )));
            }
            if args.len() > limits::MAX_FACT_ARGS {
                return Err(RecordError(format!(
                    "{} fact arguments exceed the limit of {}",
                    args.len(),
                    limits::MAX_FACT_ARGS
                )));
            }
            if args.iter().any(|a| a.len() > limits::MAX_FACT_ARG_BYTES) {
                return Err(RecordError(format!(
                    "a fact argument exceeds {} bytes",
                    limits::MAX_FACT_ARG_BYTES
                )));
            }
            args.iter().try_for_each(|a| no_nul("args", a))
        }
    }
}

fn check_command(command: &TestCommand) -> Result<(), RecordError> {
    non_empty("command.program", &command.program)?;
    command
        .args
        .iter()
        .try_for_each(|a| no_nul("command.args", a))?;
    if command.cwd == "." {
        return Ok(());
    }
    match relative_path_problem(&command.cwd) {
        Some(problem) => Err(RecordError(format!(
            "command.cwd `{}` {problem}",
            command.cwd
        ))),
        None => Ok(()),
    }
}

fn non_empty(field: &str, value: &str) -> Result<(), RecordError> {
    if value.is_empty() {
        return Err(RecordError(format!("`{field}` is empty")));
    }
    no_nul(field, value)
}

fn no_nul(field: &str, value: &str) -> Result<(), RecordError> {
    if value.contains('\0') {
        return Err(RecordError(format!("`{field}` contains a NUL byte")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SRC: &str = "fn a() {}\nfn é() {\n}\n";

    fn span(start: usize, end: usize) -> SourceSpan {
        SourceSpan::from_bytes(SRC, start, end).unwrap()
    }

    fn decl(start: usize, end: usize) -> PackRecord {
        PackRecord::Declaration {
            name: "a".into(),
            scope: "".into(),
            receiver: None,
            span: span(start, end),
            test: false,
            command: None,
        }
    }

    fn validate(records: &[PackRecord]) -> Result<(), PackError> {
        validate_records("demo", "a.demo", records, SRC)
    }

    #[test]
    fn spans_follow_source_lines_and_char_boundaries() {
        assert_eq!((span(0, 9).start_line, span(0, 9).end_line), (1, 1));
        let second = span(10, 21);
        assert_eq!((second.start_line, second.end_line), (2, 3));
        // A span ending after a newline ends on the next line, as in tree-sitter.
        assert_eq!(span(10, 22).end_line, 4);
        // `é` is two bytes starting at 13.
        assert!(SourceSpan::from_bytes(SRC, 14, 15).is_none());
        assert!(SourceSpan::from_bytes(SRC, 5, 4).is_none());
        assert!(SourceSpan::from_bytes(SRC, 0, SRC.len() + 1).is_none());
        let mut lying = span(0, 9);
        lying.end_line = 2;
        assert!(lying.check(SRC).unwrap_err().0.contains("lines"));
    }

    #[test]
    fn records_round_trip_and_reject_unknown_fields() {
        let record = decl(0, 9);
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value["kind"], "declaration");
        assert_eq!(serde_json::from_value::<PackRecord>(value).unwrap(), record);

        let mut forged = serde_json::to_value(decl(0, 9)).unwrap();
        forged["provenance"] = json!("rule_firing");
        assert!(serde_json::from_value::<PackRecord>(forged).is_err());
        let edge = json!({"kind": "edge", "span": span(0, 9)});
        assert!(serde_json::from_value::<PackRecord>(edge).is_err());
        let reference = json!({"kind": "reference", "owner_start_byte": 0, "target": "b",
            "scope": "", "resolution": "resolved", "span": span(0, 9)});
        assert!(serde_json::from_value::<PackRecord>(reference).is_err());
    }

    #[test]
    fn record_validation_rejects_contract_violations() {
        validate(&[decl(0, 9)]).unwrap();

        let reference = |owner| PackRecord::Reference {
            owner_start_byte: owner,
            target: "b".into(),
            scope: "".into(),
            resolution: ReferenceMode::SameScope,
            span: span(7, 9),
        };
        validate(&[decl(0, 9), reference(0)]).unwrap();
        let err = validate(&[decl(0, 9), reference(3)]).unwrap_err();
        assert_eq!((err.pack(), err.path()), (Some("demo"), Some("a.demo")));
        assert!(err.to_string().contains("owner_start_byte 3"), "{err}");

        let with_command = |test, cwd: &str| PackRecord::Declaration {
            name: "TestA".into(),
            scope: "".into(),
            receiver: None,
            span: span(0, 9),
            test,
            command: Some(TestCommand {
                program: "go".into(),
                args: vec!["test".into()],
                cwd: cwd.into(),
            }),
        };
        validate(&[with_command(true, ".")]).unwrap();
        validate(&[with_command(true, "pkg/sub")]).unwrap();
        assert!(validate(&[with_command(false, ".")]).is_err());
        for cwd in [
            "../x",
            "/tmp",
            "",
            "a/../b",
            "C:/x",
            "C:x",
            "pkg/C:/x",
            "pkg:stream",
        ] {
            assert!(validate(&[with_command(true, cwd)]).is_err(), "{cwd}");
        }

        let fact = |name: &str, args: Vec<String>| PackRecord::Fact {
            name: name.into(),
            args,
            span: span(0, 9),
        };
        validate(&[fact("dot_import", vec!["x".into()])]).unwrap();
        assert!(validate(&[fact("Pack-X", vec![])]).is_err());
        assert!(validate(&[fact("f", vec![String::new(); 33])]).is_err());
        assert!(validate(&[fact("f", vec!["x".repeat(4097)])]).is_err());

        let mut bad_span = decl(0, 9);
        if let PackRecord::Declaration { span, .. } = &mut bad_span {
            span.end_byte = 14;
        }
        assert!(validate(&[bad_span]).is_err());

        let many = vec![decl(0, 9); limits::MAX_OUTPUT_RECORDS + 1];
        assert_eq!(validate(&many).unwrap_err().stage(), "limit");
    }
}
