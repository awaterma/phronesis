//! End-to-end: a rule using `function_returns_result_string` fires when the
//! hook sees an edit that adds such a function.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn write_rules_file(dir: &Path, contents: &str) {
    let ep = dir.join(".phronesis");
    std::fs::create_dir_all(&ep).unwrap();
    std::fs::write(ep.join("rules.json"), contents).unwrap();
}

fn run_hook_with_root(payload: &str, root: &Path) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.arg("pre-check")
        .env("PHRONESIS_PROJECT_ROOT", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(payload.as_bytes());
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn init_pack(root: &Path, pack: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["init", "--packs", pack])
        .current_dir(root)
        .output()
        .expect("spawn init");
    assert!(
        out.status.success(),
        "init stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn edit_payload(path: &str, new_content: &str) -> String {
    serde_json::json!({
        "tool_name": "Write",
        "tool_input": {
            "file_path": path,
            "content": new_content,
        }
    })
    .to_string()
}

#[test]
fn generated_typescript_pack_warns_once_for_a_real_explicit_any() {
    let dir = tempfile::tempdir().unwrap();
    init_pack(dir.path(), "typescript");

    let payload = edit_payload(
        "src/service.ts",
        "export const load = (value: any) => value;\n",
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());

    assert_eq!(code, 1, "explicit any should warn: {stderr}");
    assert!(
        stderr.contains("explicit `any` annotation"),
        "stderr: {stderr}"
    );
    assert_eq!(
        stderr.matches("explicit `any` annotation").count(),
        1,
        "the retired lexical rule must not duplicate the structural warning: {stderr}"
    );
}

#[test]
fn generated_typescript_pack_ignores_any_text_in_comments_and_strings() {
    let dir = tempfile::tempdir().unwrap();
    init_pack(dir.path(), "typescript");
    let payload = edit_payload(
        "src/service.ts",
        "// example annotation: any\nexport const note = \": any\";\n",
    );

    let (code, stderr) = run_hook_with_root(&payload, dir.path());

    assert_eq!(code, 0, "lexical lookalikes must not warn: {stderr}");
}

#[test]
fn generated_typescript_pack_matches_console_log_structurally() {
    let dir = tempfile::tempdir().unwrap();
    init_pack(dir.path(), "typescript");
    let payload = edit_payload(
        "src/service.ts",
        "export function run() { console . log (\"ready\"); }\n",
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 1, "console.log should warn: {stderr}");
    assert!(stderr.contains("console.log"), "stderr: {stderr}");

    let payload = edit_payload(
        "src/service.ts",
        "// console.log(\"comment\")\nconst note = \"console.log(fake)\";\nlogger.console.log(\"nested\");\n",
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 0, "lookalikes must not warn: {stderr}");
}

#[test]
fn generated_swift_pack_matches_precheck_constructs_structurally() {
    let dir = tempfile::tempdir().unwrap();
    init_pack(dir.path(), "swift");
    let payload = edit_payload(
        "Sources/App.swift",
        "func load(value: Any) { let _ = try! fetch(); let _ = value as! String }\n",
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 1, "Swift force operations should warn: {stderr}");
    assert!(stderr.contains("try!"), "stderr: {stderr}");
    assert!(stderr.contains("as!"), "stderr: {stderr}");

    let payload = edit_payload(
        "Sources/App.swift",
        "// try! as!\nlet note = \"fatalError( CGRectMake( arc4random(\"\nfunc safe() {}\n",
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 0, "Swift lexical lookalikes must not warn: {stderr}");
}

#[test]
fn generated_swift_structural_rules_participate_in_whole_tree_audit() {
    let dir = tempfile::tempdir().unwrap();
    init_pack(dir.path(), "swift");
    std::fs::create_dir_all(dir.path().join("Sources")).unwrap();
    std::fs::write(
        dir.path().join("Sources/App.swift"),
        r#"
func load(value: Any) {
    let _ = try! fetch()
    let _ = value as! String
    fatalError("unreachable")
    CGRectMake(0, 0, 1, 1)
    arc4random_uniform(10)
}
final class Service { static var shared = Service() }
"#,
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["audit", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("spawn audit");
    assert!(
        out.status.success(),
        "audit stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    for rule_id in [
        "audit-swift-fatal-error",
        "audit-swift-mutable-singleton",
        "audit-swift-legacy-constructor",
        "audit-swift-legacy-random",
    ] {
        assert!(stdout.contains(rule_id), "missing {rule_id}: {stdout}");
        let rule = report["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rule| rule["rule_id"] == rule_id)
            .unwrap();
        assert_eq!(
            rule["hits"], 1,
            "literal construct arguments must filter sibling facts: {rule}"
        );
    }
}

#[test]
fn generated_rust_pack_matches_structural_invocation_variants_through_hook() {
    let cases = [
        ("Some(1).unwrap ( )", "Avoid .unwrap()"),
        ("todo!(\"later\")", "Don't ship todo!()"),
        ("std::panic!(\"boom\")", "Avoid panic!()"),
        ("unimplemented! { \"later\" }", "Avoid unimplemented!()"),
        ("dbg!(1)", "dbg!() in src/"),
        ("Some(1).expect(\"\")", ".expect(\"\")"),
    ];

    for (expression, expected_message) in cases {
        let dir = tempfile::tempdir().unwrap();
        init_pack(dir.path(), "rust");
        let payload = edit_payload(
            "src/lib.rs",
            &format!("fn demo() {{ let _ = {expression}; }}\n"),
        );
        let (code, stderr) = run_hook_with_root(&payload, dir.path());
        assert_ne!(code, 0, "{expression} should fire a rule: {stderr}");
        assert!(
            stderr.contains(expected_message),
            "{expression} should produce its packaged message: {stderr}"
        );
    }
}

#[test]
fn generated_rust_pack_ignores_invocation_text_in_comments_and_strings() {
    let dir = tempfile::tempdir().unwrap();
    init_pack(dir.path(), "rust");
    let payload = edit_payload(
        "src/lib.rs",
        r#"
fn safe(value: Option<u8>) {
    // value.unwrap(); todo!(); panic!("x"); unimplemented!(); dbg!(value);
    let _ = ".unwrap() todo!() panic!( unimplemented!() dbg!( .expect(\"\")";
    let _ = value.expect("documented invariant");
}
"#,
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 0, "lexical lookalikes must not fire: {stderr}");
}

#[test]
fn generated_rust_structural_rules_participate_in_whole_tree_audit() {
    let dir = tempfile::tempdir().unwrap();
    init_pack(dir.path(), "rust");
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        r#"
#![deny(warnings)]
fn demo(value: Option<u8>) {
    let _ = value.unwrap();
    todo!("later");
    panic!("boom");
    unimplemented!("later");
    dbg!(value);
    let _ = value.expect("");
    std::env::set_var("KEY", "value");
    match value { None => { }, Some(_) => {} }
    let result: Result<u8, E> = todo!();
    match result { Err(_) => { }, Ok(_) => {} }
    match result { Err(error) => return Err(error), Ok(_) => {} }
}
impl std::ops::Deref for Wrapper {
    type Target = u8;
    fn deref(&self) -> &u8 { &0 }
}
struct Stored { user_id: u64, shared: Rc < RefCell <u8> > }
"#,
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["audit", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("spawn audit");
    assert!(
        out.status.success(),
        "audit stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for rule_id in [
        "enforce-no-unwrap-in-src",
        "enforce-no-todo-in-src",
        "enforce-no-panic-in-src",
        "enforce-no-unimplemented-in-src",
        "warn-dbg-in-src",
        "warn-expect-with-empty-message",
        "audit-env-set-var-in-src",
        "block-deny-warnings-attribute",
        "warn-deref-for-non-pointer-type",
        "audit-manual-err-return",
        "audit-if-let-opportunity-none-empty",
        "audit-if-let-opportunity-err-empty",
        "audit-newtype-id-u64",
        "audit-rc-refcell-in-src",
    ] {
        assert!(stdout.contains(rule_id), "missing {rule_id}: {stdout}");
    }
}

#[test]
fn generated_rust_pack_matches_box_ref_parameter_structurally() {
    let dir = tempfile::tempdir().unwrap();
    init_pack(dir.path(), "rust");
    let payload = edit_payload("src/lib.rs", "fn consume(value: & Box <u8>) {}\n");
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 1, "&Box<T> should warn: {stderr}");
    assert!(stderr.contains("&Box<T>"), "stderr: {stderr}");

    let payload = edit_payload(
        "src/lib.rs",
        "fn safe(value: &u8) { let _ = \": &Box<Fake>\"; }\n",
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 0, "string lookalike must not warn: {stderr}");
}

#[test]
fn generated_rust_pack_matches_deny_warnings_attribute_structurally() {
    let dir = tempfile::tempdir().unwrap();
    init_pack(dir.path(), "rust");
    let payload = edit_payload("src/lib.rs", "#![ deny ( warnings ) ]\nfn safe() {}\n");
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 2, "deny(warnings) should block: {stderr}");
    assert!(stderr.contains("deny(warnings)"), "stderr: {stderr}");

    let payload = edit_payload(
        "src/lib.rs",
        "// #![deny(warnings)]\nconst NOTE: &str = \"#![deny(warnings)]\";\n",
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 0, "lexical lookalikes must not block: {stderr}");
}

#[test]
fn generated_rust_pack_blocks_panic_in_drop_impl_through_hook() {
    let dir = tempfile::tempdir().unwrap();
    init_pack(dir.path(), "rust");
    // `.expect("")` is only a warn outside a Drop impl, so exit 2 here can
    // only come from the Drop-specific block rule.
    let payload = edit_payload(
        "src/lib.rs",
        "struct Conn;\nimpl Drop for Conn { fn drop(&mut self) { self.close().expect(\"\"); } }\n",
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(
        code, 2,
        "panic construct in Drop::drop should block: {stderr}"
    );
    assert!(
        stderr.contains("aborts the whole process"),
        "stderr: {stderr}"
    );

    let payload = edit_payload(
        "src/lib.rs",
        "// impl Drop for Fake { fn drop(&mut self) { panic!(\"x\") } }\nconst NOTE: &str = \"impl Drop for Fake\";\n",
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 0, "lexical lookalikes must not block: {stderr}");
}

#[test]
fn generated_rust_pack_matches_deref_impl_structurally() {
    let dir = tempfile::tempdir().unwrap();
    init_pack(dir.path(), "rust");
    let payload = edit_payload(
        "src/lib.rs",
        "impl std::ops::Deref for Wrapper { type Target = u8; fn deref(&self) -> &u8 { &0 } }\n",
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 1, "Deref impl should warn: {stderr}");
    assert!(stderr.contains("Deref polymorphism"), "stderr: {stderr}");

    let payload = edit_payload(
        "src/lib.rs",
        "// impl Deref for Fake {}\nconst NOTE: &str = \"impl Deref for Fake\";\n",
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 0, "lexical lookalikes must not warn: {stderr}");
}

#[test]
fn result_string_rule_blocks_offending_function() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn existing() -> u32 { 0 }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"no-string-error","phase":"pre","priority":10,
            "when":[
                {"function_returns_result_string":["?file","?fn"]}
            ],
            "then":{"block":"Function `?fn` in ?file uses Result<_, String>. Define a thiserror enum."}
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "fn existing() -> u32 { 0 }",
            "new_string": "fn existing() -> u32 { 0 }\nfn bad() -> Result<u32, String> { Ok(0) }"
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2, "should block: {}", stderr);
    assert!(stderr.contains("bad"), "stderr: {}", stderr);
    assert!(stderr.contains("thiserror"), "stderr: {}", stderr);
}

#[test]
fn result_string_rule_allows_proper_error_type() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn existing() -> u32 { 0 }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"no-string-error","phase":"pre","priority":10,
            "when":[
                {"function_returns_result_string":["?file","?fn"]}
            ],
            "then":{"block":"bad: ?fn"}
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "fn existing() -> u32 { 0 }",
            "new_string": "fn existing() -> u32 { 0 }\nfn ok() -> Result<u32, MyError> { Ok(0) }"
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 0, "should allow: {}", stderr);
}

