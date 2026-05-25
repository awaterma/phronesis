//! Parse-once primitive shared across per-predicate extractors. Each language
//! variant carries the tree-sitter parse tree plus the original source bytes
//! (queries need both).

use std::sync::LazyLock;
use tree_sitter::{Parser, Tree};

static RUST_LANG: LazyLock<tree_sitter::Language> =
    LazyLock::new(|| tree_sitter_rust::LANGUAGE.into());

static SWIFT_LANG: LazyLock<tree_sitter::Language> =
    LazyLock::new(|| tree_sitter_swift::LANGUAGE.into());

pub enum ParsedFile {
    Rust { tree: Tree, source: String },
    Swift { tree: Tree, source: String },
}

impl ParsedFile {
    pub fn parse_rust(source: &str) -> Option<Self> {
        let mut parser = Parser::new();
        parser.set_language(&RUST_LANG).ok()?;
        let tree = parser.parse(source, None)?;
        Some(ParsedFile::Rust {
            tree,
            source: source.to_string(),
        })
    }

    pub fn parse_swift(source: &str) -> Option<Self> {
        let mut parser = Parser::new();
        parser.set_language(&SWIFT_LANG).ok()?;
        let tree = parser.parse(source, None)?;
        Some(ParsedFile::Swift {
            tree,
            source: source.to_string(),
        })
    }
}
