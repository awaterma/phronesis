//! Typed language-pack failures. Every variant names the pack (when known),
//! the asset path or manifest field, and the stage that failed.

use std::io;

use thiserror::Error;

use super::records::PackDiagnostic;

#[derive(Debug, Error)]
pub enum PackError {
    #[error("{}: cannot read `{path}`: {source}", label(.pack))]
    Io {
        pack: Option<String>,
        path: String,
        #[source]
        source: io::Error,
    },
    /// Malformed JSON, an unknown field, or a field with an invalid value.
    #[error("{}: `{path}`: invalid `{field}`: {message}", label(.pack))]
    Manifest {
        pack: Option<String>,
        path: String,
        field: String,
        message: String,
    },
    #[error("{}: unsafe resource path `{path}`: {reason}", label(.pack))]
    UnsafePath {
        pack: Option<String>,
        path: String,
        reason: String,
    },
    #[error("{}: duplicate `{field}` value `{value}`", label(.pack))]
    Duplicate {
        pack: Option<String>,
        field: String,
        value: String,
    },
    #[error("{}: {}{what} exceeds the limit of {max}", label(.pack), at(.path))]
    Limit {
        pack: Option<String>,
        path: Option<String>,
        what: String,
        max: usize,
    },
    #[error(
        "{}: unknown grammar `{grammar}`; available grammars: {}",
        label(.pack),
        .available.join(", ")
    )]
    UnknownGrammar {
        pack: Option<String>,
        grammar: String,
        available: Vec<String>,
    },
    #[error("{}: ownership conflict: {message}", label(.pack))]
    Ownership {
        pack: Option<String>,
        message: String,
    },
    #[error("{}: {}query error: {message}", label(.pack), at(.path))]
    Query {
        pack: Option<String>,
        path: Option<String>,
        message: String,
    },
    #[error("{}: {}script error: {message}", label(.pack), at(.path))]
    Script {
        pack: Option<String>,
        path: Option<String>,
        message: String,
    },
    /// An adapter produced a record that violates the record schema.
    #[error("{}: {}invalid record: {message}", label(.pack), at(.path))]
    Record {
        pack: Option<String>,
        path: Option<String>,
        message: String,
    },
    #[error("{}: unsupported: {message}", label(.pack))]
    Unsupported {
        pack: Option<String>,
        message: String,
    },
}

fn label(pack: &Option<String>) -> String {
    match pack {
        Some(id) => format!("language pack `{id}`"),
        None => "language pack".to_string(),
    }
}

fn at(path: &Option<String>) -> String {
    path.as_ref()
        .map(|p| format!("`{p}`: "))
        .unwrap_or_default()
}

impl PackError {
    pub fn pack(&self) -> Option<&str> {
        match self {
            Self::Io { pack, .. }
            | Self::Manifest { pack, .. }
            | Self::UnsafePath { pack, .. }
            | Self::Duplicate { pack, .. }
            | Self::Limit { pack, .. }
            | Self::UnknownGrammar { pack, .. }
            | Self::Ownership { pack, .. }
            | Self::Query { pack, .. }
            | Self::Script { pack, .. }
            | Self::Record { pack, .. }
            | Self::Unsupported { pack, .. } => pack.as_deref(),
        }
    }

    /// The pack-relative asset path involved, when there is one.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Io { path, .. } | Self::Manifest { path, .. } | Self::UnsafePath { path, .. } => {
                Some(path)
            }
            Self::Limit { path, .. }
            | Self::Query { path, .. }
            | Self::Script { path, .. }
            | Self::Record { path, .. } => path.as_deref(),
            Self::Duplicate { .. }
            | Self::UnknownGrammar { .. }
            | Self::Ownership { .. }
            | Self::Unsupported { .. } => None,
        }
    }

    pub fn stage(&self) -> &'static str {
        match self {
            Self::Io { .. } => "read",
            Self::Manifest { .. } | Self::Duplicate { .. } => "manifest",
            Self::UnsafePath { .. } => "path",
            Self::Limit { .. } => "limit",
            Self::UnknownGrammar { .. } => "grammar",
            Self::Ownership { .. } => "ownership",
            Self::Query { .. } => "query",
            Self::Script { .. } => "script",
            Self::Record { .. } => "record",
            Self::Unsupported { .. } => "unsupported",
        }
    }

    pub fn to_diagnostic(&self) -> PackDiagnostic {
        PackDiagnostic {
            pack: self.pack().map(str::to_string),
            path: self.path().map(str::to_string),
            stage: self.stage().to_string(),
            message: self.to_string(),
        }
    }
}
