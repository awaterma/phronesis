use super::*;

fn discover_poms(poms: &[(&str, &str)]) -> Discovery {
    discover(
        &poms
            .iter()
            .map(|(path, body)| (path.to_string(), body.to_string()))
            .collect(),
    )
}

#[test]
fn declared_external_parent_coordinates_supply_group_and_root_interpolation() {
    let out = discover_poms(&[(
        "child/pom.xml",
        "<project><parent><groupId>expected</groupId><artifactId>parent</artifactId></parent><artifactId>app</artifactId><build><sourceDirectory>${project.groupId}/src</sourceDirectory></build></project>",
    )]);
    assert_eq!(out.diagnostics.count("external_parent_skipped"), 1);
    assert_eq!(out.diagnostics.count("unresolved_property"), 0);
    assert_eq!(out.units[0].id, "expected:app");
    assert!(out.units[0].sources.contains(&SourceRoot {
        path: "child/expected/src".into(),
        context: Context::Production,
    }));
}

#[test]
fn helper_execution_ids_merge_before_materializing_child_roots() {
    let out = discover_poms(&[
        (
            "pom.xml",
            r#"<project><groupId>e</groupId><artifactId>parent</artifactId>
          <build><plugins><plugin><artifactId>build-helper-maven-plugin</artifactId>
            <configuration><sources><source>plugin-default</source></sources></configuration>
            <executions>
              <execution><id>replace</id><goals><goal>add-source</goal></goals>
                <configuration><sources><source>old-one</source><source>old-two</source></sources></configuration></execution>
              <execution><id>local</id><inherited>false</inherited><goals><goal>add-source</goal></goals>
                <configuration><sources><source>parent-only</source></sources></configuration></execution>
              <execution><id>defaults</id><goals><goal>add-test-source</goal></goals></execution>
              <execution><id>disabled</id><goals><goal>add-source</goal></goals>
                <configuration><sources><source>disabled-root</source></sources></configuration></execution>
            </executions>
          </plugin></plugins></build></project>"#,
        ),
        (
            "child/pom.xml",
            r#"<project><parent><artifactId>parent</artifactId></parent><artifactId>child</artifactId>
          <build><plugins><plugin><artifactId>build-helper-maven-plugin</artifactId>
            <configuration><sources><source>child-default</source></sources></configuration>
            <executions>
              <execution><id>replace</id><configuration><sources><source>new</source></sources></configuration></execution>
              <execution><id>disabled</id><phase>none</phase></execution>
            </executions>
          </plugin></plugins></build></project>"#,
        ),
    ]);
    let parent = out.units.iter().find(|u| u.id == "e:parent").unwrap();
    assert!(parent.sources.iter().any(|s| s.path == "parent-only"));
    let child = out.units.iter().find(|u| u.id == "e:child").unwrap();
    let paths = child
        .sources
        .iter()
        .map(|s| (s.path.as_str(), s.context))
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        [
            ("child/child-default", Context::Test),
            ("child/new", Context::Production),
            ("child/src/main/java", Context::Production),
            ("child/src/test/java", Context::Test),
        ]
    );
}

