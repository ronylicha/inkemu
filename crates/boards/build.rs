//! Lists the built-in device profiles: every `profiles/<id>.toml`, sorted by id, so that a new
//! device is one file and no line of code. The profiles stay embedded with `include_str!`, which
//! the WebAssembly build needs, and the list depends only on the names of the files.
// A build script runs on the host at compile time; reading the profiles directory is its job. The
// crate's own code does no host I/O (`clippy.toml`). Its output to cargo is `println!`.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros
)]

use std::fmt::Write as _;
use std::path::Path;

mod build_profiles;

fn main() -> Result<(), String> {
    println!("cargo:rerun-if-changed=profiles");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?;
    let ids = build_profiles::profile_ids(&Path::new(&manifest).join("profiles"))?;
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
