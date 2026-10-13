//! The ban on host I/O of `firmware`, `boards` and `soc-esp32c3` lives in one `clippy.toml` per
//! crate, because clippy reads the one nearest to a crate and no other. Three copies can drift, and
//! an entry removed from all of them is silent: clippy then accepts the call. This test holds the
//! copies equal, holds the workspace settings in each, and names every call the ban must refuse.
#![allow(clippy::disallowed_methods)] // reads the clippy.toml files: it is a test of the build

use std::path::PathBuf;

/// The crates that do no host I/O: a graphical front end and the WebAssembly build use them.
const CRATES: [&str; 3] = ["firmware", "boards", "soc-esp32c3"];

/// Every path the ban refuses, by kind. A path missing from a `clippy.toml` is a call that
/// compiles again; a new one is added here and to each copy.
const METHODS: &[&str] = &[
    "std::fs::read",
    "std::fs::read_to_string",
    "std::fs::write",
    "std::fs::read_dir",
    "std::fs::metadata",
    "std::fs::symlink_metadata",
    "std::fs::exists",
    "std::fs::create_dir",
    "std::fs::create_dir_all",
    "std::fs::remove_file",
    "std::fs::remove_dir",
    "std::fs::remove_dir_all",
    "std::fs::rename",
    "std::fs::copy",
    "std::fs::hard_link",
    "std::fs::canonicalize",
    "std::fs::read_link",
    "std::fs::set_permissions",
    "std::env::var",
    "std::env::var_os",
    "std::env::vars",
    "std::env::vars_os",
    "std::env::args",
    "std::env::args_os",
    "std::env::current_dir",
    "std::env::set_current_dir",
    "std::env::current_exe",
    "std::env::home_dir",
    "std::env::temp_dir",
    "std::env::set_var",
    "std::env::remove_var",
    "std::time::SystemTime::now",
    "std::time::Instant::now",
    "std::process::exit",
    "std::process::abort",
    "std::process::id",
    "std::process::Command::new",
    "std::net::ToSocketAddrs::to_socket_addrs",
    "std::path::Path::exists",
    "std::path::Path::try_exists",
    "std::path::Path::is_file",
    "std::path::Path::is_dir",
    "std::path::Path::is_symlink",
    "std::path::Path::metadata",
    "std::path::Path::symlink_metadata",
    "std::path::Path::canonicalize",
    "std::path::Path::read_dir",
    "std::path::Path::read_link",
    "std::io::stdin",
    "std::io::stdout",
    "std::io::stderr",
    "std::thread::spawn",
    "std::thread::sleep",
    "std::thread::scope",
    "std::thread::Builder::spawn",
];

const TYPES: &[&str] = &[
    "std::fs::File",
    "std::fs::OpenOptions",
    "std::fs::ReadDir",
    "std::fs::DirEntry",
    "std::time::SystemTime",
    "std::time::Instant",
    "std::process::Command",
    "std::process::Child",
    "std::net::TcpStream",
    "std::net::TcpListener",
    "std::net::UdpSocket",
];

const MACROS: &[&str] = &[
    "std::print",
    "std::println",
    "std::eprint",
    "std::eprintln",
    "std::dbg",
];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &PathBuf) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e}; restore the file", path.display()))
}

fn parse(text: &str, origin: &str) -> toml::Table {
    text.parse()
        .unwrap_or_else(|e| panic!("{origin}: {e}; fix the TOML"))
}

/// The `path` of every entry of the array `key`, in the order of the file.
fn paths(table: &toml::Table, key: &str) -> Vec<String> {
    table
        .get(key)
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("path")?.as_str().map(str::to_owned))
        .collect()
}

#[test]
fn the_three_copies_of_the_ban_are_the_same_file() {
    let texts: Vec<(String, String)> = CRATES
        .iter()
        .map(|name| {
            let path = workspace_root()
                .join("crates")
                .join(name)
                .join("clippy.toml");
            (format!("crates/{name}/clippy.toml"), read(&path))
        })
        .collect();
    for (name, text) in &texts[1..] {
        assert_eq!(
            text, &texts[0].1,
            "{name} differs from {}; copy the change into every copy",
            texts[0].0
        );
    }
}

#[test]
fn each_copy_carries_the_workspace_settings() {
    let workspace = parse(&read(&workspace_root().join("clippy.toml")), "clippy.toml");
    assert!(
        !workspace.is_empty(),
        "the workspace clippy.toml holds no setting; the copies would have nothing to repeat"
    );
    for name in CRATES {
        let origin = format!("crates/{name}/clippy.toml");
        let copy = parse(&read(&workspace_root().join(&origin)), &origin);
        for (key, value) in &workspace {
            assert_eq!(
                copy.get(key),
                Some(value),
                "{origin} must repeat `{key}` of the workspace clippy.toml, which clippy does not \
                 read for this crate; add it"
            );
        }
    }
}

#[test]
fn each_copy_refuses_every_call_to_the_host() {
    for name in CRATES {
        let origin = format!("crates/{name}/clippy.toml");
        let copy = parse(&read(&workspace_root().join(&origin)), &origin);
        for (key, required) in [
            ("disallowed-methods", METHODS),
            ("disallowed-types", TYPES),
            ("disallowed-macros", MACROS),
        ] {
            let present = paths(&copy, key);
            for path in required {
                assert!(
                    present.iter().any(|p| p == path),
                    "{origin} does not refuse `{path}` in `{key}`; add it, and to the other copies"
                );
            }
        }
    }
}