#[test]
fn helper_configuration_append_override_and_plugin_inheritance_are_distinct() {
    let parent = r#"<project><groupId>e</groupId><artifactId>parent</artifactId>
      <build><plugins><plugin><artifactId>build-helper-maven-plugin</artifactId>
        <configuration><sources><source>base</source></sources></configuration>
        <executions><execution><id>main</id><goals><goal>add-source</goal></goals></execution></executions>
      </plugin></plugins></build></project>"#;
    for (configuration, expected) in [
        (
            r#"<configuration><sources combine.children="append"><source>extra</source></sources></configuration>"#,
            vec!["child/base", "child/extra"],
        ),
        (
            r#"<configuration combine.self="override"><sources><source>extra</source></sources></configuration>"#,
            vec!["child/extra"],
        ),
        (r#"<configuration combine.self="override"/>"#, vec![]),
    ] {
        let child = format!(
            r#"<project><parent><artifactId>parent</artifactId></parent><artifactId>child</artifactId>
          <build><plugins><plugin><artifactId>build-helper-maven-plugin</artifactId>{configuration}</plugin></plugins></build></project>"#
        );
        let out = discover_poms(&[("pom.xml", parent), ("child/pom.xml", &child)]);
        let unit = out.units.iter().find(|u| u.id == "e:child").unwrap();
        let added = unit
            .sources
            .iter()
            .filter(|s| !s.path.starts_with("child/src/"))
            .map(|s| s.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(added, expected);
    }
    let non_inherited = parent.replace(
        "<configuration>",
        "<inherited>false</inherited><configuration>",
    );
    let out = discover_poms(&[
        ("pom.xml", &non_inherited),
        (
            "child/pom.xml",
            "<project><parent><artifactId>parent</artifactId></parent><artifactId>child</artifactId></project>",
        ),
    ]);
    let unit = out.units.iter().find(|u| u.id == "e:child").unwrap();
    assert_eq!(unit.sources.len(), 2);
    // An execution can explicitly opt in even when its plugin opts out.
    let opted_in = non_inherited.replace(
        "<id>main</id>",
        "<id>main</id><inherited>true</inherited><configuration><sources><source>explicit</source></sources></configuration>",
    );
    let out = discover_poms(&[
        ("pom.xml", &opted_in),
        (
            "child/pom.xml",
            "<project><parent><artifactId>parent</artifactId></parent><artifactId>child</artifactId></project>",
        ),
    ]);
    let unit = out.units.iter().find(|u| u.id == "e:child").unwrap();
    assert!(unit.sources.iter().any(|s| s.path == "child/explicit"));
    assert!(!unit.sources.iter().any(|s| s.path == "child/base"));
}

#[test]
fn parent_inheritance_and_profiles_resolve_roots_in_child_context() {
    let out = discover_poms(&[
        (
            "pom.xml",
            r#"<project xmlns="http://maven.apache.org/POM/4.0.0">
          <groupId>example</groupId><artifactId>parent</artifactId><packaging>pom</packaging>
          <build><sourceDirectory>${project.basedir}/code/${project.artifactId}</sourceDirectory></build>
        </project>"#,
        ),
        (
            "child/pom.xml",
            r#"<project><parent><artifactId>parent</artifactId></parent><artifactId>child</artifactId>
          <profiles>
            <profile><id>default</id><activation><activeByDefault>true</activeByDefault></activation>
              <build><testSourceDirectory>checks/${project.groupId}</testSourceDirectory></build></profile>
            <profile><id>optional</id><build><sourceDirectory>wrong</sourceDirectory></build></profile>
          </profiles>
        </project>"#,
        ),
    ]);
    assert!(out.failed_manifests.is_empty());
    assert_eq!(out.units.len(), 1);
    assert_eq!(out.units[0].id, "example:child");
    assert!(out.units[0].sources.contains(&SourceRoot {
        path: "child/code/child".into(),
        context: Context::Production
    }));
    assert!(out.units[0].sources.contains(&SourceRoot {
        path: "child/checks/example".into(),
        context: Context::Test
    }));
    assert_eq!(out.diagnostics.count("inactive_profile_skipped"), 1);
}

#[test]
fn build_helper_roots_are_normalized_then_versioned_roots_are_removed() {
    let out = discover_poms(&[(
        "app/pom.xml",
        r#"<project><groupId>example</groupId><artifactId>app</artifactId>
      <build><sourceDirectory>${unknown}</sourceDirectory><plugins><plugin>
        <artifactId>build-helper-maven-plugin</artifactId><executions>
          <execution><id>main</id><goals><goal>add-source</goal></goals><configuration><sources>
            <source>extra/../sources</source><source>src/main/java11</source>
          </sources></configuration></execution>
          <execution><id>test</id><goals><goal>add-test-source</goal></goals><configuration><sources>
            <source>checks</source><source>src/test/java17</source>
          </sources></configuration></execution>
        </executions></plugin></plugins></build>
    </project>"#,
    )]);
    let paths = out.units[0]
        .sources
        .iter()
        .map(|s| s.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        [
            "app/checks",
            "app/sources",
            "app/src/main/java",
            "app/src/test/java"
        ]
    );
    assert_eq!(out.diagnostics.count("multi_release_root_ignored"), 2);
    assert_eq!(out.diagnostics.count("unresolved_property"), 1);
    assert!(out.excluded_roots.contains("app/src/main/java11"));
}

#[test]
fn unavailable_parents_group_fallback_collisions_and_parse_failures_are_visible() {
    let out = discover_poms(&[
        (
            "a/pom.xml",
            "<project><parent><groupId>remote</groupId><relativePath/></parent><artifactId>app</artifactId></project>",
        ),
        (
            "b/pom.xml",
            "<project><groupId>remote</groupId><artifactId>app</artifactId></project>",
        ),
        (
            "c/pom.xml",
            "<project><artifactId>solo</artifactId></project>",
        ),
        ("broken/pom.xml", "<project>"),
    ]);
    assert_eq!(out.units.len(), 2);
    assert_eq!(out.diagnostics.count("external_parent_skipped"), 1);
    assert_eq!(out.diagnostics.count("unit_id_collision"), 1);
    assert_eq!(out.diagnostics.count("group_id_unresolved"), 1);
    assert_eq!(
        out.failed_manifests,
        BTreeSet::from(["broken/pom.xml".into()])
    );
}

#[test]
fn compile_visibility_propagates_scopes_and_excludes_unrelated_units() {
    let mut manifests = BTreeMap::new();
    for (name, deps) in [
        (
            "app",
            vec![
                ("api", "compile"),
                ("container", "provided"),
                ("tests", "test"),
            ],
        ),
        (
            "api",
            vec![
                ("runtime", "runtime"),
                ("core", "compile"),
                ("hidden", "provided"),
            ],
        ),
        ("container", vec![("support", "runtime")]),
        ("tests", vec![("helper", "compile")]),
        ("runtime", vec![]),
        ("core", vec![]),
        ("hidden", vec![]),
        ("support", vec![]),
        ("helper", vec![]),
        ("unrelated", vec![]),
    ] {
        let deps = deps.iter().map(|(name, scope)| format!("<dependency><groupId>e</groupId><artifactId>{name}</artifactId><scope>{scope}</scope></dependency>")).collect::<String>();
        manifests.insert(format!("{name}/pom.xml"), format!("<project><groupId>e</groupId><artifactId>{name}</artifactId><dependencies>{deps}</dependencies></project>"));
    }
    let out = discover(&manifests);
    assert_eq!(
        out.visible_units("e:app", false),
        BTreeSet::from([
            "e:api".into(),
            "e:core".into(),
            "e:container".into(),
            "e:support".into()
        ])
    );
    assert_eq!(
        out.visible_units("e:app", true),
        BTreeSet::from([
            "e:api".into(),
            "e:core".into(),
            "e:container".into(),
            "e:support".into(),
            "e:tests".into(),
            "e:helper".into(),
            "e:runtime".into()
        ])
    );
}

#[test]
fn manifest_input_cannot_escape_repository_or_expand_external_entities() {
    assert_eq!(normalize("app", "../../outside"), None);
    assert_eq!(normalize("", "/outside"), None);
    let out = discover_poms(&[(
        "pom.xml",
        r#"<!DOCTYPE project [<!ENTITY external SYSTEM "file:///etc/passwd">]><project><artifactId>&external;</artifactId></project>"#,
    )]);
    assert_eq!(out.failed_manifests.len(), 1);
    assert!(out.units.is_empty());
}

#[test]
fn cyclic_parent_chains_terminate_with_a_diagnostic() {
    let out = discover_poms(&[
        (
            "a/pom.xml",
            "<project><parent><relativePath>../b/pom.xml</relativePath></parent><artifactId>a</artifactId></project>",
        ),
        (
            "b/pom.xml",
            "<project><parent><relativePath>../a/pom.xml</relativePath></parent><artifactId>b</artifactId></project>",
        ),
    ]);
    assert_eq!(out.diagnostics.count("parent_cycle"), 1);
}

#[test]
fn basedir_does_not_hide_unresolved_properties_and_wrong_parent_does_not_supply_roots() {
    let out = discover_poms(&[
        (
            "pom.xml",
            "<project><groupId>wrong</groupId><artifactId>parent</artifactId><packaging>pom</packaging><build><sourceDirectory>wrong</sourceDirectory></build></project>",
        ),
        (
            "app/pom.xml",
            "<project><parent><groupId>expected</groupId><artifactId>parent</artifactId></parent><artifactId>app</artifactId><build><testSourceDirectory>${project.basedir}/${unknown}</testSourceDirectory></build></project>",
        ),
    ]);
    assert_eq!(out.units[0].id, "expected:app");
    assert_eq!(out.diagnostics.count("external_parent_skipped"), 1);
    assert_eq!(out.diagnostics.count("unresolved_property"), 1);
    assert!(out.units[0].sources.contains(&SourceRoot {
        path: "app/src/main/java".into(),
        context: Context::Production
    }));
    assert!(out.units[0].sources.contains(&SourceRoot {
        path: "app/src/test/java".into(),
        context: Context::Test
    }));
}
