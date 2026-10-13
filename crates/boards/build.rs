//! Lists the built-in device profiles: every `profiles/<id>.toml`, sorted by id, so that a new
//! device is one file and no line of code. The profiles stay embedded with `include_str!`, which
//! the WebAssembly build needs, and the list depends only on the names of the files.
// A build script runs on the host at compile time; reading the profiles directory is its job. The
// crate's own code does no host I/O (`clippy.toml`).
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fmt::Write as _;
use std::path::Path;

/// A device id names a file and appears in messages: lower-case letters, digits and `-` only, as
/// the loader requires of the `id` key the profile must also carry.
fn is_device_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn main() -> Result<(), String> {
    println!("cargo:rerun-if-changed=profiles");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?;
    let dir = Path::new(&manifest).join("profiles");
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut ids = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| is_device_id(stem))
            .ok_or_else(|| {
                format!(
                    "{}: a profile file is named after its device id, of a-z, 0-9 and `-`; rename the file",
                    path.display()
                )
            })?
            .to_owned();
        ids.push(id);
    }
    // Sorted: the order does not depend on the order the file system lists the directory in.
    ids.sort();
    let mut list = format!("const BUILTIN: [(&str, &str); {}] = [\n", ids.len());
    for id in &ids {
        // `id` holds only a-z, 0-9 and `-`: it needs no escaping, in a string or a path.
        writeln!(
            list,
            "    (\"{id}\", include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/profiles/{id}.toml\"))),"
        )
        .map_err(|e| e.to_string())?;
    }
    list.push_str("];\n");
    let out = std::env::var("OUT_DIR").map_err(|e| e.to_string())?;
    let target = Path::new(&out).join("builtin_profiles.rs");
    std::fs::write(&target, list).map_err(|e| format!("{}: {e}", target.display()))
}