#[test]
fn python_phase_one_predicates_fire_through_hook() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/service.py"),
        "def existing():\n    pass\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[
            {"id":"python-print","phase":"pre","priority":5,
             "when":[{"python_print_call":["?file","?fn"]}],
             "then":{"warn":"python print call"}},
            {"id":"python-call-default","phase":"pre","priority":5,
             "when":[{"python_call_in_default_arg":["?file","?fn","?param","?callee"]}],
             "then":{"warn":"python call default"}},
            {"id":"python-handler-pass","phase":"pre","priority":5,
             "when":[{"python_exception_handler_passes":["?file","?fn","?exception"]}],
             "then":{"warn":"python handler pass"}}
        ]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/service.py",
            "old_string": "def existing():\n    pass\n",
            "new_string": "def build(value=load_default()):\n    print(value)\n    try:\n        work()\n    except ValueError:\n        pass\n"
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning rules should return one: {stderr}");
    assert!(stderr.contains("python print call"), "stderr: {stderr}");
    assert!(stderr.contains("python call default"), "stderr: {stderr}");
    assert!(stderr.contains("python handler pass"), "stderr: {stderr}");
}

#[test]
fn python_patterns_guide_predicates_fire_through_hook() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/service.py"), "VALUE = 1\n").unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[
            {"id":"import-io","phase":"pre","priority":5,"when":[{"python_import_time_io":["?file","?callee"]}],"then":{"warn":"import io"}},
            {"id":"is-literal","phase":"pre","priority":5,"when":[{"python_is_literal_comparison":["?file","?fn"]}],"then":{"warn":"is literal"}},
            {"id":"mutable-global","phase":"pre","priority":5,"when":[{"python_mutated_module_global":["?file","?fn","?global"]}],"then":{"warn":"mutable global"}},
            {"id":"star-import","phase":"pre","priority":5,"when":[{"python_star_import":["?file","?module"]}],"then":{"warn":"star import"}}
        ]}"#,
    );
    let payload = r#"{
        "tool_name":"Write",
        "tool_input":{
            "file_path":"src/service.py",
            "content":"from tools import *\nCACHE = {}\nCONFIG = open('config.json')\ndef remember(x):\n    CACHE.update({x: True})\n    return x is 'ready'\n"
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning rules should return one: {stderr}");
    for message in ["import io", "is literal", "mutable global", "star import"] {
        assert!(stderr.contains(message), "missing {message}: {stderr}");
    }
}

