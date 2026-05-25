use phronesis_mcp::server::extract_rules_from_markdown;

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "docs/RUST-PATTERNS-GUIDE.md".to_string());

    let content = std::fs::read_to_string(&path)?;
    let rules = extract_rules_from_markdown(&content, &path);

    println!("Extracted {} rule(s) from '{}'\n", rules.len(), path);
    for rule in &rules {
        let constraint = rule
            .actions
            .first()
            .map(|a| a.params.join(" "))
            .unwrap_or_default();
        println!("  [{}] {}", rule.id, constraint);
    }
    Ok(())
}
