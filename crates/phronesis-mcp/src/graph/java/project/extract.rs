//! Graph edges from one file in the repository-wide Java snapshot.

use super::*;
use crate::graph::java::index::{Import, Resolution};
use crate::graph::java::parse::{Call, ImportDecl, Method};
use crate::graph::{Edge, extract::Extracted};

fn function_id(owner: &Owner, method: &Method) -> String {
    format!(
        "{}::{}::{}",
        owner.module,
        method.declaring_type.replace('.', "::"),
        method.name
    )
}

fn qualified(package: &str, name: &str) -> String {
    if package.is_empty() {
        name.into()
    } else {
        format!("{package}.{name}")
    }
}

impl Project {
    /// Extraction does no I/O and shares discovery's declaration index.
    pub fn extract(&self, path: &str) -> Extracted {
        let Some(file) = self.files.get(path) else {
            return Extracted::default();
        };
        if file.source.parse_failed {
            return Extracted::unparseable();
        }
        let mut out = Extracted {
            skipped: file.source.skipped + usize::from(file.path_mismatch),
            ..Extracted::default()
        };
        let mut edges = BTreeSet::new();
        let mut emit = |predicate: &str, args: &[&str]| {
            edges.insert((
                predicate.to_string(),
                args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            ));
        };
        let context = match file.owner.context {
            Context::Production => "production",
            Context::Test => "test",
            Context::Unclaimed => "unclaimed",
        };
        emit("file_type", &[path, context]);
        emit("declares_module", &[path, &file.owner.module]);
        for import in &file.source.imports {
            match self
                .index
                .resolve(import.as_import(), &file.owner, |owner| {
                    file.visible.contains(&owner.file)
                }) {
                Resolution::Module(target) => emit("imports", &[&file.owner.module, &target]),
                Resolution::Unresolved => out.skipped += 1,
                _ => {}
            }
        }
        for method in &file.source.methods {
            if method.declaring_type.is_empty() {
                out.skipped += 1;
                continue;
            }
            let function = function_id(&file.owner, method);
            emit("defines_fn", &[path, &function]);
            for call in &method.calls {
                if call.receiver.is_some()
                    && call.arity == 0
                    && matches!(
                        call.name.as_str(),
                        "get" | "orElseThrow" | "getAsInt" | "getAsLong" | "getAsDouble"
                    )
                {
                    emit("calls_api", &[&function, &call.name]);
                }
                if file.owner.context == Context::Test
                    && method.test_annotation
                    && let Some(callee) = self.resolve_call(file, method, call)
                {
                    emit("tested_by", &[&callee, &function]);
                }
            }
        }
        out.edges = edges
            .into_iter()
            .map(|(p, a)| Edge {
                p,
                a,
                src: path.into(),
                d: false,
            })
            .collect();
        out
    }