const PYTHON_PATTERNS_FIXTURE: &str = r#"
_seed = 42

def set_seed(v):
    global _seed
    _seed = v

for name in dir(_seed):
    globals()[name] = getattr(_seed, name)

def build(name, a, b):
    return type(name, (a, b), {})

class Logger:
    _instance = None
    def __new__(cls):
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

class Grade:
    def __new__(cls, percent):
        return super().__new__(cls)

def render(w):
    if isinstance(w, Frame):
        return 1
    elif isinstance(w, Label):
        return 2
    return 0

class Bag:
    items = []
    def __len__(self):
        return 0
    def __iter__(self):
        return self
    def __next__(self):
        raise StopIteration

class FilteredSocketLogger(FilteredLogger, SocketLogger):
    pass

class A:
    pass
class B(A):
    pass
class C(B):
    pass

class FilterMixin:
    def __init__(self):
        self.pattern = ''

class Wrapper:
    def read(self, n):
        return self._file.read(n)
    def write(self, s):
        return self._file.write(s)
    def close(self):
        return self._file.close()
    def flush(self):
        return self._file.flush()

def check(x):
    return x == None
"#;

const PYTHON_PATTERNS_RULE_IDS: &[&str] = &[
    "warn-python-global-statement",
    "warn-python-globals-introspection-assignment",
    "warn-python-dynamic-class-creation",
    "warn-python-singleton-new",
    "audit-python-custom-new",
    "audit-python-isinstance-dispatch",
    "warn-python-container-is-own-iterator",
    "warn-python-multiple-inheritance",
    "audit-python-deep-inheritance",
    "warn-python-mixin-with-init",
    "warn-python-static-delegation-wrapper",
    "warn-python-mutable-class-attribute",
    "warn-python-equality-with-none",
];

