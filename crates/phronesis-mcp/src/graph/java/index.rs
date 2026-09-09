//! Pure declaration index. No filesystem or build-system guesses occur here.

use std::collections::{BTreeMap, BTreeSet};

/// Compilation context of a declared type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Context {
    Production,
    Test,
    Unclaimed,
}

/// A declaration's identity and the source file that provides it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Owner {
    pub unit: String,
    pub module: String,
    pub context: Context,
    pub file: String,
}

/// Import syntax, normalized by the parser without resolving names.
#[derive(Debug, Clone, Copy)]
pub enum Import<'a> {
    Type(&'a str),
    Wildcard(&'a str),
    StaticMember(&'a str),
    StaticWildcard(&'a str),
    Module,
}

/// An absent declaration differs from a project lookup we cannot resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Module(String),
    External,
    Unresolved,
    SelfImport,
    ModuleImportIgnored,
}

/// Names are keyed exactly as declared, including nested member type paths.
#[derive(Debug, Clone, Default)]
pub struct DeclarationIndex {
    packages: BTreeMap<String, BTreeSet<Owner>>,
    types: BTreeMap<String, BTreeSet<Owner>>,
}

/// Build a package identity from a unit and a declared dotted package name.
pub fn module_id(unit: &str, package: &str) -> String {
    if package.is_empty() {
        format!("java:{unit}")
    } else {
        format!("java:{unit}::{}", package.replace('.', "::"))
    }
}

impl DeclarationIndex {
    /// Register a package and all named member type paths from one file.
    /// `types` are relative to the package, e.g. `Outer.Inner.Item`.
    pub fn insert(&mut self, package: &str, owner: Owner, types: &[String]) {
        self.packages
            .entry(package.to_string())
            .or_default()
            .insert(owner.clone());
        for name in types {
            let qualified = if package.is_empty() {
                name.clone()
            } else {
                format!("{package}.{name}")
            };
            self.types
                .entry(qualified)
                .or_default()
                .insert(owner.clone());
        }
    }

    /// Resolve against eligible declaration files supplied by discovery.
    /// The predicate enforces compilation visibility even within a Bazel
    /// unit, where targets in one package may have different classpaths.
    /// Test-over-main shadowing applies only within the source's own unit.
    pub fn resolve(
        &self,
        import: Import<'_>,
        source: &Owner,
        eligible: impl Fn(&Owner) -> bool,
    ) -> Resolution {
        let (owners, package_lookup) = match import {
            Import::Module => return Resolution::ModuleImportIgnored,
            Import::Type(name) | Import::StaticWildcard(name) => (self.types.get(name), false),
            Import::Wildcard(name) => match self.packages.get(name) {
                Some(owners) => (Some(owners), true),
                None => (self.types.get(name), false),
            },
            Import::StaticMember(name) => {
                // JLS 7.5.3: TypeName.Identifier. Removing further segments
                // would reinterpret an invalid member path as another type.
                let owners = name
                    .rsplit_once('.')
                    .and_then(|(declaring_type, _)| self.types.get(declaring_type));
                (owners, false)
            }
        };
        let Some(owners) = owners else {
            return Resolution::External;
        };
        let Some(owner) = Self::select(owners, package_lookup, source, eligible) else {
            return Resolution::Unresolved;
        };
        if owner.module == source.module {
            Resolution::SelfImport
        } else {
            Resolution::Module(owner.module)
        }
    }

    /// Preserve the selected declaration's file for exact function lookup.
    pub fn type_owner(
        &self,
        name: &str,
        source: &Owner,
        eligible: impl Fn(&Owner) -> bool,
    ) -> Option<Owner> {
        Self::select(self.types.get(name)?, false, source, eligible)
    }

    fn select(
        owners: &BTreeSet<Owner>,
        package_lookup: bool,
        source: &Owner,
        eligible: impl Fn(&Owner) -> bool,
    ) -> Option<Owner> {
        let visible = owners
            .iter()
            .filter(|owner| {
                (source.context != Context::Production || owner.context != Context::Test)
                    && eligible(owner)
            })
            .collect::<Vec<_>>();
        let local = visible
            .iter()
            .copied()
            .filter(|owner| owner.unit == source.unit)
            .collect::<Vec<_>>();
        let preferred = if source.context == Context::Test
            && local.iter().any(|owner| owner.context == Context::Test)
        {
            local
                .into_iter()
                .filter(|owner| owner.context == Context::Test)
                .collect::<Vec<_>>()
        } else {
            local
        };
        let candidates = if preferred.is_empty() {
            visible
        } else {
            preferred
        };
        // Filter by source file before deduplicating package owners. Three
        // visible files in a package are one candidate; duplicate types in
        // distinct files remain ambiguous.
        let identities = candidates
            .iter()
            .map(|owner| {
                (
                    &owner.unit,
                    &owner.module,
                    owner.context,
                    if package_lookup { "" } else { &owner.file },
                )
            })
            .collect::<BTreeSet<_>>();
        if identities.len() != 1 {
            return None;
        }
        candidates.first().map(|owner| (*owner).clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(unit: &str, package: &str, file: &str, context: Context) -> Owner {
        Owner {
            unit: unit.into(),
            module: module_id(unit, package),
            context,
            file: file.into(),
        }
    }

    fn insert(index: &mut DeclarationIndex, unit: &str, package: &str, file: &str, name: &str) {
        index.insert(
            package,
            owner(unit, package, file, Context::Production),
            &[name.into()],
        );
    }

    #[test]
    fn exact_import_forms_resolve_without_ancestor_fallback() {
        let mut index = DeclarationIndex::default();
        insert(&mut index, "app", "a", "A.java", "Outer");
        insert(&mut index, "app", "a", "A.java", "Outer.Inner.Item");
        let source = owner("app", "b", "B.java", Context::Production);
        for import in [
            Import::Type("a.Outer"),
            Import::Type("a.Outer.Inner.Item"),
            Import::Wildcard("a"),
            Import::Wildcard("a.Outer"),
            Import::StaticMember("a.Outer.make"),
            Import::StaticWildcard("a.Outer"),
        ] {
            assert_eq!(
                index.resolve(import, &source, |_| true),
                Resolution::Module("java:app::a".into())
            );
        }
        for import in [
            Import::Type("a.vendor.Widget"),
            Import::Wildcard("a.vendor"),
            Import::StaticMember("a.Outer.missing.make"),
        ] {
            assert_eq!(
                index.resolve(import, &source, |_| true),
                Resolution::External
            );
        }
        assert_eq!(
            index.resolve(Import::Module, &source, |_| true),
            Resolution::ModuleImportIgnored
        );
    }

    #[test]
    fn package_files_deduplicate_but_duplicate_types_do_not() {
        let mut index = DeclarationIndex::default();
        for file in ["A.java", "B.java", "C.java"] {
            insert(&mut index, "lib", "p", file, "Duplicate");
        }
        let source = owner("app", "a", "Main.java", Context::Production);
        assert_eq!(
            index.resolve(Import::Wildcard("p"), &source, |_| true),
            Resolution::Module("java:lib::p".into())
        );
        assert_eq!(
            index.resolve(Import::Type("p.Duplicate"), &source, |_| true),
            Resolution::Unresolved
        );
    }

    #[test]
    fn build_visibility_filters_same_unit_and_cross_unit_candidates() {
        let mut index = DeclarationIndex::default();
        insert(&mut index, "app", "p", "Private.java", "Private");
        insert(&mut index, "lib", "q", "Api.java", "Api");
        let source = owner("app", "main", "Main.java", Context::Production);
        for name in ["p.Private", "q.Api"] {
            assert_eq!(
                index.resolve(Import::Type(name), &source, |_| false),
                Resolution::Unresolved
            );
        }
        assert_eq!(
            index.resolve(Import::Type("q.Api"), &source, |o| o.file == "Api.java"),
            Resolution::Module("java:lib::q".into())
        );
    }

    #[test]
    fn split_package_selection_is_local_then_unique_visible_owner() {
        let mut index = DeclarationIndex::default();
        for unit in ["app", "left", "right"] {
            insert(&mut index, unit, "p", &format!("{unit}/A.java"), "A");
        }
        let source = owner("app", "main", "Main.java", Context::Production);
        assert_eq!(
            index.resolve(Import::Type("p.A"), &source, |_| true),
            Resolution::Module("java:app::p".into())
        );
        let source = owner("other", "main", "Other.java", Context::Production);
        assert_eq!(
            index.resolve(Import::Type("p.A"), &source, |_| true),
            Resolution::Unresolved
        );
        assert_eq!(
            index.resolve(Import::Type("p.A"), &source, |o| o.unit == "left"),
            Resolution::Module("java:left::p".into())
        );
    }

    #[test]
    fn test_context_shadows_main_without_hiding_production_types() {
        let mut index = DeclarationIndex::default();
        insert(&mut index, "app", "p", "main/Env.java", "Env");
        index.insert(
            "p",
            owner("app", "p", "test/Env.java", Context::Test),
            &["Env".into(), "TestOnly".into()],
        );
        for context in [Context::Production, Context::Test] {
            let source = owner("app", "q", "Use.java", context);
            assert_eq!(
                index.resolve(Import::Type("p.Env"), &source, |_| true),
                Resolution::Module("java:app::p".into())
            );
            assert_eq!(
                index.resolve(Import::Wildcard("p"), &source, |_| true),
                Resolution::Module("java:app::p".into())
            );
        }
        let source = owner("app", "q", "Use.java", Context::Production);
        assert_eq!(
            index.resolve(Import::Type("p.TestOnly"), &source, |_| true),
            Resolution::Unresolved
        );
    }

    #[test]
    fn self_imports_and_default_package_do_not_invent_cycles() {
        assert_eq!(module_id("group:artifact", ""), "java:group:artifact");
        let mut index = DeclarationIndex::default();
        insert(&mut index, "app", "p", "A.java", "A");
        let source = owner("app", "p", "B.java", Context::Production);
        assert_eq!(
            index.resolve(Import::Type("p.A"), &source, |_| true),
            Resolution::SelfImport
        );
    }
}
