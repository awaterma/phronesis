use phronesis_mcp::coverage::region_map::{
    MAX_REGION_ID_BYTES, branch_region_id, changed_regions, extract_branch_sites,
    extract_function_sites, file_segment, function_region_id, is_qualified_region_id,
    reference_matches,
};

const OLD_SRC: &str = r#"pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {
    if denominator == 0 {
        return Err("division by zero");
    }

    Ok(numerator / denominator)
}
"#;
const NEW_SRC: &str = r#"pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {
    if denominator == 0 {
        return Err("invalid denominator");
    }

    Ok(numerator / denominator)
}
"#;

#[test]
fn test_branch_anchor_survives_message_edit() {
    let old = extract_branch_sites(OLD_SRC).unwrap();
    let new = extract_branch_sites(NEW_SRC).unwrap();
    assert_eq!(old.len(), 1);
    assert_eq!(old[0].anchor, new[0].anchor);
    let ch = changed_regions("src/lib.rs", OLD_SRC, NEW_SRC).unwrap();
    assert!(ch.branches.contains(&branch_region_id(
        "src/lib.rs",
        "safe_divide",
        &old[0].anchor,
        1
    )));
    assert_eq!(
        ch.branches,
        vec!["branch:src/lib.rs::safe_divide:cd6054b02dde".to_string()]
    );
    assert!(
        ch.functions
            .contains(&function_region_id("src/lib.rs", "safe_divide"))
    );
}

#[test]
fn test_untouched_function_not_reported() {
    let old = format!("{OLD_SRC}pub fn helper() -> i32 {{ 1 }}\n");
    let new = format!("{NEW_SRC}pub fn helper() -> i32 {{ 1 }}\n");
    let ch = changed_regions("src/lib.rs", &old, &new).unwrap();
    assert!(
        !ch.functions
            .contains(&function_region_id("src/lib.rs", "helper"))
    );
}

#[test]
fn test_branch_anchor_matches_committed_export() {
    // Cross-implementation pin: the export generator (python FNV-1a over the
    // condition text) and this module (tree-sitter condition + FNV-1a) must
    // agree on the anchor, or joins between coverage hits and changed
    // regions silently break. Value comes from the committed fixture export.
    let sites = extract_branch_sites(OLD_SRC).unwrap();
    assert_eq!(sites[0].anchor, "cd6054b02dde");
}

#[test]
fn test_condition_change_moves_anchor() {
    let new_cond = OLD_SRC.replace("denominator == 0", "denominator <= 0");
    let a = extract_branch_sites(OLD_SRC).unwrap()[0].anchor.clone();
    let b = extract_branch_sites(&new_cond).unwrap()[0].anchor.clone();
    assert_ne!(a, b);
}

// Review finding #5 gate (cross-revision persistence): a rustfmt reflow of a
// condition is a semantic-preserving transformation — the branch anchor must
// survive it so imported evidence keeps joining.
#[test]
fn test_branch_anchor_survives_formatting_reflow() {
    const REFLOWED: &str = "pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {\n    if denominator\n        == 0\n    {\n        return Err(\"division by zero\");\n    }\n\n    Ok(numerator / denominator)\n}\n";
    let base = extract_branch_sites(OLD_SRC).unwrap();
    let reflowed = extract_branch_sites(REFLOWED).unwrap();
    assert_eq!(base.len(), 1);
    assert_eq!(reflowed.len(), 1, "reflow must not change the site count");
    assert_eq!(
        base[0].anchor, reflowed[0].anchor,
        "whitespace-only reflow must not re-anchor the branch: {} vs {}",
        base[0].anchor, reflowed[0].anchor
    );
}

fn item_paths(src: &str) -> Vec<String> {
    extract_function_sites(src)
        .unwrap()
        .into_iter()
        .map(|s| s.item_path)
        .collect()
}