#[test]
fn generated_python_patterns_pack_loads_and_fires_through_hook_and_audit() {
    let dir = tempfile::tempdir().unwrap();
    init_pack(dir.path(), "python,python-patterns");
    let rules: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    let ids: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    for id in PYTHON_PATTERNS_RULE_IDS {
        assert!(ids.contains(id), "rules.json missing {id}");
    }
    // Hard constraint: no substring/regex conditions anywhere in the Python packs.
    for rule in rules["rules"].as_array().unwrap() {
        let id = rule["id"].as_str().unwrap_or("");
        if !id.contains("python") {
            continue;
        }
        for cond in rule["when"].as_array().unwrap() {
            for key in cond.as_object().unwrap().keys() {
                assert!(
                    key.starts_with("python_") || key == "file_path_matches",
                    "{id} uses non-structural condition {key}"
                );
            }
        }
    }

    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    // One distinctive fragment per `warn` (pre-phase) rule; the hook prints
    // messages, not rule IDs.
    let pre_fragments = [
        "rebinds module global `_seed`",
        "assigns through `globals()[...]`",
        "`build` in src/patterns.py builds a class at runtime",
        "Class `Logger` in src/patterns.py implements the Singleton Pattern",
        "Container class `Bag`",
        "`FilteredSocketLogger` in src/patterns.py inherits from 2 concrete classes",
        "Mixin `FilterMixin`",
        "`Wrapper` in src/patterns.py re-declares 4 methods",
        "assigns mutable container `items`",
        "`check` in src/patterns.py compares against `None`",
    ];
    let payload = edit_payload("src/patterns.py", PYTHON_PATTERNS_FIXTURE);
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 1, "warn rules should return one: {stderr}");
    assert!(
        stderr.contains("https://python-patterns.guide/"),
        "messages cite the guide: {stderr}"
    );
    for fragment in pre_fragments {
        assert!(
            stderr.contains(fragment),
            "hook missing {fragment}: {stderr}"
        );
    }
    // Bindings substituted: the delegation wrapper names its attribute.
    assert!(
        stderr.contains("self._file"),
        "?attr binding not substituted: {stderr}"
    );

    std::fs::write(dir.path().join("src/patterns.py"), PYTHON_PATTERNS_FIXTURE).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["audit", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("spawn audit");
    assert!(
        out.status.success(),
        "audit stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    for rule_id in PYTHON_PATTERNS_RULE_IDS {
        let rule = report["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rule| rule["rule_id"] == *rule_id)
            .unwrap_or_else(|| panic!("audit missing {rule_id}: {}", report));
        assert!(
            rule["hits"].as_u64().unwrap_or(0) >= 1,
            "{rule_id} should hit the fixture: {rule}"
        );
    }
}

#[test]
fn generated_python_patterns_pack_is_silent_on_idiomatic_code() {
    let dir = tempfile::tempdir().unwrap();
    init_pack(dir.path(), "python-patterns");
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    let clean = r#"
class Random8:
    def __init__(self):
        self.seed = 42
    def random(self):
        return self.seed

_instance = Random8()
random = _instance.random

class OddIterator:
    def __init__(self, maximum):
        self.maximum = maximum
        self.n = -1
    def __iter__(self):
        return self
    def __next__(self):
        self.n += 2
        if self.n > self.maximum:
            raise StopIteration
        return self.n

class FilteredLogger(FilterMixin, Logger):
    pass

class Wrapper:
    def __init__(self, f):
        self._file = f
    def __getattr__(self, name):
        return getattr(self._file, name)
    def write(self, s):
        return self._file.write(s.upper())

def check(x):
    return x is None
"#;
    let payload = edit_payload("src/clean.py", clean);
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 0, "idiomatic code must not warn: {stderr}");
}

