//! The language-pack registry: built-in and project-installed packs, loaded
//! strictly for runtime use (`load`) or inspected tolerantly for listing
//! (`inspect`). Neither path executes a query, script, or fixture — that
//! only happens when `compile_pack` (and, through it, fixture verification
//! during install) decides a pack is otherwise sound.
//!
//! Built-in packs are supplied by per-language builtin modules — Go's lives
//! at `language_packs/builtin.rs` — through `builtin_pack_sources`, below.
//! That function returns raw `PackAssets` (manifest bytes parse without any
//! optional feature); append a source there once `builtin.rs` lands. This
//! file defines the seam so built-in wiring needs no further registry
//! changes, and so a built-in pack whose adapter needs a compiled-out
//! feature (e.g. Rhai) is still listed — with `status: unsupported` — rather
//! than silently vanishing.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::assets::{PackAssets, digest_assets, read_assets, read_bounded};
use super::error::PackError;
use super::extract::{ValidatedPack, compile_pack};
use super::limits;
use super::manifest::{Capability, PackManifest, check_ownership, validate_manifest};
use super::records::PackDiagnostic;

/// Where installed packs live, relative to the project root.
pub const PACKS_DIR: &str = ".phronesis/packs";
/// Per-installed-pack metadata file recording the digest it was installed at.
pub const INSTALLED_FILE: &str = "installed.json";
/// `installed.json` schema version this binary writes and requires on read.
pub(crate) const INSTALLED_SCHEMA_VERSION: u32 = 1;

/// The host-authored record of what was installed: schema/API version, the
/// pack identity, and the content digest at install time. `install` writes
/// this; every later read of an installed pack (`enumerate`, re-install)
/// re-derives the digest from disk and compares it here, so bytes changed
/// after installation are a hard, explicit error rather than silently
/// accepted new content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InstalledMeta {
    pub schema_version: u32,
    pub adapter_api: u32,
    pub id: String,
    pub version: String,
    pub digest: String,
}

