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
            let predicate = format!("pack_{}_{}", pack.assets.manifest.id, name);
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
