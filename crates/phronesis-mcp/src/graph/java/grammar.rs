//! Java grammar with qualified record patterns supported.

unsafe extern "C" {
    fn tree_sitter_java() -> *const ();
}

/// Load the bundled parser generated from the documented upstream grammar.
pub fn language() -> tree_sitter::Language {
    // SAFETY: build.rs links the generated Tree-sitter C parser whose exported
    // function returns the static TSLanguage expected by LanguageFn.
    let function = unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_java) };
    function.into()
}