/// Extensions natively owned outside the pack system; a pack may never claim
/// one of these (v1: no shadowing of native or built-in language ownership).
/// This includes every host-evaluated-but-not-executed extension (`cue`,
/// `json`, `yaml`/`yml`, `tpl`, `rhai`) alongside the structurally parsed
/// native languages, so a pack cannot shadow host configuration or scripting
/// surfaces either.
pub(crate) const NATIVE_EXTENSIONS: &[(&str, &str)] = &[
    ("rs", "rust"),
    ("py", "python"),
    ("ts", "typescript"),
    ("tsx", "typescript"),
    ("mts", "typescript"),
    ("cts", "typescript"),
    ("js", "javascript"),
    ("jsx", "javascript"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("swift", "swift"),
    ("java", "java"),
    ("lua", "lua"),
    ("cue", "cue"),
    ("json", "json"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("tpl", "helm"),
    ("rhai", "rhai"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackOrigin {
    BuiltIn,
    Installed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackStatus {
    Valid,
    Invalid,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Activation {
    Active,
    Available,
    Overridden,
    Unsupported,
    Invalid,
    NotProvided,
}

/// One pack as reported by `pack list` / `list_language_packs`. Fields that
/// require a valid manifest are `None` for an invalid pack; `diagnostics`
/// always explains why, so a broken pack is a visible, identifiable entry
/// rather than a silent omission.
#[derive(Debug, Clone, Serialize)]
pub struct PackInfo {
    pub id: Option<String>,
    pub version: Option<String>,
    pub language: Option<String>,
    pub grammar: Option<String>,
    pub adapter_api: Option<u32>,
    pub digest: Option<String>,
    pub origin: PackOrigin,
    pub location: String,
    pub extensions: Vec<String>,
    pub capabilities: Vec<Capability>,
    pub activation: BTreeMap<String, Activation>,
    pub status: PackStatus,
    pub diagnostics: Vec<PackDiagnostic>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PackInventory {
    pub packs: Vec<PackInfo>,
    pub diagnostics: Vec<PackDiagnostic>,
    pub complete: bool,
}

/// A loaded, strict snapshot: every extension uniquely owned, and every
/// installed pack compiled. Built for runtime consumers (hooks, audit,
/// graph).
pub struct PackRegistry {
    root: PathBuf,
    packs: Vec<RegistryEntry>,
    by_extension: BTreeMap<String, usize>,
    /// Extensions whose pack exists but could not be activated right now
    /// (e.g. a built-in pack needing a compiled-out feature). Looking one up
    /// returns an explicit `PackError::Unsupported` rather than silently
    /// falling through to native/unknown-file handling.
    unsupported_extensions: BTreeMap<String, String>,
}

/// One pack in a [`PackRegistry`]. An installed pack is compiled while
/// loading, so a broken install fails `load` and the hooks fail closed. A
/// built-in pack is compiled on first use: its bytes ship in this binary
/// and a unit test holds them to their fixtures, and compiling its queries
/// on every hook invocation (most of which never touch its language) cost
/// about 2 ms per registry load.
struct RegistryEntry {
    manifest: PackManifest,
    origin: PackOrigin,
    pending: std::sync::Mutex<Option<PackAssets>>,
    compiled: std::sync::OnceLock<Result<ValidatedPack, String>>,
}

impl RegistryEntry {
    fn compiled(pack: ValidatedPack, origin: PackOrigin) -> Self {
        let manifest = pack.assets.manifest.clone();
        let compiled = std::sync::OnceLock::new();
        let _ = compiled.set(Ok(pack));
        Self {
            manifest,
            origin,
            pending: std::sync::Mutex::new(None),
            compiled,
        }
    }

    fn deferred(assets: PackAssets, origin: PackOrigin) -> Self {
        Self {
            manifest: assets.manifest.clone(),
            origin,
            pending: std::sync::Mutex::new(Some(assets)),
            compiled: std::sync::OnceLock::new(),
        }
    }

    fn pack(&self) -> Result<&ValidatedPack, PackError> {
        self.compiled
            .get_or_init(|| {
                let assets = self.pending.lock().ok().and_then(|mut slot| slot.take());
                match assets {
                    Some(assets) => compile_pack(assets).map_err(|e| e.to_string()),
                    None => Err("pack assets are unavailable".to_string()),
                }
            })
            .as_ref()
            .map_err(|message| PackError::Record {
                pack: Some(self.manifest.id.clone()),
                path: None,
                message: message.clone(),
            })
    }
}

impl PackRegistry {
    /// Root-relative on-disk inputs that can change the loaded snapshot or
    /// an adapter's nearest context for a tracked source. Embedded assets
    /// have no disk path; graph freshness records their digest separately.
    pub fn input_files(&self) -> Vec<PathBuf> {
        fn visit(root: &Path, dir: &Path, out: &mut BTreeSet<PathBuf>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && !path.is_symlink() {
                    visit(root, &path, out);
                } else if let Ok(rel) = path.strip_prefix(root) {
                    out.insert(rel.to_path_buf());
                }
            }
        }
        let mut out = BTreeSet::new();
        visit(&self.root, &self.root.join(PACKS_DIR), &mut out);
        for entry in ignore::WalkBuilder::new(&self.root)
            .hidden(true)
            .build()
            .flatten()
        {
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&self.root) else {
                continue;
            };
            let Some(rel_str) = rel.to_str() else {
                continue;
            };
            let Ok(Some(pack)) = self.for_path_with(rel_str, Capability::Graph) else {
                continue;
            };
            let parent = rel.parent().unwrap_or_else(|| Path::new(""));
            for name in &pack.assets.manifest.context_files {
                for dir in parent.ancestors() {
                    let candidate = dir.join(name);
                    if fs::symlink_metadata(self.root.join(&candidate)).is_ok() {
                        out.insert(candidate);
                        break;
                    }
                }
            }
        }
        out.into_iter().collect()
    }

    /// Every pack's manifest, without compiling anything.
    pub fn manifests(&self) -> impl Iterator<Item = &PackManifest> {
        self.packs.iter().map(|entry| &entry.manifest)
    }

    /// The installed (project) packs, all compiled at load.
    pub fn installed_packs(&self) -> Result<Vec<&ValidatedPack>, PackError> {
        self.packs
            .iter()
            .filter(|entry| entry.origin == PackOrigin::Installed)
            .map(RegistryEntry::pack)
            .collect()
    }

    /// The validated pack owning `rel`'s extension, if any. `Ok(None)` means
    /// no pack claims this extension; `Err` means a pack claims it but
    /// cannot be used right now (explicit, not a silent fallback).
    pub fn for_path(&self, rel: &str) -> Result<Option<&ValidatedPack>, PackError> {
        let Some(ext) = Path::new(rel).extension().and_then(|e| e.to_str()) else {
            return Ok(None);
        };
        if let Some(reason) = self.unsupported_extensions.get(ext) {
            return Err(PackError::Unsupported {
                pack: None,
                message: format!(
                    "`.{ext}` is owned by a language pack that is currently unsupported: {reason}"
                ),
            });
        }
        self.by_extension
            .get(ext)
            .map(|&i| self.packs[i].pack())
            .transpose()
    }

    /// [`for_path`](Self::for_path), restricted to a pack that declares
    /// `capability`. A pack's capabilities decide what it runs: callers that
    /// parse source for facts ask for `StructuralFacts`, callers that build
    /// graph evidence ask for `Graph`, so a pack that declared neither (a
    /// toolchains-only pack) is never parsed.
    pub fn for_path_with(
        &self,
        rel: &str,
        capability: Capability,
    ) -> Result<Option<&ValidatedPack>, PackError> {
        // Decide from the manifest first, so a caller that needs a
        // capability the owning pack lacks never compiles it.
        let Some(ext) = Path::new(rel).extension().and_then(|e| e.to_str()) else {
            return Ok(None);
        };
        if let Some(&i) = self.by_extension.get(ext)
            && !self.packs[i].manifest.has(capability)
        {
            return Ok(None);
        }
        Ok(self
            .for_path(rel)?
            .filter(|pack| pack.assets.manifest.has(capability)))
    }
}

/// Raw manifest+asset bytes for every built-in pack, independent of whether
/// its capability-requiring features are compiled in. Empty until a builtin
/// module (e.g. `language_packs::builtin` for Go) supplies entries.
pub(crate) fn builtin_pack_sources() -> Vec<PackAssets> {
    super::builtin::builtin_pack_sources()
}

/// One enumerated pack location: identity is established the moment its
/// assets fail to read/parse/validate, so a broken pack is a visible,
/// identifiable entry (via `PackError::pack()`) rather than a bare
/// diagnostic with no id.
pub(crate) struct Enumerated {
    pub(crate) origin: PackOrigin,
    pub(crate) location: String,
    pub(crate) assets: Result<PackAssets, PackError>,
}

/// Enumerate every pack this project could see: built-in sources plus
/// `.phronesis/packs/*`. Reads bytes only; never executes anything. A
/// directory-level read failure (e.g. `.phronesis/packs` itself is missing a
/// permission) is reported as a location-less diagnostic in the returned
/// `Vec`'s second element; per-pack failures stay attached to their entry.
pub(crate) fn enumerate(root: &Path) -> (Vec<Enumerated>, Vec<PackDiagnostic>) {
    let mut out = Vec::new();
    let mut top_level_diagnostics = Vec::new();

    for assets in builtin_pack_sources() {
        // Built-in sources are embedded bytes (T6's `builtin.rs`); re-run the
        // same manifest validation installed packs get so there is exactly
        // one validation path, never a privileged shortcut for built-ins.
        let result = validate_manifest(&assets.manifest).map(|()| assets);
        out.push(Enumerated {
            origin: PackOrigin::BuiltIn,
            location: "built-in".to_string(),
            assets: result,
        });
    }

    let packs_dir = root.join(PACKS_DIR);
    match fs::symlink_metadata(&packs_dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (out, top_level_diagnostics),
        Err(e) => {
            top_level_diagnostics.push(PackDiagnostic {
                pack: None,
                path: Some(packs_dir.display().to_string()),
                stage: "read".into(),
                message: format!("cannot inspect {}: {e}", packs_dir.display()),
            });
            return (out, top_level_diagnostics);
        }
        Ok(meta) if !meta.is_dir() => {
            top_level_diagnostics.push(PackDiagnostic {
                pack: None,
                path: Some(packs_dir.display().to_string()),
                stage: "read".into(),
                message: "installed-pack root is not a directory or is a symlink".into(),
            });
            return (out, top_level_diagnostics);
        }
        Ok(_) => {}
    }
    let entries = match fs::read_dir(&packs_dir) {
        Ok(entries) => entries,
        Err(e) => {
            top_level_diagnostics.push(PackDiagnostic {
                pack: None,
                path: Some(packs_dir.display().to_string()),
                stage: "read".into(),
                message: format!("cannot read {}: {e}", packs_dir.display()),
            });
            return (out, top_level_diagnostics);
        }
    };

    let mut locations = Vec::new();
    let mut scanned = 0usize;
    for entry in entries {
        scanned += 1;
        if scanned > limits::MAX_INSTALLED_PACKS + 8 {
            top_level_diagnostics.push(PackDiagnostic {
                pack: None,
                path: Some(packs_dir.display().to_string()),
                stage: "limit".into(),
                message: "installed-pack directory has too many entries".into(),
            });
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                top_level_diagnostics.push(PackDiagnostic {
                    pack: None,
                    path: Some(packs_dir.display().to_string()),
                    stage: "read".into(),
                    message: format!("cannot enumerate installed packs: {e}"),
                });
                break;
            }
        };
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if locations.len() == limits::MAX_INSTALLED_PACKS {
            top_level_diagnostics.push(PackDiagnostic {
                pack: None,
                path: Some(packs_dir.display().to_string()),
                stage: "limit".into(),
                message: format!(
                    "installed packs exceed the limit of {}",
                    limits::MAX_INSTALLED_PACKS
                ),
            });
            break;
        }
        locations.push(entry.path());
    }
    locations.sort();
    for path in locations {
        let location = path.display().to_string();
        let assets = match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => read_installed_pack_assets(&path),
            Ok(_) => Err(PackError::UnsafePath {
                pack: path.file_name().map(|n| n.to_string_lossy().to_string()),
                path: location.clone(),
                reason: "installed-pack entry is not a directory or is a symlink".into(),
            }),
            Err(source) => Err(PackError::Io {
                pack: path.file_name().map(|n| n.to_string_lossy().to_string()),
                path: location.clone(),
                source,
            }),
        };
        out.push(Enumerated {
            origin: PackOrigin::Installed,
            location,
            assets,
        });
    }

    (out, top_level_diagnostics)
}

/// Existing ownership inputs for both real and dry-run installs. Enumeration
/// is bounded and strict here: a damaged entry cannot be ignored while a new
/// pack claims its extension.
pub(crate) fn manifests_for_install(
    root: &Path,
    exclude_id: &str,
) -> Result<(Vec<PackManifest>, usize), PackError> {
    let (entries, diagnostics) = enumerate(root);
    if let Some(d) = diagnostics.into_iter().next() {
        return Err(PackError::Manifest {
            pack: d.pack,
            path: d.path.unwrap_or_default(),
            field: d.stage,
            message: d.message,
        });
    }
    let mut manifests = Vec::new();
    let mut installed_others = 0usize;
    for entry in entries {
        let assets = entry.assets?;
        if entry.origin == PackOrigin::BuiltIn || assets.manifest.id != exclude_id {
            if entry.origin == PackOrigin::Installed {
                installed_others += 1;
            }
            manifests.push(assets.manifest);
        }
    }
    Ok((manifests, installed_others))
}

/// Read an installed pack's assets and verify the install is intact: the
/// recorded `installed.json` schema/id/version match `pack.json`, the
/// directory name matches the pack id, the recomputed content digest
/// matches the one recorded at install time (tampering is an explicit
/// error, never a silent re-adoption of new bytes), and no undeclared file
/// exists in the installed directory. Used by both diagnostic `inspect` and
/// strict `load`, plus `install`'s own idempotent-reinstall check, so there
/// is exactly one definition of "this installed pack is intact".
pub(crate) fn read_installed_pack_assets(dir: &Path) -> Result<PackAssets, PackError> {
    let assets = read_assets(dir)?;
    let id = assets.manifest.id.clone();
    let pack = || Some(id.clone());
    let bad = |field: &str, message: String| PackError::Manifest {
        pack: pack(),
        path: INSTALLED_FILE.to_string(),
        field: field.to_string(),
        message,
    };

    let dir_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if dir_name != id {
        return Err(bad(
            "id",
            format!("installed under directory name `{dir_name}` but pack.json names `{id}`"),
        ));
    }

    let meta_bytes = read_bounded(dir, Some(&id), INSTALLED_FILE, limits::MAX_MANIFEST_BYTES)?;
    let meta: InstalledMeta = serde_json::from_slice(&meta_bytes)
        .map_err(|e| bad("installed_metadata", e.to_string()))?;

    if meta.schema_version != INSTALLED_SCHEMA_VERSION {
        return Err(bad(
            "schema_version",
            format!(
                "unsupported {}; expected {INSTALLED_SCHEMA_VERSION}",
                meta.schema_version
            ),
        ));
    }
    if meta.id != id {
        return Err(bad(
            "id",
            format!(
                "installed.json names `{}` but pack.json names `{id}`",
                meta.id
            ),
        ));
    }
    if meta.version != assets.manifest.version {
        return Err(bad(
            "version",
            format!(
                "installed.json records version `{}` but pack.json has `{}`",
                meta.version, assets.manifest.version
            ),
        ));
    }
    if meta.adapter_api != assets.manifest.adapter_api {
        return Err(bad(
            "adapter_api",
            format!(
                "installed.json records adapter_api `{}` but pack.json has `{}`",
                meta.adapter_api, assets.manifest.adapter_api
            ),
        ));
    }

    let digest = digest_assets(&assets);
    if meta.digest != digest {
        return Err(bad(
            "digest",
            format!(
                "installed content digest `{digest}` does not match the recorded `{}`; \
                 files were modified after installation — remove and reinstall from a trusted source",
                meta.digest
            ),
        ));
    }

    let declared: BTreeSet<&str> = assets.files.keys().map(String::as_str).collect();
    for rel in list_relative_files(dir)? {
        if rel == INSTALLED_FILE {
            continue;
        }
        if !declared.contains(rel.as_str()) {
            return Err(PackError::UnsafePath {
                pack: pack(),
                path: rel,
                reason: "exists in the installed pack directory but is not declared by pack.json"
                    .to_string(),
            });
        }
    }

    Ok(assets)
}

/// Every regular file under `dir`, relative to `dir` with `/` separators.
/// A symlink anywhere in the tree is refused rather than followed or
/// silently skipped, matching the symlink refusal `read_assets` already
/// applies to declared resources.
fn list_relative_files(dir: &Path) -> Result<Vec<String>, PackError> {
    fn walk(base: &Path, cur: &Path, out: &mut Vec<String>) -> Result<(), PackError> {
        let io_err = |path: &Path, source: std::io::Error| PackError::Io {
            pack: None,
            path: path.display().to_string(),
            source,
        };
        let entries = fs::read_dir(cur).map_err(|e| io_err(cur, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| io_err(cur, e))?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).map_err(|e| io_err(&path, e))?;
            if meta.file_type().is_symlink() {
                let rel = path.strip_prefix(base).unwrap_or(&path);
                return Err(PackError::UnsafePath {
                    pack: None,
                    path: rel.to_string_lossy().replace('\\', "/"),
                    reason: "is a symlink".to_string(),
                });
            }
            if meta.is_dir() {
                walk(base, &path, out)?;
            } else {
                let rel = path.strip_prefix(base).unwrap_or(&path);
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

/// Diagnostic, tolerant enumeration: list every pack whether or not it is
/// usable, so a damaged or removed pack stays visible and evaluation cannot
/// silently allow. Never executes a query, script, or fixture.
pub fn inspect(root: &Path) -> PackInventory {
    let (enumerated, mut diagnostics) = enumerate(root);
    let mut packs = Vec::with_capacity(enumerated.len());
    let mut complete = diagnostics.is_empty();

    let valid_manifests: Vec<&PackManifest> = enumerated
        .iter()
        .filter_map(|e| e.assets.as_ref().ok())
        .map(|a| &a.manifest)
        .collect();
    let ownership = check_ownership(valid_manifests.iter().copied(), NATIVE_EXTENSIONS);

    for entry in enumerated {
        let assets = match entry.assets {
            Ok(assets) => assets,
            Err(err) => {
                // Identity is whatever the error could establish (often just
                // the id); the entry is still a visible, diagnosable row.
                complete = false;
                packs.push(PackInfo {
                    id: err.pack().map(str::to_string),
                    version: None,
                    language: None,
                    grammar: None,
                    adapter_api: None,
                    digest: None,
                    origin: entry.origin,
                    location: entry.location,
                    extensions: Vec::new(),
                    capabilities: Vec::new(),
                    activation: BTreeMap::new(),
                    status: PackStatus::Invalid,
                    diagnostics: vec![err.to_diagnostic()],
                });
                continue;
            }
        };

        let manifest = assets.manifest.clone();
        let mut pack_diagnostics = Vec::new();
        let mut status = PackStatus::Valid;

        if let Err(err) = &ownership
            && err.pack().is_none_or(|p| p == manifest.id)
        {
            pack_diagnostics.push(err.to_diagnostic());
            status = PackStatus::Invalid;
            complete = false;
        }

        let digest = digest_assets(&assets);
        let rule_ids = resource_ids(&assets, manifest.rules.as_deref(), Some("rules"));
        let toolchain_ids = resource_ids(&assets, manifest.toolchains.as_deref(), None);
        if status == PackStatus::Valid
            && let Err(err) = compile_pack(assets)
        {
            pack_diagnostics.push(err.to_diagnostic());
            if matches!(err, PackError::Unsupported { .. }) {
                status = PackStatus::Unsupported;
            } else {
                status = PackStatus::Invalid;
                complete = false;
            }
        }

        let activation = activation_for(
            root,
            entry.origin,
            status,
            &manifest,
            &rule_ids,
            &toolchain_ids,
        );

        packs.push(PackInfo {
            id: Some(manifest.id.clone()),
            version: Some(manifest.version.clone()),
            language: Some(manifest.language.clone()),
            grammar: Some(manifest.grammar.clone()),
            adapter_api: Some(manifest.adapter_api),
            digest: Some(digest),
            origin: entry.origin,
            location: entry.location,
            extensions: manifest.extensions,
            capabilities: manifest.capabilities,
            activation,
            status,
            diagnostics: pack_diagnostics,
        });
    }

    packs.sort_by(|a, b| (a.origin, &a.id, &a.location).cmp(&(b.origin, &b.id, &b.location)));
    diagnostics.sort_by(|a, b| (&a.pack, &a.path, &a.message).cmp(&(&b.pack, &b.path, &b.message)));

    PackInventory {
        packs,
        diagnostics,
        complete,
    }
}

fn resource_ids(
    assets: &PackAssets,
    resource: Option<&str>,
    array_field: Option<&str>,
) -> BTreeSet<String> {
    resource
        .and_then(|path| assets.files.get(path))
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
        .and_then(|value| match array_field {
            Some(field) => value
                .get(field)
                .and_then(serde_json::Value::as_array)
                .cloned(),
            None => value.as_array().cloned(),
        })
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            item.get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

fn activation_for(
    root: &Path,
    origin: PackOrigin,
    status: PackStatus,
    manifest: &PackManifest,
    rule_ids: &BTreeSet<String>,
    toolchain_ids: &BTreeSet<String>,
) -> BTreeMap<String, Activation> {
    let mut activation = BTreeMap::new();
    let has = |cap: Capability| manifest.capabilities.contains(&cap);
    let state_for = |provided: bool, project_selected: bool| -> Activation {
        match status {
            PackStatus::Invalid => Activation::Invalid,
            PackStatus::Unsupported => Activation::Unsupported,
            PackStatus::Valid if !provided => Activation::NotProvided,
            PackStatus::Valid => match origin {
                PackOrigin::BuiltIn if project_selected => Activation::Active,
                PackOrigin::BuiltIn => Activation::Available,
                PackOrigin::Installed if project_selected => Activation::Overridden,
                PackOrigin::Installed => Activation::Active,
            },
        }
    };
    let project_rules = crate::rules_file::read(&crate::rules_file::default_path(root))
        .map(|file| file.rules.iter().any(|r| rule_ids.contains(&r.id)))
        .unwrap_or(false);
    let project_toolchains = std::fs::read_to_string(root.join(".phronesis/toolchains.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<serde_json::Value>>(&raw).ok())
        .is_some_and(|defs| {
            defs.iter().any(|def| {
                def.get("id")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|id| toolchain_ids.contains(id))
            })
        });
    // A bundled parser is usable without init; only the starter policy and
    // outcome definitions require project selection. Installed packs opt in
    // directly, subject to explicit project overrides.
    activation.insert(
        "structural_facts".to_string(),
        match status {
            PackStatus::Valid if has(Capability::StructuralFacts) => Activation::Active,
            _ => state_for(has(Capability::StructuralFacts), false),
        },
    );
    activation.insert(
        "graph".to_string(),
        match status {
            PackStatus::Valid if has(Capability::Graph) => Activation::Active,
            _ => state_for(has(Capability::Graph), false),
        },
    );
    activation.insert(
        "rules".to_string(),
        state_for(manifest.rules.is_some(), project_rules),
    );
    activation.insert(
        "toolchains".to_string(),
        state_for(
            manifest.toolchains.is_some() && has(Capability::Toolchains),
            project_toolchains,
        ),
    );
    activation
}

/// Strict snapshot for runtime use: every *installed* pack must compile and
/// every extension must have exactly one owner. A built-in pack whose
/// adapter needs a compiled-out feature is tolerated (its extension is
/// simply not activated) so a native-only project is never broken by it;
/// an installed pack that fails to compile is a hard load error.
pub fn load(root: &Path) -> Result<PackRegistry, PackError> {
    let (enumerated, diagnostics) = enumerate(root);
    if let Some(d) = diagnostics.into_iter().next() {
        return Err(PackError::Manifest {
            pack: d.pack,
            path: d.path.unwrap_or_default(),
            field: d.stage,
            message: d.message,
        });
    }

    // Every entry's assets must have read/parsed/validated cleanly; an
    // invalid *installed* pack is a strict load error, same as a built-in
    // one (built-ins ship from this binary, so a failure here is ours, not
    // a feature-gate case — that is decided at `compile_pack`, below).
    let mut assets = Vec::with_capacity(enumerated.len());
    for entry in enumerated {
        assets.push((entry.origin, entry.assets?));
    }

    let manifests: Vec<&PackManifest> = assets.iter().map(|(_, a)| &a.manifest).collect();
    check_ownership(manifests, NATIVE_EXTENSIONS)?;

    let mut packs = Vec::new();
    let mut by_extension = BTreeMap::new();
    let mut unsupported_extensions = BTreeMap::new();

    for (origin, assets) in assets {
        let manifest = assets.manifest.clone();
        let entry = if origin == PackOrigin::BuiltIn {
            if super::grammar::grammar_for(&manifest.grammar).is_none() {
                let message = format!(
                    "grammar `{}` is a known bundled grammar, but this binary was built without it",
                    manifest.grammar
                );
                for ext in &manifest.extensions {
                    unsupported_extensions.insert(ext.clone(), message.clone());
                }
                continue;
            }
            RegistryEntry::deferred(assets, origin)
        } else {
            RegistryEntry::compiled(compile_pack(assets)?, origin)
        };
        let index = packs.len();
        for ext in &manifest.extensions {
            by_extension.insert(ext.clone(), index);
        }
        packs.push(entry);
    }

    Ok(PackRegistry {
        root: root.to_path_buf(),
        packs,
        by_extension,
        unsupported_extensions,
    })
}

#[cfg(test)]
mod lazy_builtin_tests {
    use super::load;

    /// Loading the registry must not compile a built-in pack: most hook
    /// invocations never touch its language, and compiling the Go pack's
    /// queries cost about 2 ms per load. First use compiles it.
    #[test]
    fn a_built_in_pack_compiles_on_first_use_not_at_load() {
        let dir = tempfile::tempdir().unwrap();
        let registry = load(dir.path()).unwrap();
        if registry.packs.is_empty() {
            return; // the Go grammar is compiled out of this build
        }
        assert!(registry.packs.iter().all(|e| e.compiled.get().is_none()));
        assert!(registry.for_path("src/lib.rs").unwrap().is_none());
        assert!(registry.packs.iter().all(|e| e.compiled.get().is_none()));
        let go = registry.for_path("main.go").unwrap().expect("go pack");
        assert_eq!(go.assets.manifest.id, "go");
        assert!(registry.packs.iter().any(|e| e.compiled.get().is_some()));
    }
}
