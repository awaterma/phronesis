//! Execute every report-producing script: generated suffixes cannot overwrite
//! another test's native stem, and late failures cannot publish a manifest.
#![cfg(unix)]
use phronesis_mcp::coverage::{collect_js, collect_lua, collect_swift, pytest};
use std::{os::unix::fs::PermissionsExt, process::Command};

#[test]
fn collectors_preserve_every_tn_and_reject_late_failures() {
    let fake = r#"#!/usr/bin/python3
import json,os,pathlib,subprocess,sys
tool=pathlib.Path(sys.argv[0]).name
args=sys.argv[1:]
root=pathlib.Path(os.environ['ROOT'])
out=root/'out'
mode=os.environ['MODE']
def entry():
 p=root/'entry';n=int(p.read_text())+1;p.write_text(str(n));return n
def fail(n):
 if n==2 and mode=='failure': sys.exit(3)
def report(path,n):
 if n==2 and mode=='missing': return
 pathlib.Path(path).parent.mkdir(parents=True,exist_ok=True)
 pathlib.Path(path).write_text('DA:1,1\nend_of_record\n')
if tool=='git':
 print(root if '--show-toplevel' in args else 'abc')
elif tool=='python3':
 if args[0]=='-':
  subprocess.run(['/usr/bin/python3']+args,input=sys.stdin.read(),text=True,check=True)
 elif '--version' in args: print('fixture')
 elif 'run' in args:
  n=entry();fail(n)
  path=next(a.split('=',1)[1] for a in args if a.startswith('--junitxml='))
  pathlib.Path(path).write_text('<testsuite><testcase classname="tests.test_store" name="test_load"/></testsuite>')
  path=next(a.split('=',1)[1] for a in args if a.startswith('--data-file='));pathlib.Path(path).write_text('fresh')
 elif 'lcov' in args: report(args[args.index('-o')+1],int((root/'entry').read_text()))
 else: sys.exit(9)
elif tool=='swift':
 if args==['--version']: print('fixture')
 elif args==['build','--build-tests','--enable-code-coverage']:
  p=root/'build/Example.xctest';p.parent.mkdir();p.touch();p.chmod(0o755)
 elif args==['build','--show-bin-path']: print(root/'build')
 elif args==['test','--show-codecov-path']: print(root/'build/coverage.json')
 else:
  n=entry();fail(n)
  if not (n==2 and mode=='missing'): (root/'build/default.profdata').write_text('fresh')
  print('Executed 1 test, with 0 failures')
elif tool=='xcrun': sys.exit(1)
elif tool=='llvm-cov': print('DA:1,1\nend_of_record')
elif tool=='busted':
 if '--version' in args: print('fixture')
 else:
  n=entry();fail(n)
  (root/'luacov.stats.out').write_text('fresh')
  print('1 success / 0 failures / 0 errors / 0 pending : 0.001 seconds')
elif tool=='luacov': report(root/'luacov.report.out',int((root/'entry').read_text()))
elif tool=='lua':
 sys.stdin.read();(out/'manifest.json').write_text('{}')
elif tool=='npx':
 if '--version' in args: print('fixture');sys.exit(0)
 n=entry();fail(n)
 path=next((a.split('=',1)[1] for a in args if a.startswith(('--coverage.reportsDirectory=','--coverageDirectory='))),None)
 if path is None: path=args[args.index('-o')+1]
 report(path+'/lcov.info',n)
 if 'c8' in args: print('ok 1 - selected\n# pass 1\n# fail 0')
 else:
  path=next(a.split('=',1)[1] for a in args if a.startswith('--outputFile='))
  pathlib.Path(path).write_text(json.dumps({'numPassedTests':1,'numFailedTests':0,'testResults':[{'assertionResults':[{'fullName':'selected','status':'passed'}]}]}))
else: sys.exit(9)
"#;
    for runner in ["python", "swift", "lua", "vitest", "jest", "node"] {
        for mode in ["success", "failure", "missing"] {
            let dir = tempfile::tempdir().expect("project");
            let root = dir.path();
            let bin = root.join("bin");
            std::fs::create_dir(&bin).expect("bin");
            for name in [
                "python3", "swift", "xcrun", "llvm-cov", "busted", "luacov", "lua", "npx", "git",
            ] {
                let path = bin.join(name);
                std::fs::write(&path, fake).expect("fake tool");
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod");
            }
            std::fs::write(root.join("entry"), "0").expect("counter");
            let ids = ["example::a+b", "example::a?b", "example::a_b_2"];
            let entries: Vec<_> = ids
                .iter()
                .map(|id| {
                    (
                        id.to_string(),
                        "tests/test_store.py".to_string(),
                        "selected".to_string(),
                    )
                })
                .collect();
            let out = root.join("out");
            let script = match runner {
                "python" => pytest::collection_script(
                    &ids.iter()
                        .map(|id| ("tests/test_store.py::test_load".to_string(), id.to_string()))
                        .collect::<Vec<_>>(),
                    &out,
                ),
                "swift" => collect_swift::collection_script(
                    &ids.iter()
                        .map(|id| (id.to_string(), "selected".to_string()))
                        .collect::<Vec<_>>(),
                    &out,
                ),
                "lua" => collect_lua::collection_script(&entries, &out),
                _ => collect_js::collection_script(
                    match runner {
                        "vitest" => collect_js::JsRunner::Vitest,
                        "jest" => collect_js::JsRunner::Jest,
                        _ => collect_js::JsRunner::NodeTest,
                    },
                    &entries,
                    &out,
                ),
            };
            let file = root.join("collect.sh");
            std::fs::write(&file, script).expect("script");
            let result = Command::new("sh")
                .arg(&file)
                .current_dir(root)
                .env("ROOT", root)
                .env("MODE", mode)
                .env(
                    "PATH",
                    format!("{}:{}", bin.display(), std::env::var("PATH").expect("PATH")),
                )
                .output()
                .expect("execute collector");
            assert_eq!(
                result.status.success(),
                mode == "success",
                "{runner}/{mode}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(
                out.join("manifest.json").exists(),
                mode == "success",
                "{runner}/{mode}"
            );
            let reports: Vec<_> = std::fs::read_dir(&out)
                .expect("reports")
                .map(|entry| entry.expect("entry").path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "lcov")
                })
                .collect();
            let actual: std::collections::BTreeSet<_> = reports
                .iter()
                .flat_map(|path| {
                    std::fs::read_to_string(path)
                        .expect("report")
                        .lines()
                        .filter_map(|line| line.strip_prefix("TN:").map(str::to_string))
                        .collect::<Vec<_>>()
                })
                .collect();
            let expected: std::collections::BTreeSet<_> = if mode == "success" {
                ids.iter().map(|id| id.to_string()).collect()
            } else {
                [ids[0].to_string()].into_iter().collect()
            };
            assert_eq!(
                actual, expected,
                "{runner}/{mode}: every distinct TN must survive"
            );
        }
    }
}
