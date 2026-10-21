// Go starter pack adapter.
//
// `context` is this file's `FileContext` (its `go.mod`, if any, is the sole
// declared context file — see pack.json). `matches` holds every capture
// from declarations.scm / calls.scm / imports.scm, grouped by match (one
// match per query-pattern occurrence) and tagged with its query id.
//
// This script owns every Go-specific interpretation: scope naming from the
// nearest `go.mod`, pointer/value/generic receiver normalization, shadow
// suppression for same-scope call resolution, and `testing.T` recognition
// (including import aliases). It never touches the filesystem, a process,
// or the network — only the `context`/`matches` constants it was given.
//
// Logic is split into many small functions rather than nested if/else
// chains: Rhai's sandbox enforces a parse-time expression-depth limit
// (debug builds default to 32), and a function call resets that depth
// count for its own body, where a deeply nested branch inline would not.

// ── small capture/string helpers ───────────────────────────────────────

fn caps_named(m, name) {
    let out = [];
    for c in m.captures {
        if c.name == name {
            out.push(c);
        }
    }
    out
}

fn cap1(m, name) {
    let all = caps_named(m, name);
    if all.len() > 0 {
        all[0]
    } else {
        ()
    }
}

fn has_cap(m, name) {
    for c in m.captures {
        if c.name == name {
            return true;
        }
    }
    false
}

fn dirname(path) {
    let idx = -1;
    let i = 0;
    for ch in path {
        if ch == '/' {
            idx = i;
        }
        i += 1;
    }
    if idx <= 0 {
        "."
    } else {
        path.sub_string(0, idx)
    }
}

fn strip_quotes(s) {
    if s.len() >= 2 {
        let first = s[0];
        let last = s[s.len() - 1];
        if (first == '"' && last == '"') || (first == '`' && last == '`') {
            return s.sub_string(1, s.len() - 2);
        }
    }
    s
}

fn is_lower(ch) {
    is_unicode_lowercase(ch)
}

fn find_context_file(ctx, name) {
    for cf in ctx.context_files {
        if cf.name == name {
            return cf;
        }
    }
    ()
}

// The directory recorded by the host for a context file: empty string and
// "." both mean "repository root" (the exact empty-vs-dot spelling is a
// host detail this adapter does not depend on).
fn norm_dir(d) {
    if d == "" {
        "."
    } else {
        d
    }
}

fn parse_module_path(content) {
    for line in content.split("\n") {
        let t = line;
        t.trim();
        if t.starts_with("module ") {
            let module_path = t.sub_string(7);
            module_path.trim();
            return module_path;
        }
    }
    ""
}

// `target_dir` relative to `base_dir`, both posix and both either "." or a
// slash-separated repository-relative path. Returns "." when they are the
// same directory. `target_dir` is assumed to be `base_dir` or a descendant
// of it (true for every file under the module directory that supplied
// `base_dir`, since the host only ever attaches the *nearest* `go.mod`).
fn rel_under(base_dir, target_dir) {
    let b = norm_dir(base_dir);
    let t = norm_dir(target_dir);
    if t == b {
        return ".";
    }
    if b == "." {
        return t;
    }
    let prefix = b + "/";
    if t.len() > prefix.len() && t.sub_string(0, prefix.len()) == prefix {
        return t.sub_string(prefix.len());
    }
    // Not actually nested under `base_dir` — deterministic fallback rather
    // than fabricating a relationship.
    t
}

fn base_type_name(text) {
    let t = text;
    if t.len() > 0 && t[0] == '*' {
        t = t.sub_string(1);
    }
    let bracket = t.index_of("[");
    if bracket >= 0 {
        t = t.sub_string(0, bracket);
    }
    t
}

fn import_effective_name_default(path) {
    let segments = path.split("/");
    segments[segments.len() - 1]
}

// ── scope / package naming ─────────────────────────────────────────────

