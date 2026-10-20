//! Translate validated custom records into host-owned structural facts.

use phr::Fact;

use super::{FileContext, PackError, PackRecord, PackRegistry, extract};
use crate::fact_id::fact_id;

pub fn facts_for_file(
    registry: &PackRegistry,
    context: &FileContext,
    source: &str,
) -> Result<Vec<Fact>, PackError> {
    Ok(facts_with_lines(registry, context, source)?
        .into_iter()
        .map(|(fact, _)| fact)
        .collect())
}

/// Audit retains the validated source line beside the host fact.
pub fn facts_with_lines(
    registry: &PackRegistry,
    context: &FileContext,
    source: &str,
) -> Result<Vec<(Fact, u32)>, PackError> {
    let Some(pack) = registry.for_path(&context.file_rel)? else {
        return Ok(Vec::new());
    };
    let extraction = extract(pack, context, source)?;
    let mut facts = Vec::new();
    for record in extraction.records {
        if let PackRecord::Fact { name, args, span } = record {
            let predicate = fact_predicate(&pack.assets.manifest.id, &name)?;
            let mut fact_args = Vec::with_capacity(args.len() + 1);
            fact_args.push(context.file_rel.clone());
            fact_args.extend(args);
            let mut identity: Vec<&str> = fact_args.iter().map(String::as_str).collect();
            let byte = span.start_byte.to_string();
            identity.push(&byte);
            facts.push((
                Fact {
                    id: fact_id(&predicate, &identity),
                    predicate,
                    args: fact_args,
                    timestamp: 0,
                    source: Some(format!(
                        "pack:{}@{}:{}",
                        pack.assets.manifest.id, pack.assets.manifest.version, pack.digest
                    )),
                },
                span.start_line,
            ));
        }
    }
    Ok(facts)
}

/// `pack_<id>_<name>`, refused when it names a host-owned `pack_*` relation
/// (all of which live in `GRAPH_RELATIONS`). Load-time id checks
/// (`manifest::check_ownership`, `host_pack_relation_under`) keep this
/// unreachable for today's host vocabulary; the check stays so a host
/// relation added later cannot be forged by an installed pack.
fn fact_predicate(id: &str, name: &str) -> Result<String, PackError> {
    let predicate = format!("pack_{id}_{name}");
    if crate::graph::hydrate::GRAPH_RELATIONS.contains(&predicate.as_str()) {
        return Err(PackError::Record {
            pack: Some(id.to_string()),
            path: None,
            message: format!("fact `{name}` would emit the host-owned predicate `{predicate}`"),
        });
    }
    Ok(predicate)
}

#[cfg(test)]
mod fact_predicate_tests {
    use super::fact_predicate;

    #[test]
    fn a_fact_never_emits_a_host_owned_predicate() {
        assert_eq!(
            fact_predicate("go", "dot_import").unwrap(),
            "pack_go_dot_import"
        );
        let err = fact_predicate("file", "adapter").unwrap_err();
        assert!(err.to_string().contains("pack_file_adapter"), "{err}");
    }
}
