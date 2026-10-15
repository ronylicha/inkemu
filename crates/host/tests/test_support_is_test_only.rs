//! The feature `test-support` of `host` gives a front end's tests a device that loops for ever
//! (`host::test_support`); it must never reach a build a user runs. This test reads the root
//! manifest, every member of the workspace it lists, and the Tauri shell (`app/src-tauri`, a
//! workspace of its own), and refuses one that turns the feature on outside its dev-dependencies:
//! in `workspace.dependencies`, in a normal or a build dependency on `host` (renamed or not, for
//! any target), or in a feature of its own (`"host/test-support"`, or through a renamed `host`).
#![allow(clippy::disallowed_methods)] // reads the manifests: it is a test of the build
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

const FEATURE: &str = "test-support";
const CRATE: &str = "host";

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The manifests of the members `root` lists (`workspace.members`), under `dir`; a member that
/// ends in `/*` stands for every directory there with a manifest.
fn member_manifests(root: &str, dir: &Path) -> Vec<PathBuf> {
    let manifest: toml::Table = root.parse().unwrap();
    let members = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut found = Vec::new();
    for member in members.iter().filter_map(toml::Value::as_str) {
        if let Some(parent) = member.strip_suffix("/*") {
            for entry in std::fs::read_dir(dir.join(parent)).unwrap() {
                let path = entry.unwrap().path().join("Cargo.toml");
                if path.exists() {
                    found.push(path);
                }
            }
        } else {
            found.push(dir.join(member).join("Cargo.toml"));
        }
    }
    found
}

/// The manifests that build what a user runs: the root's, its members', and the shell's.
fn manifests() -> Vec<PathBuf> {
    let root = repository();
    let root_manifest = root.join("Cargo.toml");
    let mut found = member_manifests(&std::fs::read_to_string(&root_manifest).unwrap(), &root);
    found.push(root_manifest);
    found.push(root.join("app/src-tauri/Cargo.toml"));
    found.sort();
    found
}

/// Whether the dependency `spec` turns the feature on.
fn turns_it_on(spec: &toml::Value) -> bool {
    spec.get("features")
        .and_then(toml::Value::as_array)
        .is_some_and(|features| features.iter().any(|f| f.as_str() == Some(FEATURE)))
}

/// Whether the dependency named `name` with `spec` is `host`, under its name or renamed.
fn is_host(name: &str, spec: &toml::Value) -> bool {
    match spec.get("package").and_then(toml::Value::as_str) {
        Some(package) => package == CRATE,
        None => name == CRATE,
    }
}

/// The tables of dependencies a build a user runs takes, with where they are: the manifest's own,
/// those of each target, and the workspace's.
fn dependency_tables(manifest: &toml::Table) -> Vec<(String, &toml::Table)> {
    let mut places = vec![(String::new(), manifest)];
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for (target, table) in targets {
            if let Some(table) = table.as_table() {
                places.push((format!("target.{target}."), table));
            }
        }
    }
    let mut tables = Vec::new();
    for (prefix, place) in places {
        for section in ["dependencies", "build-dependencies"] {
            if let Some(table) = place.get(section).and_then(toml::Value::as_table) {
                tables.push((format!("{prefix}{section}"), table));
            }
        }
    }
    if let Some(table) = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(toml::Value::as_table)
    {
        tables.push(("workspace.dependencies".to_owned(), table));
    }
    tables
}

