//! Syntax-aware fact extraction. Unlike `diff_extract` (regex-based, surfaces
//! function names and imports), this module parses source files with
//! tree-sitter and produces facts about *structural properties* — return
//! types, parameter types, function bodies. The hook asserts these facts so
//! rules can reason about the AST shape, not just text patterns.

pub mod facts;
pub mod parsed;
pub mod rust;
pub mod swift;

pub use facts::SyntaxFacts;

/// Dispatch by file extension; returns `SyntaxFacts::default()` for unsupported
/// languages or when parsing fails.
pub fn extract(file_path: &str, content: &str) -> SyntaxFacts {
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "rs" => rust::extract(content),
        "swift" => swift::extract(content),
        _ => SyntaxFacts::default(),
    }
}
