//! Which files of the profiles directory the build script embeds, and what it refuses. A file it
//! skipped without a word would be a device that is missing at run time, and a directory taken
//! for a profile would end in an error of the compiler far from the cause.
// builds directories to read; a failure of the helpers is a failure of the test
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::unwrap_used
)]

#[path = "../build_profiles.rs"]
mod build_profiles;

use std::path::PathBuf;

/// An empty directory of its own for the test `name`.
fn directory(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("build_profiles")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn file(dir: &std::path::Path, name: &str) {
    std::fs::write(dir.join(name), "id = \"x\"\n").unwrap();
}

#[test]
fn the_profiles_are_the_toml_files_sorted_by_id() {
    let dir = directory("sorted");
    for name in ["x4.toml", "a-1.toml", "x3.toml"] {
        file(&dir, name);
    }
    // Neither a profile nor a refusal: a file of another kind, a directory, a name with no
    // extension at all.
    file(&dir, "README.md");
    file(&dir, "notes");
    std::fs::create_dir(dir.join("backup")).unwrap();
    assert_eq!(
        build_profiles::profile_ids(&dir).unwrap(),
        ["a-1", "x3", "x4"]
    );
}

#[test]
fn an_extension_in_another_case_is_refused_not_skipped() {
    for extension in ["TOML", "Toml", "tOML"] {
        let dir = directory(&format!("case-{extension}"));
        file(&dir, "x3.toml");
        file(&dir, &format!("x9.{extension}"));
        let error = build_profiles::profile_ids(&dir).unwrap_err();
        assert!(
            error.ends_with(&format!(
                "x9.{extension}: the extension of a profile file is `.toml` in lower case, and the build would leave this file out; rename the file"
            )),
            "{error}"
        );
    }
}

#[test]
fn a_directory_named_like_a_profile_is_refused_with_its_cause() {
    let dir = directory("directory");
    file(&dir, "x3.toml");
    std::fs::create_dir(dir.join("d.toml")).unwrap();
    let error = build_profiles::profile_ids(&dir).unwrap_err();
    assert!(
        error.ends_with("d.toml: a profile is a file, and this is not one; rename or remove it"),
        "{error}"
    );
}

#[test]
fn a_name_that_is_not_a_device_id_is_refused_with_its_remedy() {
    for name in ["X9.toml", "x_9.toml", "x 9.toml", "é.toml"] {
        let dir = directory("name");
        file(&dir, name);
        let error = build_profiles::profile_ids(&dir).unwrap_err();
        assert!(
            error.ends_with(
                "a profile file is named after its device id, of a-z, 0-9 and `-`; rename the file"
            ),
            "{name}: {error}"
        );
    }
}

#[test]
fn a_directory_that_cannot_be_read_is_reported_with_its_path() {
    let dir = directory("missing");
    let missing = dir.join("no-such-directory");
    let error = build_profiles::profile_ids(&missing).unwrap_err();
    assert!(error.starts_with(&missing.display().to_string()), "{error}");
}