// Every record in this file shares one package scope: `<pkg_path>::<pkg_name>`.
// `pkg_path` is `<module>/<dir-under-module>` when a `go.mod` was found
// (nested modules each resolve against their own nearest `go.mod`), or a
// deterministic `local/<file-dir>` fallback when none was. Including the
// declared package name (not just the directory) keeps an external test
// package (`package foo_test`) distinct from the main package in the same
// directory (`package foo`).
fn module_pkg_path(gomod, file_dir) {
    let module_path = parse_module_path(gomod.content);
    if module_path == "" {
        return "local/" + file_dir;
    }
    let rel = rel_under(gomod.directory_rel, file_dir);
    if rel == "." {
        module_path
    } else {
        module_path + "/" + rel
    }
}

fn package_scope(ctx, pkg_name) {
    let file_dir = dirname(ctx.file_rel);
    let gomod = find_context_file(ctx, "go.mod");
    let pkg_path = if gomod == () { "local/" + file_dir } else { module_pkg_path(gomod, file_dir) };
    pkg_path + "::" + pkg_name
}

fn test_module_dir(ctx) {
    let gomod = find_context_file(ctx, "go.mod");
    if gomod == () { "." } else { norm_dir(gomod.directory_rel) }
}

fn test_package_arg(ctx) {
    let rel = rel_under(test_module_dir(ctx), dirname(ctx.file_rel));
    if rel == "." { "." } else { "./" + rel }
}

// ── go vet's `TestXxx` naming rule ─────────────────────────────────────
//
// `go vet` (and `go test`) only treat `TestXxx` as a test when the rune
// right after `Test` is not a lowercase letter — `Testhelper` is a plain
// function, `TestHelper` / `Test_foo` / `Test1` are tests.
fn looks_like_test_name(name) {
    if !name.starts_with("Test") {
        return false;
    }
    name.len() == 4 || !is_lower(name[4])
}

// ── record builders (kept flat on purpose) ──────────────────────────────

fn dot_import_fact(path, span) {
    #{ "kind": "fact", "name": "dot_import", "args": [path], "span": span }
}

fn import_record(scope, path, alias, span) {
    let rec = #{ "kind": "import", "scope": scope, "target": path, "span": span };
    if alias != () {
        rec.alias = alias;
    }
    rec
}

fn test_command(name, package_arg, module_dir) {
    #{ "program": "go", "args": ["test", package_arg, "-run", "^" + name + "$"], "cwd": module_dir }
}

fn declaration_record(name, scope, span, test, receiver, command) {
    let rec = #{ "kind": "declaration", "name": name, "scope": scope, "span": span, "test": test };
    if receiver != () {
        rec.receiver = receiver;
    }
    if command != () {
        rec.command = command;
    }
    rec
}

fn reference_record(owner_start_byte, target, scope, resolution, span) {
    #{
        "kind": "reference",
        "owner_start_byte": owner_start_byte,
        "target": target,
        "scope": scope,
        "resolution": resolution,
        "span": span,
    }
}

// ── imports ───────────────────────────────────────────────────────────

// `#{ alias, effective, is_dot }`: `alias` is the raw written spelling
// (`.`, `_`, a name) or unit for a plain import; `effective` is the local
// identifier that could shadow an unqualified call or supply `testing.T`.
fn import_name_info(name_cap, path) {
    if name_cap == () {
        return #{ "alias": (), "effective": import_effective_name_default(path), "is_dot": false };
    }
    let text = name_cap.text;
    if text == "." {
        return #{ "alias": text, "effective": "", "is_dot": true };
    }
    if text == "_" {
        return #{ "alias": text, "effective": "", "is_dot": false };
    }
    #{ "alias": text, "effective": text, "is_dot": false }
}

// Returns unit when this match has no usable path capture, otherwise a
// `#{ record, dot_fact, effective, path }` the caller folds into its own
// `records`/`alias_to_path`/`has_dot_import` — Rhai passes maps and arrays
// to a plain function call *by value*, so a helper cannot mutate a caller's
// collection through a parameter; only the top-level loop's own locals are
// mutated, via direct `.push`/index-assignment.
fn import_outcome(m, scope) {
    let path_cap = cap1(m, "import_path");
    if path_cap == () {
        return ();
    }
    let path = strip_quotes(path_cap.text);
    let info = import_name_info(cap1(m, "import_name"), path);
    let dot_fact = if info.is_dot { dot_import_fact(path, m.captures[0].span) } else { () };
    #{
        "record": import_record(scope, path, info.alias, path_cap.span),
        "dot_fact": dot_fact,
        "effective": info.effective,
        "path": path,
    }
}