#[test]
fn rust_runtime_hazard_predicates_fire_through_hook() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "fn existing() {}\n").unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[
            {"id":"lock-await","phase":"pre","priority":5,"when":[{"rust_sync_lock_guard_across_await":["?file","?fn","?guard"]}],"then":{"warn":"lock await"}},
            {"id":"unsafe-doc","phase":"pre","priority":5,"when":[{"rust_unsafe_without_safety_comment":["?file","?fn"]}],"then":{"warn":"unsafe doc"}},
            {"id":"blocking-async","phase":"pre","priority":5,"when":[{"rust_async_blocking_call":["?file","?fn","?callee"]}],"then":{"warn":"blocking async"}}
        ]}"#,
    );
    let payload = r#"{
        "tool_name":"Write",
        "tool_input":{
            "file_path":"src/lib.rs",
            "content":"use std::sync::Mutex;\nasync fn update(m: &Mutex<u8>, p: *const u8) {\n    let guard = m.lock().expect(\"lock\");\n    let _ = std::fs::read(\"x\");\n    work().await;\n    unsafe { let _ = *p; }\n    drop(guard);\n}\n"
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning rules should return one: {stderr}");
    for message in ["lock await", "unsafe doc", "blocking async"] {
        assert!(stderr.contains(message), "missing {message}: {stderr}");
    }
}

#[test]
fn result_string_rule_ignores_test_blocks() {
    // Regression test: a Result<_, String> function inside #[cfg(test)] mod
    // tests must NOT trigger the production rule. The hook strips test blocks
    // before running the values analyzer.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn existing() -> u32 { 0 }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"no-string-error","phase":"pre","priority":10,
            "when":[
                {"function_returns_result_string":["?file","?fn"]}
            ],
            "then":{"block":"bad: ?fn"}
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "fn existing() -> u32 { 0 }",
            "new_string": "fn existing() -> u32 { 0 }\n#[cfg(test)]\nmod tests {\n    fn helper() -> Result<u32, String> { Ok(0) }\n}\n"
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(
        code, 0,
        "test-scoped Result<_, String> must not block; stderr: {}",
        stderr
    );
}

fn run_post_hook_with_root(payload: &str, root: &Path) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.arg("post-check")
        .env("PHRONESIS_PROJECT_ROOT", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(payload.as_bytes());
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

// NOTE on action message phrasing in these tests:
// There is a pre-existing bug in the upstream `phronesis` crate where mixed
// action strings (e.g. `"Public ?fn takes ?param: &String"`) do not have
// their `?var` placeholders substituted with bound values — variables remain
// literal in the rendered message. To keep these integration tests robust to
// that bug (and to its eventual fix), action params are phrased as plain
// English without `?var` interpolation, and assertions check for literal
// substrings of the action text rather than substituted variable bindings.

#[test]
fn public_fn_with_string_ref_warns() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    // Post-check reads file content from disk (not the payload), so the
    // on-disk content must reflect the post-edit state.
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn greet(name: &String) {}\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-pub-str-ref","phase":"post","priority":10,
            "when":[
                {"function_is_public":["?file","?fn"]},
                {"function_param_type":["?file","?fn","?param","&String"]}
            ],
            "then":{"warn":"Public fn takes &String — prefer &str"}
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "",
            "new_string": "pub fn greet(name: &String) {}"
        }
    }"#;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning should exit 1: {}", stderr);
    assert!(stderr.contains("&str"), "stderr: {}", stderr);
}

