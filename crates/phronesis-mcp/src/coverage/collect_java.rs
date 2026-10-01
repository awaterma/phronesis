use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaRunner {
    Mvn,
    Gradle,
}

pub fn detect_runner(root: &Path, runner: Option<JavaRunner>) -> Result<JavaRunner> {
    if let Some(runner) = runner {
        return Ok(runner);
    }
    let mvn = root.join("pom.xml").exists();
    let gradle = root.join("build.gradle").exists() || root.join("build.gradle.kts").exists();
    match (mvn, gradle) {
        (true, false) => Ok(JavaRunner::Mvn),
        (false, true) => Ok(JavaRunner::Gradle),
        (true, true) => bail!("both pom.xml and Gradle build files found; pass --runner"),
        (false, false) => bail!("no Maven or Gradle build file found; pass --runner"),
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn collection_script(
    runner: JavaRunner,
    entries: &[(String, String, String)],
    out: &Path,
) -> String {
    let mut script = format!(
        "#!/bin/sh\nset -eu\nOUT={}\nmkdir -p \"$OUT\"\n",
        quote(&out.to_string_lossy())
    );
    script.push_str(
        r#"OUT=$(cd "$OUT" && pwd)
rm -f "$OUT/manifest.json"
python3 - "$OUT" <<'PYTHON'
import pathlib,shutil,sys
for entry in pathlib.Path(sys.argv[1]).iterdir():
 if entry.name.isdigit() and entry.is_dir():
  shutil.rmtree(entry)
PYTHON
"#,
    );
    if runner == JavaRunner::Gradle {
        script.push_str(
            r#"cat > "$OUT/coverage.init.gradle" <<'GRADLE'
allprojects {
    if (projectDir.canonicalPath != System.getenv('PHR_JAVA_MODULE')) return
    plugins.withId('java') {
        apply plugin: 'jacoco'
        tasks.named('test', Test).configure {
            outputs.upToDateWhen { false }
            reports.junitXml.required = true
            reports.junitXml.outputLocation = file(System.getenv('PHR_JAVA_RUN') + '/junit')
            jacoco.destinationFile = file(System.getenv('PHR_JAVA_RUN') + '/coverage.exec')
        }
        tasks.named('jacocoTestReport', JacocoReport).configure {
            executionData.setFrom(file(System.getenv('PHR_JAVA_RUN') + '/coverage.exec'))
            reports.xml.required = true
            reports.xml.outputLocation = file(System.getenv('PHR_JAVA_RUN') + '/jacoco.xml')
        }
    }
}
GRADLE
"#,
        );
    }
    for (index, (module, native, graph_id)) in entries.iter().enumerate() {
        let n = index + 1;
        script.push_str(&format!("rm -rf \"$OUT/{n}\"\nmkdir -p \"$OUT/{n}\"\nprintf '%s\\n' {} > \"$OUT/{n}/module.txt\"\nprintf '%s\\n' {} > \"$OUT/{n}/TN\"\n", quote(module), quote(graph_id)));
        match runner {
            JavaRunner::Mvn => {
                // Reactor dependencies must not run similarly named tests and
                // overwrite evidence attributed to the selected module.
                script.push_str(&format!("rm -rf {}/target/surefire-reports {}/target/site/jacoco\nmvn -pl {} -Dtest={} -Dsurefire.failIfNoSpecifiedTests=true -Djacoco.append=false -Djacoco.destFile=\"$OUT/{n}/coverage.exec\" org.jacoco:jacoco-maven-plugin:prepare-agent test org.jacoco:jacoco-maven-plugin:report -Djacoco.dataFile=\"$OUT/{n}/coverage.exec\" > \"$OUT/{n}/runner.log\" 2>&1\ncp {}/target/site/jacoco/jacoco.xml \"$OUT/{n}/jacoco.xml\"\nREPORTS={}/target/surefire-reports\n", quote(module), quote(module), quote(module), quote(native), quote(module), quote(module)));
            }
            JavaRunner::Gradle => script.push_str(&format!("PHR_JAVA_MODULE=$(cd {} && pwd) PHR_JAVA_RUN=\"$OUT/{n}\" gradle -p {} --init-script \"$OUT/coverage.init.gradle\" --rerun-tasks --no-build-cache test --tests {} jacocoTestReport > \"$OUT/{n}/runner.log\" 2>&1\nREPORTS=\"$OUT/{n}/junit\"\n", quote(module), quote(module), quote(&native.replace('#', ".")))),
        }
        script.push_str(&format!("SELECTED={}\n", quote(&native.replace('#', "."))));
        script.push_str(r#"python3 - "$REPORTS" "$SELECTED" <<'PYTHON'
import glob,sys,xml.etree.ElementTree as ET
cases=[]
for path in glob.glob(sys.argv[1]+'/**/*.xml',recursive=True):
 cases.extend(ET.parse(path).getroot().iter('testcase'))
if len(cases)!=1 or any(c.find(tag) is not None for c in cases for tag in ('failure','error','skipped')):
 raise SystemExit('expected exactly one successful Java test; found '+str(len(cases)))
case=cases[0]
if case.get('classname','')+'.'+case.get('name','') != sys.argv[2]:
 raise SystemExit('Java runner executed a different test than requested')
PYTHON
"#);
        script.push_str(&format!(
            "test -s \"$OUT/{n}/coverage.exec\"\ntest -s \"$OUT/{n}/jacoco.xml\"\n"
        ));
    }
    script.push_str(&format!("python3 - \"$OUT\" {} <<'PY'\nimport hashlib,json,os,subprocess,sys,xml.etree.ElementTree as ET\nout=sys.argv[1];runner=sys.argv[2];root=subprocess.check_output(['git','rev-parse','--show-toplevel'],text=True).strip();files={{}}\nfor base,dirs,names in os.walk(out):\n if 'jacoco.xml' not in names: continue\n module=open(os.path.join(base,'module.txt')).read().strip()\n tree=ET.parse(os.path.join(base,'jacoco.xml'))\n for package in tree.findall('.//package'):\n  pkg=package.attrib.get('name','')\n  for source in package.findall('sourcefile'):\n   rel=os.path.join(module,'src','main','java',pkg.replace('/',os.sep),source.attrib['name'])\n   full=os.path.realpath(os.path.join(root,rel))\n   if not full.startswith(root+os.sep): raise SystemExit('source outside repository: '+full)\n   if os.path.isfile(full): files[os.path.relpath(full,root).replace(os.sep,'/')]=hashlib.sha256(open(full,'rb').read()).hexdigest()\nrev=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()\njson.dump({{'revision':rev,'files':files,'runner':runner}},open(os.path.join(out,'manifest.json'),'w'),sort_keys=True)\nPY\n", if runner == JavaRunner::Mvn { "mvn" } else { "gradle" }));
    script
}

pub fn default_output(root: &Path) -> PathBuf {
    root.join(".phronesis/java-coverage")
}

pub fn read_runner_arg(value: &str) -> Result<JavaRunner> {
    match value {
        "mvn" => Ok(JavaRunner::Mvn),
        "gradle" => Ok(JavaRunner::Gradle),
        _ => bail!("unknown Java runner {value:?}; expected mvn or gradle"),
    }
}

pub fn require_project(root: &Path) -> Result<()> {
    root.canonicalize()
        .with_context(|| format!("resolving project {}", root.display()))
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_maven_gradle_and_ambiguity() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pom.xml"), "").expect("pom");
        assert_eq!(
            detect_runner(dir.path(), None).expect("maven"),
            JavaRunner::Mvn
        );
        std::fs::write(dir.path().join("build.gradle.kts"), "").expect("gradle");
        assert!(
            detect_runner(dir.path(), None)
                .expect_err("ambiguous")
                .to_string()
                .contains("--runner")
        );
    }
    #[test]
    fn generated_scripts_filter_and_stamp_maven_and_gradle_runs() {
        let entries = vec![(
            "core".into(),
            "com.x.StoreTest#testLoad".into(),
            "java:core::com::x::StoreTest::testLoad".into(),
        )];
        let mvn = collection_script(JavaRunner::Mvn, &entries, Path::new("cov"));
        assert!(
            mvn.contains("-Dsurefire.failIfNoSpecifiedTests=true")
                && mvn.contains("module.txt")
                && mvn.contains("TN")
        );
        assert!(mvn.contains("runner=sys.argv[2]") && mvn.contains("mvn"));
        let gradle = collection_script(JavaRunner::Gradle, &entries, Path::new("cov"));
        assert!(
            gradle.contains("--tests")
                && gradle.contains("--rerun-tasks")
                && gradle.contains("reports.xml.outputLocation")
                && gradle.contains("gradle")
        );
    }
    #[test]
    #[cfg(unix)]
    fn emitted_java_scripts_validate_real_report_counts_and_clear_stale_evidence() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        for runner in [JavaRunner::Mvn, JavaRunner::Gradle] {
            let dir = tempfile::tempdir().expect("tempdir");
            let root = dir.path();
            std::fs::create_dir(root.join("core")).expect("module");
            let bin = root.join("bin");
            std::fs::create_dir(&bin).expect("bin");
            let name = if runner == JavaRunner::Mvn {
                "mvn"
            } else {
                "gradle"
            };
            let fake = bin.join(name);
            std::fs::write(&fake, r#"#!/usr/bin/env python3
import os,pathlib,sys
args=sys.argv[1:]
if 'PHR_JAVA_RUN' in os.environ:
 out=pathlib.Path(os.environ['PHR_JAVA_RUN'])
 reports=out/'junit'
 init=pathlib.Path(args[args.index('--init-script')+1]).read_text()
 assert 'reports.xml.outputLocation' in init and 'jacoco.destinationFile' in init
 assert '--rerun-tasks' in args and '--no-build-cache' in args
else:
 assert '-am' not in args
 out=pathlib.Path(next(a.split('=',1)[1] for a in args if a.startswith('-Djacoco.destFile='))).parent
 reports=pathlib.Path('core/target/surefire-reports')
reports.mkdir(parents=True,exist_ok=True)
wrong=os.environ['FAKE_COUNT']=='wrong'
count=1 if wrong else int(os.environ['FAKE_COUNT'])
skip='<skipped/>' if os.environ.get('FAKE_SKIP') else ''
(reports/'TEST-fixture.xml').write_text('<testsuite>'+''.join('<testcase classname="example.Test" name="selected">'+skip+'</testcase>' for _ in range(count))+'</testsuite>')
if not os.environ.get('FAKE_NO_COVERAGE'):
 (out/'coverage.exec').write_bytes(b'fresh')
 xml=out/'jacoco.xml' if 'PHR_JAVA_RUN' in os.environ else pathlib.Path('core/target/site/jacoco/jacoco.xml')
 xml.parent.mkdir(parents=True,exist_ok=True)
 xml.write_text('<report/>')
if wrong:
 (reports/'TEST-fixture.xml').write_text('<testsuite><testcase classname="example.Test" name="different"/></testsuite>')
print('BUILD SUCCESSFUL; Tests run: 1; 11 tests completed, 0 failed')
"#).expect("fake");
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            for args in [
                vec!["init", "-q"],
                vec![
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.com",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "fixture",
                ],
            ] {
                assert!(
                    Command::new("git")
                        .args(args)
                        .current_dir(root)
                        .status()
                        .expect("git")
                        .success()
                );
            }
            let out = root.join("coverage");
            let script = root.join("collect.sh");
            std::fs::write(
                &script,
                collection_script(
                    runner,
                    &[(
                        "core".into(),
                        "example.Test#selected".into(),
                        "java:core::example::Test::selected".into(),
                    )],
                    &out,
                ),
            )
            .expect("script");
            let run = |count: &str, skip: bool, no_coverage: bool| {
                let mut command = Command::new("sh");
                command
                    .arg(&script)
                    .current_dir(root)
                    .env(
                        "PATH",
                        format!("{}:{}", bin.display(), std::env::var("PATH").expect("PATH")),
                    )
                    .env("FAKE_COUNT", count);
                if skip {
                    command.env("FAKE_SKIP", "1");
                }
                if no_coverage {
                    command.env("FAKE_NO_COVERAGE", "1");
                }
                command.output().expect("collector")
            };
            assert!(run("1", false, false).status.success());
            assert!(out.join("manifest.json").exists());
            for count in ["0", "2", "11", "21"] {
                let result = run(count, false, false);
                assert!(!result.status.success(), "accepted {count}");
                assert!(!out.join("manifest.json").exists());
                assert!(
                    String::from_utf8_lossy(&result.stderr).contains("expected exactly one"),
                    "{result:?}"
                );
            }
            assert!(!run("1", true, false).status.success());
            assert!(!run("wrong", false, false).status.success());
            assert!(run("1", false, false).status.success());
            assert!(
                !run("1", false, true).status.success(),
                "stale coverage survived"
            );
        }
    }
}
