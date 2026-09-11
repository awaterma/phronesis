//! Java grammar supplied by the maintained Orchard dependency.

/// Load the published grammar, including qualified record pattern support.
pub fn language() -> tree_sitter::Language {
    tree_sitter_java_orchard::LANGUAGE.into()
}