#[test]
fn public_fn_with_vec_ref_warns() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn process(items: &Vec<u8>) {}\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-pub-vec-ref","phase":"post","priority":10,
            "when":[
                {"function_is_public":["?file","?fn"]},
                {"function_param_is_vec_ref":["?file","?fn","?param"]}
            ],
            "then":{"warn":"Public fn takes &Vec<T> — prefer &[T]"}
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "",
            "new_string": "pub fn process(items: &Vec<u8>) {}"
        }
    }"#;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning should exit 1: {}", stderr);
    assert!(stderr.contains("&[T]"), "stderr: {}", stderr);
}

#[test]
fn clone_count_warning_fires() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn copy_heavy(a: &String, b: &String) { let _x = a.clone(); let _y = b.clone(); }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-clone-count","phase":"post","priority":5,
            "when":[
                {"function_clone_count":["?file","?fn","?count"]}
            ],
            "then":{"warn":"clone usage detected — review for borrows"}
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "",
            "new_string": "fn copy_heavy(a: &String, b: &String) { let _x = a.clone(); let _y = b.clone(); }"
        }
    }"#;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning should exit 1: {}", stderr);
    assert!(
        stderr.contains("clone usage detected"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn struct_derives_warning_fires() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "#[derive(Clone)]\npub struct Foo { x: u32 }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-clone-without-debug","phase":"post","priority":5,
            "when":[
                {"struct_derives":["?file","?struct","Clone"]}
            ],
            "then":{"warn":"Cloneable struct — consider Debug too"}
        }]}"#,
    );

    let payload = r##"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "",
            "new_string": "#[derive(Clone)]\npub struct Foo { x: u32 }"
        }
    }"##;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning should exit 1: {}", stderr);
    assert!(stderr.contains("Cloneable struct"), "stderr: {}", stderr);
}

#[test]
fn swift_force_unwrap_warning_fires() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Sources")).unwrap();
    std::fs::write(
        dir.path().join("Sources/A.swift"),
        "func grab(x: Int?) -> Int { return x! }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-force-unwrap","phase":"post","priority":10,
            "when":[
                {"function_uses_force_unwrap":["?file","?fn","?count"]}
            ],
            "then":{"warn":"force-unwrap detected — prefer guard let"}
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "Sources/A.swift",
            "old_string": "",
            "new_string": "func grab(x: Int?) -> Int { return x! }"
        }
    }"#;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning should exit 1: {}", stderr);
    assert!(
        stderr.contains("force-unwrap detected"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn swift_throws_predicate_fires() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Sources")).unwrap();
    std::fs::write(
        dir.path().join("Sources/A.swift"),
        "func fetch() throws { }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-throws","phase":"post","priority":5,
            "when":[
                {"function_throws":["?file","?fn"]}
            ],
            "then":{"warn":"throwing function added"}
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "Sources/A.swift",
            "old_string": "",
            "new_string": "func fetch() throws { }"
        }
    }"#;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning should exit 1: {}", stderr);
    assert!(
        stderr.contains("throwing function added"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn log_entry_records_rule_id_and_bindings_per_consequence() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn existing() -> u32 { 0 }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"rust-error-thiserror-for-libraries","phase":"pre","priority":10,
            "when":[
                {"function_returns_result_string":["?file","?fn"]}
            ],
            "then":{"block":"`?fn` in ?file returns Result<_, String>"}
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "fn existing() -> u32 { 0 }",
            "new_string": "fn existing() -> u32 { 0 }\nfn bad() -> Result<u32, String> { Ok(0) }"
        }
    }"#;
    let (code, _stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2, "rule must block");

    let log_path = dir.path().join(".phronesis/log.jsonl");
    let contents = std::fs::read_to_string(&log_path).expect("log written");
    let last_line = contents.lines().last().expect("at least one log line");
    let entry: serde_json::Value = serde_json::from_str(last_line).expect("log line is valid JSON");

    let consequences = entry
        .get("consequences")
        .and_then(|v| v.as_array())
        .expect("entry has consequences array");
    assert_eq!(consequences.len(), 1, "exactly one consequence fired");

    let c = &consequences[0];
    assert_eq!(c["rule_id"], "rust-error-thiserror-for-libraries");
    assert_eq!(c["action_type"], "constraint_violation");
    // Substitution should now actually work — message contains "bad", not "?fn".
    assert!(
        c["message"].as_str().unwrap().contains("bad"),
        "message should contain substituted function name: {}",
        c["message"]
    );
    assert!(
        c["message"].as_str().unwrap().contains("src/lib.rs"),
        "message should contain substituted file path: {}",
        c["message"]
    );
    // Bindings preserved as a queryable map.
    assert_eq!(c["bindings"]["?fn"], "bad");
    assert_eq!(c["bindings"]["?file"], "src/lib.rs");
}

