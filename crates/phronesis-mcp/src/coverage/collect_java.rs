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
    for (index, (module, native, graph_id)) in entries.iter().enumerate() {
        let n = index + 1;
        script.push_str(&format!("mkdir -p \"$OUT/{n}\"\nprintf '%s\\n' {} > \"$OUT/{n}/module.txt\"\nprintf '%s\\n' {} > \"$OUT/{n}/TN\"\n", quote(module), quote(graph_id)));
        match runner {
            JavaRunner::Mvn => script.push_str(&format!("mvn -pl {} -am -Dtest={} -Dsurefire.failIfNoSpecifiedTests=true -Djacoco.destFile=\"$OUT/{n}/coverage.exec\" org.jacoco:jacoco-maven-plugin:prepare-agent test org.jacoco:jacoco-maven-plugin:report -Djacoco.dataFile=\"$OUT/{n}/coverage.exec\" -Djacoco.outputDirectory=\"$OUT/{n}\" > \"$OUT/{n}/runner.log\" 2>&1\ngrep -Eq 'Tests run: [1-9][0-9]*, Failures: [0-9]+, Errors: [0-9]+' \"$OUT/{n}/runner.log\"\ntest -s \"$OUT/{n}/jacoco.xml\"\n", quote(module), quote(native)) ),
            JavaRunner::Gradle => script.push_str(&format!("gradle -p {} test --tests {} jacocoTestReport -Pjacoco.xml.destination=\"$OUT/{n}/jacoco.xml\" > \"$OUT/{n}/runner.log\" 2>&1\ngrep -Eq '[1-9][0-9]* tests completed, [0-9]+ failed' \"$OUT/{n}/runner.log\"\ngrep -q 'BUILD SUCCESSFUL' \"$OUT/{n}/runner.log\"\ntest -s \"$OUT/{n}/jacoco.xml\"\n", quote(module), quote(native)) ),
        }
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
                && gradle.contains("BUILD SUCCESSFUL")
                && gradle.contains("-Pjacoco.xml.destination")
                && gradle.contains("gradle")
        );
    }
}
