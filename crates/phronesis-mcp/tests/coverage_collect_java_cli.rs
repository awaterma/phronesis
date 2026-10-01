//! Graph namespaces are identities, not Maven/Gradle filesystem paths.
use std::process::Command;

#[test]
fn java_collection_uses_test_source_module_not_graph_namespace() {
    for module in ["", "core/"] {
        let dir = tempfile::tempdir().expect("fixture");
        let root = dir.path();
        let source = format!("{module}src/test/java/com/x/StoreTest.java");
        std::fs::create_dir_all(root.join(&source).parent().expect("parent")).expect("mkdir");
        std::fs::write(root.join(&source), "package com.x; import org.junit.Test; public class StoreTest { @Test public void testLoad() {} }").expect("source");
        std::fs::write(root.join(format!("{module}pom.xml")), "<project><modelVersion>4.0.0</modelVersion><groupId>com.x</groupId><artifactId>identity</artifactId><version>1</version></project>").expect("pom");
        if !module.is_empty() {
            std::fs::write(root.join("pom.xml"), "<project><modelVersion>4.0.0</modelVersion><groupId>com.x</groupId><artifactId>parent</artifactId><version>1</version><packaging>pom</packaging><modules><module>core</module></modules></project>").expect("parent pom");
        }
        for args in [
            vec!["init", "-q"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
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
        let bin = env!("CARGO_BIN_EXE_phr-mcp");
        assert!(
            Command::new(bin)
                .args(["graph", "rebuild"])
                .current_dir(root)
                .status()
                .expect("graph")
                .success()
        );
        let output = Command::new(bin)
            .args([
                "coverage",
                "collect",
                "--tool",
                "java-cov",
                "--runner",
                "mvn",
                "--emit-script",
            ])
            .current_dir(root)
            .output()
            .expect("emit");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let script = String::from_utf8(output.stdout).expect("script");
        let path = if module.is_empty() { "." } else { "core" };
        assert!(script.contains(&format!("mvn -pl '{path}'")), "{script}");
        assert!(!script.contains("mvn -pl 'com.x:"), "{script}");
    }
}
