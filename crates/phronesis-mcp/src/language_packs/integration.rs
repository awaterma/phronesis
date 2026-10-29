//! Rule and toolchain resources supplied by validated language packs.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::Deserialize;

use super::extract::ValidatedPack;
use super::{PackError, PackRegistry};
use crate::outcomes::toolchain::{CompiledDef, DefSource, ToolchainDef};
use crate::rules_file::{self, DiskRule, SourceRule};

#[derive(Debug, Clone)]
pub struct PackRuleLayer {
    pub name: String,
    pub path: PathBuf,
    pub rules: Vec<DiskRule>,
}

fn invalid(pack: &str, path: &str, field: &str, message: impl Into<String>) -> PackError {
    PackError::Manifest {
        pack: Some(pack.to_owned()),
        path: path.to_owned(),
        field: field.to_owned(),
        message: message.into(),
    }
}

fn rule_layer(pack: &ValidatedPack) -> Result<Option<PackRuleLayer>, PackError> {
    #[derive(Deserialize)]
    struct Wrapper {
        rules: Vec<SourceRule>,
    }
    let manifest = &pack.assets.manifest;
    let Some(path) = &manifest.rules else {
        return Ok(None);
    };
    let bytes = pack
        .assets
        .files
        .get(path)
        .ok_or_else(|| invalid(&manifest.id, path, "rules", "declared resource is absent"))?;
    let wrapper: Wrapper = serde_json::from_slice(bytes)
        .map_err(|e| invalid(&manifest.id, path, "rules", e.to_string()))?;
    let mut ids = HashSet::new();
    let mut rules = Vec::new();
    for source in wrapper.rules {
        if !source.id.starts_with(&format!("{}.", manifest.id)) {
            return Err(invalid(
                &manifest.id,
                path,
                "id",
                format!("rule `{}` must begin with `{}.`", source.id, manifest.id),
            ));
        }
        if !ids.insert(source.id.clone()) {
            return Err(invalid(
                &manifest.id,
                path,
                "id",
                format!("duplicate rule `{}`", source.id),
            ));
        }
        rules.extend(
            rules_file::unfold_or(&source)
                .map_err(|e| invalid(&manifest.id, path, "rules", e.to_string()))?,
        );
    }
    Ok(Some(PackRuleLayer {
        name: format!("pack:{}", manifest.id),
        path: PathBuf::from(format!("pack:{}/{}", manifest.id, path)),
        rules,
    }))
}

fn toolchains(pack: &ValidatedPack) -> Result<Vec<ToolchainDef>, PackError> {
    let manifest = &pack.assets.manifest;
    let Some(path) = &manifest.toolchains else {
        return Ok(Vec::new());
    };
    let bytes = pack.assets.files.get(path).ok_or_else(|| {
        invalid(
            &manifest.id,
            path,
            "toolchains",
            "declared resource is absent",
        )
    })?;
    let defs: Vec<ToolchainDef> = serde_json::from_slice(bytes)
        .map_err(|e| invalid(&manifest.id, path, "toolchains", e.to_string()))?;
    let mut ids = HashSet::new();
    for def in &defs {
        if !def.id.starts_with(&format!("{}.", manifest.id)) {
            return Err(invalid(
                &manifest.id,
                path,
                "id",
                format!("toolchain `{}` must begin with `{}.`", def.id, manifest.id),
            ));
        }
        if !ids.insert(def.id.clone()) {
            return Err(invalid(
                &manifest.id,
                path,
                "id",
                format!("duplicate toolchain `{}`", def.id),
            ));
        }
        CompiledDef::compile(def.clone(), DefSource::BuiltIn)
            .map_err(|e| invalid(&manifest.id, path, "toolchains", e.to_string()))?;
    }
    Ok(defs)
}

/// Install/validate caller: reject malformed pack-owned IDs, duplicate IDs,
/// and invalid rule/toolchain resources before publishing installed bytes.
pub fn validate_pack_resources(pack: &ValidatedPack) -> Result<(), PackError> {
    rule_layer(pack)?;
    toolchains(pack)?;
    Ok(())
}

/// Runtime composition keeps bundled starter policies dormant until `init`
/// copies them to project configuration; installed packs activate directly.
pub fn pack_rule_layers(registry: &PackRegistry) -> Result<Vec<PackRuleLayer>, PackError> {
    let builtin_ids: HashSet<String> = super::builtin::builtin_pack_sources()
        .into_iter()
        .map(|asset| asset.manifest.id)
        .collect();
    let mut layers = Vec::new();
    for pack in registry.installed_packs()? {
        if builtin_ids.contains(&pack.assets.manifest.id) {
            continue;
        }
        if let Some(layer) = rule_layer(pack)? {
            layers.push(layer);
        }
    }
    Ok(layers)
}

pub fn pack_toolchains(registry: &PackRegistry) -> Result<Vec<ToolchainDef>, PackError> {
    let builtin_ids: HashSet<String> = super::builtin::builtin_pack_sources()
        .into_iter()
        .map(|asset| asset.manifest.id)
        .collect();
    let mut out = Vec::new();
    for pack in registry.installed_packs()? {
        if builtin_ids.contains(&pack.assets.manifest.id) {
            continue;
        }
        out.extend(toolchains(pack)?);
    }
    Ok(out)
}