#[test]
fn function_ids_are_qualified_by_file() {
    let src = "pub fn new() {}\n";
    let a = changed_regions("src/a.rs", "", src).unwrap().functions;
    let b = changed_regions("src/b.rs", "", src).unwrap().functions;
    assert_eq!(a, vec!["fn:src/a.rs::new".to_string()]);
    assert_eq!(b, vec!["fn:src/b.rs::new".to_string()]);
}

#[test]
fn item_paths_name_mods_impls_traits_and_nested_fns() {
    let src = r#"
mod inner {
    pub struct A;
    impl A { pub fn new() -> Self { A } }
    pub struct B<T>(T);
    impl<T> B<T> { pub fn new(t: T) -> Self { B(t) } }
}
pub struct X;
impl From<u8> for X { fn from(_: u8) -> Self { X } }
impl From<u16> for X { fn from(_: u16) -> Self { X } }
pub trait T { fn dflt(&self) {} }
pub fn outer() { fn helper() {} }
"#;
    assert_eq!(
        item_paths(src),
        vec![
            "inner::A::new",
            "inner::B-T-::new",
            "X.as.From-u8-::from",
            "X.as.From-u16-::from",
            "T::dflt",
            "outer",
            "outer::helper",
        ]
    );
}

#[test]
fn repeated_item_paths_get_source_order_ordinals() {
    let src = "#[cfg(unix)]\nfn f() {}\n#[cfg(not(unix))]\nfn f() {}\n";
    assert_eq!(item_paths(src), vec!["f", "f.2"]);
}

const TWIN: &str = "pub fn f(x: i32) -> i32 {\n    if x == 0 {\n        return 1;\n    }\n    if x == 0 {\n        return 2;\n    }\n    x\n}\n";

#[test]
fn identical_conditions_get_ordinals_and_distinct_ids() {
    let sites = extract_branch_sites(TWIN).unwrap();
    assert_eq!(sites.len(), 2);
    assert_eq!(sites[0].anchor, sites[1].anchor);
    assert_eq!((sites[0].ordinal, sites[1].ordinal), (1, 2));
    let a = sites[0].region_id("src/lib.rs");
    let b = sites[1].region_id("src/lib.rs");
    assert_eq!(b, format!("{a}.2"));

    // Editing only the second site's body flags only the second id.
    let edited = TWIN.replace("return 2;", "return 3;");
    let ch = changed_regions("src/lib.rs", TWIN, &edited).unwrap();
    assert_eq!(ch.branches, vec![b]);
}

#[test]
fn twin_branch_ids_survive_whitespace_reflow() {
    let reflowed = TWIN.replacen("if x == 0 {", "if x\n        == 0\n    {", 2);
    let ids = |src: &str| -> Vec<String> {
        extract_branch_sites(src)
            .unwrap()
            .iter()
            .map(|s| s.region_id("src/lib.rs"))
            .collect()
    };
    assert_eq!(ids(TWIN), ids(&reflowed));
}

#[test]
fn ids_stay_in_the_importer_charset_and_cap() {
    let odd = "src/dir with space/ü.rs";
    let seg = file_segment(odd);
    assert!(seg.starts_with("src/dir_with_space/_.rs.h"), "{seg}");
    assert_ne!(seg, file_segment("src/dir_with_space/_.rs"));

    let long_path = format!("src/{}.rs", "d/".repeat(200));
    let id = function_region_id(&long_path, "f");
    assert!(id.len() <= MAX_REGION_ID_BYTES, "{id}");
    assert_ne!(id, function_region_id(&format!("{long_path}x"), "f"));
    for id in [id, branch_region_id(odd, "A-T-::f", "cd6054b02dde", 3)] {
        assert!(
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '/' | '-')),
            "{id}"
        );
        assert!(id.len() <= MAX_REGION_ID_BYTES);
    }
}