#[test]
fn warn_cargo_build_without_workspace_fires_on_bash() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-cargo-build-without-workspace","phase":"pre","priority":3,
            "when":[{"cargo_command_lacks_workspace":"?cmd"}],
            "then":{"warn":"use `--workspace`"}
        }]}"#,
    );
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"cargo build"}}"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "pre-check warning must exit 1");
    assert!(stderr.contains("--workspace"), "stderr: {stderr}");
}

#[test]
fn block_await_on_sync_execute_all_agenda_items_blocks_edit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    write_rules_file(
        dir.path(),
        &format!(
            r#"{{"rules":[{{
            "id":"block-await-on-sync-execute-all-agenda-items","phase":"pre","priority":10,
            "when":[
                {{"new_content_contains":"{call}"}},
                {{"file_extension_is":"rs"}}
            ],
            "then":{{"block":"execute_all_agenda_items is sync"}}
        }}]}}"#,
            call = concat!("execute_all_agenda_items()", ".await"),
        ),
    );
    let payload = format!(
        r#"{{"tool_name":"Edit","tool_input":{{"file_path":"src/lib.rs","old_string":"","new_string":"let _ = network.{call};"}}}}"#,
        call = concat!("execute_all_agenda_items()", ".await"),
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 2, "block must exit 2");
    assert!(stderr.contains("is sync"), "stderr: {stderr}");
}

#[test]
fn block_await_on_sync_fire_all_consequences_blocks_edit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    write_rules_file(
        dir.path(),
        &format!(
            r#"{{"rules":[{{
            "id":"block-await-on-sync-fire-all-consequences","phase":"pre","priority":10,
            "when":[
                {{"new_content_contains":"{call}"}},
                {{"file_extension_is":"rs"}}
            ],
            "then":{{"block":"fire_all_consequences is sync"}}
        }}]}}"#,
            call = concat!("fire_all_consequences()", ".await"),
        ),
    );
    let payload = format!(
        r#"{{"tool_name":"Edit","tool_input":{{"file_path":"src/lib.rs","old_string":"","new_string":"let _ = network.{call};"}}}}"#,
        call = concat!("fire_all_consequences()", ".await"),
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("is sync"));
}

#[test]
fn warn_clone_heavy_fires_at_threshold_3() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    // Post-check reads file content from disk, not the payload.
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn foo() { let _ = x.clone(); let _ = y.clone(); let _ = z.clone(); }\n",
    )
    .unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-clone-heavy","phase":"post","priority":5,
            "when":[{"function_clone_count_high":["?file","?fn","?count"]}],
            "then":{"warn":"clone-heavy"}
        }]}"#,
    );
    // 3 clones triggers the rule
    let payload = r#"{"tool_name":"Write","tool_input":{"file_path":"src/lib.rs","content":"fn foo() { let _ = x.clone(); let _ = y.clone(); let _ = z.clone(); }"}}"#;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("clone-heavy"));
}

#[test]
fn warn_clone_heavy_does_not_fire_below_threshold() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn foo() { let _ = x.clone(); let _ = y.clone(); }\n",
    )
    .unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-clone-heavy","phase":"post","priority":5,
            "when":[{"function_clone_count_high":["?file","?fn","?count"]}],
            "then":{"warn":"clone-heavy"}
        }]}"#,
    );
    // 2 clones — below threshold
    let payload = r#"{"tool_name":"Write","tool_input":{"file_path":"src/lib.rs","content":"fn foo() { let _ = x.clone(); let _ = y.clone(); }"}}"#;
    let (code, _stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 0, "2 clones must NOT trigger warn-clone-heavy");
}

#[test]
fn warn_pub_fn_missing_doc_fires_on_naked_pub_fn() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "pub fn naked() {}\n").unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-pub-fn-missing-doc","phase":"post","priority":3,
            "when":[{"pub_fn_without_doc_comment":["?file","?fn"]}],
            "then":{"warn":"needs doc"}
        }]}"#,
    );
    let payload = r#"{"tool_name":"Write","tool_input":{"file_path":"src/lib.rs","content":"pub fn naked() {}"}}"#;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("needs doc"));
}

#[test]
fn warn_empty_test_fires_on_test_with_no_assertions() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    // `assert_values_facts` dual-extracts so test-quality predicates see the
    // unstripped content. The canonical multi-line `#[test]\nfn ...` form
    // survives this path and is what production code actually looks like.
    std::fs::write(dir.path().join("src/lib.rs"), "#[test]\nfn empty() {\n}\n").unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-empty-test","phase":"post","priority":5,
            "when":[{"test_without_assertion":["?file","?fn"]}],
            "then":{"warn":"empty test"}
        }]}"#,
    );
    let payload = r##"{"tool_name":"Write","tool_input":{"file_path":"src/lib.rs","content":"#[test]\nfn empty() {\n}"}}"##;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("empty test"));
}