// ── declarations ─────────────────────────────────────────────────────

fn collect_names(m, cap_name) {
    let names = [];
    for c in caps_named(m, cap_name) {
        if c.text != "_" {
            names.push(c.text);
        }
    }
    names
}

fn collect_texts(m, cap_name) {
    let texts = [];
    for c in caps_named(m, cap_name) {
        texts.push(c.text);
    }
    texts
}

fn func_decl_info(m) {
    #{
        "name": cap1(m, "func_name").text,
        "span": cap1(m, "func_decl").span,
        "is_free": true,
        "param_names": collect_names(m, "func_param_name"),
        "param_types": collect_texts(m, "func_param_type"),
        "has_type_params": has_cap(m, "func_type_params"),
        "has_result": has_cap(m, "func_result"),
        "receiver": (),
    }
}

fn method_decl_info(m) {
    let recv_cap = cap1(m, "method_receiver_type");
    let receiver = if recv_cap == () { () } else { base_type_name(recv_cap.text) };
    #{
        "name": cap1(m, "method_name").text,
        "span": cap1(m, "method_decl").span,
        "is_free": false,
        "param_names": collect_names(m, "method_param_name"),
        "param_types": [],
        "receiver": receiver,
    }
}

fn local_site_info(m) {
    #{ "names": collect_names(m, "local_name"), "start_byte": m.captures[0].span.start_byte }
}

// `#{ test, command }`: recognizes a standard top-level `TestXxx(t *X.T)`
// signature in a `_test.go` file, including an aliased `testing` import.
fn test_info(d, is_test_file, alias_to_path, package_arg, module_dir) {
    let no_test = #{ "test": false, "command": () };
    if !d.is_free || !is_test_file || !looks_like_test_name(d.name) {
        return no_test;
    }
    if d.has_type_params || d.has_result || d.param_names.len() > 1 || d.param_types.len() != 1 {
        return no_test;
    }
    let t = d.param_types[0];
    if t.len() == 0 || t[0] != '*' {
        return no_test;
    }
    let inner = t.sub_string(1);
    let dot = inner.index_of(".");
    if dot <= 0 {
        return no_test;
    }
    let alias = inner.sub_string(0, dot);
    let member = inner.sub_string(dot + 1);
    if member != "T" || alias_to_path.get(alias) != "testing" {
        return no_test;
    }
    #{ "test": true, "command": test_command(d.name, package_arg, module_dir) }
}

// ── calls ─────────────────────────────────────────────────────────────

// Smallest declaration whose span contains `start_byte` (methods/functions
// never nest in Go; "smallest" is a defensive tie-break, not a requirement).
fn find_owner(decls, start_byte) {
    let owner = ();
    for d in decls {
        if start_byte < d.span.start_byte || start_byte >= d.span.end_byte {
            continue;
        }
        if owner == () {
            owner = d;
        } else if (d.span.end_byte - d.span.start_byte) < (owner.span.end_byte - owner.span.start_byte) {
            owner = d;
        }
    }
    owner
}

fn shadowed_by_param(owner, target) {
    for p in owner.param_names {
        if p == target {
            return true;
        }
    }
    false
}

fn shadowed_by_local(owner, target, local_sites) {
    for site in local_sites {
        if site.start_byte < owner.span.start_byte || site.start_byte >= owner.span.end_byte {
            continue;
        }
        for n in site.names {
            if n == target {
                return true;
            }
        }
    }
    false
}

// Conservative shadow suppression: a dot import makes every unqualified
// name in the file ambiguous (it could have come from the dot-imported
// package); `init`/`_` are never resolvable call targets.
fn is_shadowed(owner, target, local_sites, has_dot_import) {
    if has_dot_import || target == "_" || target == "init" {
        return true;
    }
    shadowed_by_param(owner, target) || shadowed_by_local(owner, target, local_sites)
}