#[test]
fn legacy_ids_are_detected_and_references_over_approximate() {
    assert!(!is_qualified_region_id("fn:new"));
    assert!(!is_qualified_region_id("branch:safe_divide:cd6054b02dde"));
    assert!(is_qualified_region_id("fn:src/lib.rs::new"));
    assert!(is_qualified_region_id(
        "branch:src/lib.rs::safe_divide:cd6054b02dde"
    ));

    // A qualified reference matches only its own site.
    assert!(!reference_matches("fn:src/a.rs::new", "fn:src/b.rs::new"));
    // A legacy reference names no site: it matches every same-leaf site.
    assert!(reference_matches("fn:new", "fn:src/b.rs::B::new"));
    assert!(reference_matches("fn:f", "fn:src/b.rs::f.2"));
    assert!(!reference_matches("fn:new", "fn:src/b.rs::renew"));
    assert!(reference_matches(
        "branch:safe_divide:cd6054b02dde",
        "branch:src/lib.rs::safe_divide:cd6054b02dde.2"
    ));
    assert!(!reference_matches(
        "branch:safe_divide:cd6054b02dde",
        "branch:src/lib.rs::safe_divide:000000000000"
    ));
}

#[test]
fn edited_paths_are_made_repo_relative() {
    use phronesis_mcp::coverage::region_map::repo_relative_path;
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/lib.rs"), "").unwrap();
    let abs = root.path().join("src/lib.rs");
    assert_eq!(
        repo_relative_path(root.path(), &abs.display().to_string()).as_deref(),
        Some("src/lib.rs")
    );
    let canonical = abs.canonicalize().unwrap();
    assert_eq!(
        repo_relative_path(root.path(), &canonical.display().to_string()).as_deref(),
        Some("src/lib.rs")
    );
    // A file the edit is about to create does not exist yet.
    let new_file = root.path().canonicalize().unwrap().join("src/new.rs");
    assert_eq!(
        repo_relative_path(root.path(), &new_file.display().to_string()).as_deref(),
        Some("src/new.rs")
    );
    assert_eq!(
        repo_relative_path(root.path(), "./src/lib.rs").as_deref(),
        Some("src/lib.rs")
    );
    assert_eq!(repo_relative_path(root.path(), "../other/src/lib.rs"), None);
    let elsewhere = tempfile::tempdir().unwrap();
    let outside = elsewhere.path().join("lib.rs");
    std::fs::write(&outside, "").unwrap();
    assert_eq!(
        repo_relative_path(root.path(), &outside.display().to_string()),
        None
    );
}

// Capping must keep the qualified shape: a >256-byte id that lost its `::`
// would be refused by the importer and read as a legacy id by hydration.
#[test]
fn capped_ids_stay_qualified_and_keep_their_leaf() {
    let long_path = format!("crates/x/src/{}.rs", "d".repeat(300));
    let deep_item = format!("{}::leaf_fn", vec!["m"; 150].join("::"));
    for id in [
        function_region_id(&long_path, "f"),
        function_region_id("src/lib.rs", &deep_item),
        function_region_id(&long_path, &deep_item),
        branch_region_id(&long_path, &deep_item, "cd6054b02dde", 7),
    ] {
        assert!(id.len() <= MAX_REGION_ID_BYTES, "{} bytes: {id}", id.len());
        assert!(
            is_qualified_region_id(&id),
            "capped id lost its shape: {id}"
        );
    }
    // Distinct inputs stay distinct after capping.
    assert_ne!(
        function_region_id(&long_path, "f"),
        function_region_id(&format!("{long_path}x"), "f")
    );
    assert_ne!(
        function_region_id("src/lib.rs", &deep_item),
        function_region_id("src/lib.rs", &format!("n::{deep_item}"))
    );
    // Legacy references still find the leaf of a capped id.
    assert!(reference_matches(
        "fn:f",
        &function_region_id(&long_path, "f")
    ));
    assert!(reference_matches(
        "fn:leaf_fn",
        &function_region_id(&long_path, &deep_item)
    ));
    assert!(reference_matches(
        "branch:leaf_fn:cd6054b02dde",
        &branch_region_id(&long_path, &deep_item, "cd6054b02dde", 7)
    ));
}