#[test]
fn block_rhai_inline_eval_string_blocks_edit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"block-rhai-inline-eval-string","phase":"pre","priority":10,
            "when":[
                {"engine_eval_string_literal":["?file","?fn"]},
                {"file_extension_is":"rs"}
            ],
            "then":{"block":"use precompiled AST"}
        }]}"#,
    );
    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs","old_string":"","new_string":"fn host() { let engine = rhai::Engine::new(); let _: i64 = engine.eval(\"40+2\").unwrap(); }"}}"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("precompiled"));
}

#[test]
fn block_rhai_print_in_script_blocks_edit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scripts/example")).unwrap();
    std::fs::write(
        dir.path().join("scripts/example/test.rhai"),
        "print(\"hello\")\n",
    )
    .unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"block-rhai-print-in-script","phase":"pre","priority":10,
            "when":[
                {"new_content_contains":"print("},
                {"file_extension_is":"rhai"}
            ],
            "then":{"block":"use response_append instead of print"}
        }]}"#,
    );
    let payload = r#"{"tool_name":"Write","tool_input":{"file_path":"scripts/example/test.rhai","content":"print(\"hello\")"}}"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2, "print( in .rhai is now a hard block");
    assert!(stderr.contains("response_append"));
}

#[test]
fn warn_cargo_with_p_flag_does_not_fire() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-cargo-build-without-workspace","phase":"pre","priority":3,
            "when":[{"cargo_command_lacks_workspace":"?cmd"}],
            "then":{"warn":"use workspace"}
        }]}"#,
    );
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"cargo build -p mycrate"}}"#;
    let (code, _stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 0, "-p flag suppresses the warning");
}

#[test]
fn warn_cargo_with_bin_flag_does_not_fire() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-cargo-build-without-workspace","phase":"pre","priority":3,
            "when":[{"cargo_command_lacks_workspace":"?cmd"}],
            "then":{"warn":"use workspace"}
        }]}"#,
    );
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"cargo test --bin server"}}"#;
    let (code, _stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 0, "--bin flag suppresses the warning");
}

#[test]
fn warn_clone_heavy_suppresses_unchanged_function() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    // Pre-existing file content WITH the heavy-clone function. The next
    // edit will leave this function unchanged but touch something else.
    let prior = "fn heavy(x: &String) {
    let _a = x.clone();
    let _b = x.clone();
    let _c = x.clone();
    let _d = x.clone();
}
fn unrelated() {}
";
    std::fs::write(dir.path().join("src/lib.rs"), prior).unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-clone-heavy","phase":"pre","priority":5,
            "when":[{"function_clone_count_high":["?file","?fn","?count"]}],
            "then":{"warn":"clone-heavy"}
        }]}"#,
    );

    // Edit replaces `unrelated` (not the heavy function). Heavy fn stays unchanged.
    let new_content = "fn heavy(x: &String) {
    let _a = x.clone();
    let _b = x.clone();
    let _c = x.clone();
    let _d = x.clone();
}
fn changed() { let _ = 42; }
";
    let payload = format!(
        r#"{{"tool_name":"Edit","tool_input":{{"file_path":"src/lib.rs","old_string":{prior_json},"new_string":{new_json}}}}}"#,
        prior_json = serde_json::to_string(prior).unwrap(),
        new_json = serde_json::to_string(new_content).unwrap(),
    );
    let (code, _stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(
        code, 0,
        "heavy-clone fn count did not change; rule should not fire"
    );
}

#[test]
fn warn_clone_heavy_fires_when_count_increases() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    // Prior file: heavy function with 3 clones (at the threshold).
    let prior = "fn heavy(x: &String) {
    let _a = x.clone();
    let _b = x.clone();
    let _c = x.clone();
}
";
    std::fs::write(dir.path().join("src/lib.rs"), prior).unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-clone-heavy","phase":"pre","priority":5,
            "when":[{"function_clone_count_high":["?file","?fn","?count"]}],
            "then":{"warn":"clone-heavy"}
        }]}"#,
    );

    // New content: same fn now with 4 clones (one more added).
    let new_content = "fn heavy(x: &String) {
    let _a = x.clone();
    let _b = x.clone();
    let _c = x.clone();
    let _d = x.clone();
}
";
    let payload = format!(
        r#"{{"tool_name":"Edit","tool_input":{{"file_path":"src/lib.rs","old_string":{prior_json},"new_string":{new_json}}}}}"#,
        prior_json = serde_json::to_string(prior).unwrap(),
        new_json = serde_json::to_string(new_content).unwrap(),
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 1, "increased clone count must fire the warning");
    assert!(stderr.contains("clone-heavy"));
}