    fn resolve_call(&self, file: &File, method: &Method, call: &Call) -> Option<String> {
        let mut types = BTreeSet::new();
        let typed_receiver = match call
            .constructed_type
            .as_deref()
            .or(call.receiver.as_deref())
        {
            None | Some("this") => {
                // Inherited Object methods also precede static imports, even
                // when the class has no explicit superclass in the source.
                let object_method = matches!(
                    call.name.as_str(),
                    "getClass"
                        | "hashCode"
                        | "equals"
                        | "clone"
                        | "toString"
                        | "notify"
                        | "notifyAll"
                        | "wait"
                        | "finalize"
                );
                let mut scope = Some(method.declaring_type.as_str());
                let mut local_method = None;
                let mut inherited = false;
                while let Some(current) = scope {
                    if file.source.methods.iter().any(|candidate| {
                        candidate.declaring_type == current && candidate.name == call.name
                    }) {
                        local_method = Some(current);
                        break;
                    }
                    inherited |= file.source.inherited_types.contains(current);
                    if call.receiver.is_some() || inherited || object_method {
                        break;
                    }
                    scope = current.rsplit_once('.').map(|(parent, _)| parent);
                }
                types.insert(qualified(
                    &file.source.package,
                    local_method.unwrap_or(&method.declaring_type),
                ));
                let explicit_static = file.source.imports.iter().any(|import| {
                    matches!(import, ImportDecl::StaticMember(name) if name.rsplit('.').next() == Some(call.name.as_str()))
                });
                for import in file.source.imports.iter().filter(|_| {
                    call.receiver.is_none()
                        && local_method.is_none()
                        && !inherited
                        && !object_method
                }) {
                    match import {
                        ImportDecl::StaticMember(name) => {
                            if let Some((ty, member)) = name.rsplit_once('.')
                                && member == call.name
                            {
                                types.insert(ty.into());
                            }
                        }
                        ImportDecl::StaticWildcard(ty) if !explicit_static => {
                            types.insert(ty.clone());
                        }
                        _ => {}
                    }
                }
                false
            }
            Some(receiver) => {
                if call.constructed_type.is_none()
                    && file
                        .source
                        .value_names
                        .contains(receiver.split('.').next()?)
                {
                    return None;
                }
                if !receiver
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '$'))
                {
                    return None;
                }
                types.insert(receiver.into());
                types.insert(qualified(&file.source.package, receiver));
                let mut explicit = BTreeSet::new();
                for import in &file.source.imports {
                    match import {
                        ImportDecl::Type(name) => {
                            let simple = name.rsplit('.').next()?;
                            if receiver == simple {
                                explicit.insert(name.clone());
                            } else if let Some(nested) =
                                receiver.strip_prefix(&format!("{simple}."))
                            {
                                explicit.insert(format!("{name}.{nested}"));
                            }
                        }
                        ImportDecl::Wildcard(name) => {
                            types.insert(format!("{name}.{receiver}"));
                        }
                        _ => {}
                    }
                }
                if !explicit.is_empty() {
                    types = explicit;
                }
                // Lexically declared member types precede imported names.
                let mut enclosing = Some(method.declaring_type.as_str());
                let mut local_type = None;
                while let Some(scope) = enclosing {
                    let candidate = format!("{scope}.{receiver}");
                    if file.source.types.contains(&candidate) {
                        local_type = Some(candidate);
                        break;
                    }
                    enclosing = scope.rsplit_once('.').map(|(parent, _)| parent);
                }
                if local_type.is_none() && file.source.types.iter().any(|ty| ty == receiver) {
                    local_type = Some(receiver.into());
                }
                if let Some(local) = local_type {
                    types = BTreeSet::from([qualified(&file.source.package, &local)]);
                }
                true
            }
        };
        // Resolve receiver types before looking for matching methods. Two
        // visible types stay ambiguous even if only one declares this method.
        let mut resolved_types = Vec::new();
        for ty in types {
            if self.index.resolve(Import::Type(&ty), &file.owner, |owner| {
                file.visible.contains(&owner.file)
            }) == Resolution::Unresolved
            {
                return None;
            }
            if let Some(owner) = self
                .index
                .type_owner(&ty, &file.owner, |owner| file.visible.contains(&owner.file))
            {
                resolved_types.push((ty, owner));
            }
        }
        if typed_receiver && resolved_types.len() != 1 {
            return None;
        }
        let mut candidates = Vec::new();
        for (ty, owner) in resolved_types {
            let target = self.files.get(&owner.file)?;
            let type_name = if target.source.package.is_empty() {
                ty.as_str()
            } else {
                ty.strip_prefix(&format!("{}.", target.source.package))?
            };
            let methods = target
                .source
                .methods
                .iter()
                .filter(|method| method.declaring_type == type_name && method.name == call.name)
                .collect::<Vec<_>>();
            // V1 identities collapse overloads. Never convert that loss of
            // precision into coverage for another overload sharing the id.
            if methods.len() > 1
                && methods
                    .iter()
                    .any(|candidate| candidate.arity == call.arity)
            {
                return None;
            }
            if methods.len() != 1 {
                continue;
            }
            let callee = methods[0];
            let same_class =
                file.owner.file == owner.file && method.declaring_type == callee.declaring_type;
            if callee.arity == call.arity
                && ((!typed_receiver && same_class)
                    || callee.is_static
                    || call.constructed_type.is_some())
            {
                candidates.push(function_id(&owner, callee));
            }
        }
        candidates.sort();
        candidates.dedup();
        if candidates.len() == 1 {
            candidates.pop()
        } else {
            None
        }
    }
}
