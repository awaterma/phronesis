use phronesis_mcp::coverage::region_map::{
    branch_region_id, changed_regions, extract_branch_sites, function_region_id,
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
    let ch = changed_regions(OLD_SRC, NEW_SRC).unwrap();
    assert!(
        ch.branches
            .contains(&branch_region_id("safe_divide", &old[0].anchor))
    );
    assert!(ch.functions.contains(&function_region_id("safe_divide")));
}

#[test]
fn test_untouched_function_not_reported() {
    let old = format!("{OLD_SRC}pub fn helper() -> i32 {{ 1 }}\n");
    let new = format!("{NEW_SRC}pub fn helper() -> i32 {{ 1 }}\n");
    let ch = changed_regions(&old, &new).unwrap();
    assert!(!ch.functions.contains(&function_region_id("helper")));
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
