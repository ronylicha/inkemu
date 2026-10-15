//! The feature `test-support` of `host` gives a front end's tests a device that loops for ever
//! (`host::test_support`); it must never reach a build a user runs. This test reads every
//! manifest of the repository (the workspace's crates and the Tauri shell, `app/src-tauri`) and
//! refuses one that turns the feature on outside its dev-dependencies: in a normal or a build
//! dependency on `host`, for any target, or in a feature of its own (`"host/test-support"`).
#![allow(clippy::disallowed_methods)] // reads the manifests: it is a test of the build
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

const FEATURE: &str = "test-support";

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The manifests that build what a user runs: every crate of the workspace, and the shell.
fn manifests() -> Vec<PathBuf> {
    let root = repository();
    let mut found: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .unwrap()
        .map(|entry| entry.unwrap().path().join("Cargo.toml"))
        .filter(|path| path.exists())
        .collect();
    found.push(root.join("app/src-tauri/Cargo.toml"));
    found.sort();
    found
}

/// Whether the dependency `spec` (on `host`) turns the feature on.
fn turns_it_on(spec: &toml::Value) -> bool {
    spec.get("features")
        .and_then(toml::Value::as_array)
        .is_some_and(|features| features.iter().any(|f| f.as_str() == Some(FEATURE)))
}

/// What in the manifest `text` turns the feature of `host` on outside the dev-dependencies.
fn offenders(text: &str) -> Vec<String> {
    let manifest: toml::Table = text.parse().unwrap();
    let mut found = Vec::new();
    let mut tables = vec![(String::new(), &manifest)];
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for (target, table) in targets {
            if let Some(table) = table.as_table() {
                tables.push((format!("target.{target}."), table));
            }
        }
    }
    for (prefix, table) in tables {
        for section in ["dependencies", "build-dependencies"] {
            if table
                .get(section)
                .and_then(|deps| deps.get("host"))
                .is_some_and(turns_it_on)
            {
                found.push(format!("{prefix}{section}.host"));
            }
        }
    }
    if let Some(features) = manifest.get("features").and_then(toml::Value::as_table) {
        for (name, enabled) in features {
            let enables = enabled.as_array().into_iter().flatten();
            if enables
                .filter_map(toml::Value::as_str)
                .any(|f| f == format!("host/{FEATURE}") || f == format!("host?/{FEATURE}"))
            {
                found.push(format!("features.{name}"));
            }
        }
    }
    found
}

#[test]
fn no_normal_build_turns_on_the_test_device_of_host() {
    let manifests = manifests();
    assert!(
        manifests
            .iter()
            .any(|path| path.ends_with("emu-cli/Cargo.toml")),
        "{manifests:?}"
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
    let refused = r#"
[dependencies]
host = { path = "../host", features = ["test-support"] }
[build-dependencies]
host = { path = "../host", features = ["test-support"] }
[target.'cfg(unix)'.dependencies]
host = { path = "../host", features = ["test-support"] }
[features]
demo = ["host/test-support"]
"#;
    assert_eq!(offenders(refused).len(), 4, "{:?}", offenders(refused));
}