// Returns unit when the call site has no enclosing declaration (nothing to
// attribute an owner to), otherwise the reference record.
fn call_reference(m, decls, local_sites, scope, has_dot_import) {
    let site_span = m.captures[0].span;
    let owner = find_owner(decls, site_span.start_byte);
    if owner == () {
        return ();
    }
    let receiver_cap = cap1(m, "call_receiver");
    let target;
    let resolution;
    if receiver_cap != () {
        target = receiver_cap.text + "." + cap1(m, "call_field").text;
        resolution = "unresolved";
    } else {
        target = cap1(m, "call_name").text;
        resolution = if is_shadowed(owner, target, local_sites, has_dot_import) { "unresolved" } else { "same_scope" };
    }
    reference_record(owner.span.start_byte, target, scope, resolution, site_span)
}

// ── main extraction ─────────────────────────────────────────────────────

fn package_name_of(matches) {
    for m in matches {
        if m.query == "declarations" {
            let pkg_cap = cap1(m, "pkg_name");
            if pkg_cap != () {
                return pkg_cap.text;
            }
        }
    }
    "main"
}

// Each pass is its own top-level function (not an inline block of
// `extract_go`) so a parse-time depth limit is spent per pass rather than
// accumulating across all four in one function body.

fn imports_pass(matches, scope) {
    let records = [];
    let alias_to_path = #{};
    let has_dot_import = false;
    for m in matches {
        if m.query != "imports" {
            continue;
        }
        let outcome = import_outcome(m, scope);
        if outcome == () {
            continue;
        }
        records.push(outcome.record);
        if outcome.dot_fact != () {
            records.push(outcome.dot_fact);
            has_dot_import = true;
        }
        if outcome.effective != "" {
            alias_to_path[outcome.effective] = outcome.path;
        }
    }
    #{ "records": records, "alias_to_path": alias_to_path, "has_dot_import": has_dot_import }
}

fn declarations_pass(matches) {
    let decls = [];
    let local_sites = [];
    for m in matches {
        if m.query != "declarations" {
            continue;
        }
        if has_cap(m, "func_decl") {
            decls.push(func_decl_info(m));
        } else if has_cap(m, "method_decl") {
            decls.push(method_decl_info(m));
        } else if has_cap(m, "local_name") {
            let site = local_site_info(m);
            if site.names.len() > 0 {
                local_sites.push(site);
            }
        }
    }
    #{ "decls": decls, "local_sites": local_sites }
}

fn declaration_records_pass(decls, scope, is_test_file, alias_to_path, package_arg, module_dir) {
    let records = [];
    for d in decls {
        let info = test_info(d, is_test_file, alias_to_path, package_arg, module_dir);
        records.push(declaration_record(d.name, scope, d.span, info.test, d.receiver, info.command));
    }
    records
}

fn calls_pass(matches, decls, local_sites, scope, has_dot_import) {
    let records = [];
    for m in matches {
        if m.query != "calls" {
            continue;
        }
        let rec = call_reference(m, decls, local_sites, scope, has_dot_import);
        if rec != () {
            records.push(rec);
        }
    }
    records
}

fn extract_go(context, matches) {
    let scope = package_scope(context, package_name_of(matches));
    let is_test_file = context.file_rel.ends_with("_test.go");
    let module_dir = test_module_dir(context);
    let package_arg = test_package_arg(context);

    let imports = imports_pass(matches, scope);
    let parsed = declarations_pass(matches);
    let decl_records = declaration_records_pass(
        parsed.decls, scope, is_test_file, imports.alias_to_path, package_arg, module_dir
    );
    let call_records = calls_pass(matches, parsed.decls, parsed.local_sites, scope, imports.has_dot_import);

    // `+` on two arrays returns their concatenation (a new array) — no
    // helper function needed, and no by-value-parameter mutation pitfall.
    imports.records + decl_records + call_records
}

extract_go(context, matches)