/// What in the manifest `text` turns the feature of `host` on outside the dev-dependencies.
fn offenders(text: &str) -> Vec<String> {
    let manifest: toml::Table = text.parse().unwrap();
    let mut found = Vec::new();
    // The names `host` goes by in this manifest, for its features.
    let mut names = vec![CRATE.to_owned()];
    for (place, table) in dependency_tables(&manifest) {
        for (name, spec) in table.iter().filter(|(name, spec)| is_host(name, spec)) {
            names.push(name.clone());
            if turns_it_on(spec) {
                found.push(format!("{place}.{name}"));
            }
        }
    }
    if let Some(features) = manifest.get("features").and_then(toml::Value::as_table) {
        for (feature, enabled) in features {
            let enabled: Vec<&str> = enabled
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                .collect();
            if names.iter().any(|name| {
                enabled.contains(&format!("{name}/{FEATURE}").as_str())
                    || enabled.contains(&format!("{name}?/{FEATURE}").as_str())
            }) {
                found.push(format!("features.{feature}"));
            }
        }
    }
    found
}

#[test]
fn no_normal_build_turns_on_the_test_device_of_host() {
    let manifests = manifests();
    for needed in [
        "crates/emu-cli/Cargo.toml",
        "crates/host/Cargo.toml",
        "app/src-tauri/Cargo.toml",
    ] {
        assert!(
            manifests.iter().any(|path| path.ends_with(needed)),
            "{needed}: {manifests:?}"
        );
    }
    assert!(
        manifests
            .iter()
            .any(|path| path == &repository().join("Cargo.toml"))
    );
    for path in manifests {
        let text = std::fs::read_to_string(&path).unwrap();
        let found = offenders(&text);
        assert!(
            found.is_empty(),
            "{} turns on `host`'s `{FEATURE}` outside its dev-dependencies: {found:?}",
            path.display()
        );
    }
}

/// The check itself: each way a manifest could turn the feature on is found, and the
/// dev-dependency the shell's tests use is not.
#[test]
fn every_way_of_turning_it_on_is_found() {
    let allowed = r#"
[dependencies]
host = { path = "../host" }
[dev-dependencies]
host = { path = "../host", features = ["test-support"] }
"#;
    assert_eq!(offenders(allowed), Vec::<String>::new());
    let cases = [
        (
            "a normal dependency",
            r#"[dependencies]
host = { path = "../host", features = ["test-support"] }"#,
        ),
        (
            "a build dependency",
            r#"[build-dependencies]
host = { path = "../host", features = ["test-support"] }"#,
        ),
        (
            "a dependency of a target",
            r#"[target.'cfg(unix)'.dependencies]
host = { path = "../host", features = ["test-support"] }"#,
        ),
        (
            "a feature of the manifest",
            r#"[dependencies]
host = { path = "../host" }
[features]
demo = ["host/test-support"]"#,
        ),
        (
            "the workspace's dependencies",
            r#"[workspace.dependencies]
host = { path = "crates/host", features = ["test-support"] }"#,
        ),
        (
            "a renamed dependency",
            r#"[dependencies]
front = { package = "host", path = "../host", features = ["test-support"] }"#,
        ),
        (
            "a feature through a renamed dependency",
            r#"[dependencies]
front = { package = "host", path = "../host", optional = true }
[features]
demo = ["front?/test-support"]"#,
        ),
    ];
    for (case, text) in cases {
        assert_eq!(offenders(text).len(), 1, "{case}: {:?}", offenders(text));
    }
}

/// The members are those the root manifest lists, wherever they are, globs included.
#[test]
fn the_members_are_read_from_the_root_manifest() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tools/one")).unwrap();
    std::fs::create_dir_all(dir.path().join("crates/a")).unwrap();
    std::fs::create_dir_all(dir.path().join("crates/b")).unwrap();
    for crate_dir in ["crates/a", "crates/b"] {
        std::fs::write(dir.path().join(crate_dir).join("Cargo.toml"), "").unwrap();
    }
    let mut found = member_manifests(
        r#"workspace = { members = ["tools/one", "crates/*"] }"#,
        dir.path(),
    );
    found.sort();
    assert_eq!(
        found,
        [
            dir.path().join("crates/a/Cargo.toml"),
            dir.path().join("crates/b/Cargo.toml"),
            dir.path().join("tools/one/Cargo.toml"),
        ]
    );
}
