//! Tree-sitter Rust analyzer. Each predicate has its own private extractor
//! function taking `&ParsedFile`; `extract()` parses once and runs them all.

mod assertions;
mod counts;
mod derives;
mod docs;
mod eval;
mod hazards;
mod signatures;
mod walk;

use super::facts::SyntaxFacts;
use super::parsed::ParsedFile;

/// Top-level entry. Parses once, then runs every predicate extractor.
pub fn extract(content: &str) -> SyntaxFacts {
    let Some(parsed) = ParsedFile::parse_rust(content) else {
        return SyntaxFacts::default();
    };
    let function_param_types = signatures::extract_function_param_types(&parsed);
    let vec_ref_params = function_param_types
        .iter()
        .filter(|(_, _, ty)| ty.starts_with("&Vec<") || ty.starts_with("&mut Vec<"))
        .map(|(fn_name, param, _)| (fn_name.clone(), param.clone()))
        .collect();
    const PARAM_COUNT_THRESHOLD: usize = 5;
    let function_param_counts_high =
        signatures::extract_function_param_counts_high(&parsed, PARAM_COUNT_THRESHOLD);
    SyntaxFacts {
        functions_returning_result_string: signatures::extract_result_string_returns(&parsed),
        public_functions: signatures::extract_public_functions(&parsed),
        async_functions: signatures::extract_async_functions(&parsed),
        function_param_types,
        vec_ref_params,
        function_param_counts_high,
        function_clone_counts: counts::extract_function_clone_counts(&parsed),
        function_clone_counts_high: counts::extract_function_clone_counts_high(&parsed),
        function_let_binding_counts_high: counts::extract_function_let_binding_counts_high(&parsed),
        function_let_mut_counts_high: counts::extract_function_let_mut_counts_high(&parsed),
        pub_fns_without_doc_comment: docs::extract_pub_fns_without_doc_comment(&parsed),
        tests_without_assertion: assertions::extract_tests_without_assertion(&parsed),
        struct_derives: derives::extract_struct_derives(&parsed),
        engine_eval_string_literals: eval::extract_engine_eval_string_literals(&parsed),
        unsafe_blocks_without_safety_comment: hazards::extract_unsafe_without_safety(&parsed),
        async_blocking_calls: hazards::extract_async_blocking_calls(&parsed),
        sync_lock_guards_across_await: hazards::extract_sync_lock_guards_across_await(&parsed),
        ..SyntaxFacts::default()
    }
}
