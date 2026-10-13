//! Which files of the profiles directory are device profiles. Shared by the build script, which
//! embeds them, and by `tests/build_profiles.rs`, which a build script cannot have.
use std::path::Path;

/// A device id names a file and appears in messages: lower-case letters, digits and `-` only, as
/// the loader requires of the `id` key the profile must also carry.
fn is_device_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The ids of the profiles in `dir`, sorted: the order does not depend on the order the file
/// system lists the directory in. A file or directory whose name has no `toml` extension is not a
/// profile and is left alone.
///
/// # Errors
/// What a person must fix, cause then remedy: a name that is not a device id, an extension that
/// is `toml` in another case (the file would be silently left out), a directory named like a
/// profile file, or a directory that cannot be read.
pub fn profile_ids(dir: &Path) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut ids = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if !extension.eq_ignore_ascii_case("toml") {
            continue;
        }
        if extension != "toml" {
            return Err(format!(
                "{}: the extension of a profile file is `.toml` in lower case, and the build would leave this file out; rename the file",
                path.display()
            ));
        }
        // `metadata` follows a link: a link to a file is a file, a link to nothing is an error.
        let metadata = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !metadata.is_file() {
            return Err(format!(
                "{}: a profile is a file, and this is not one; rename or remove it",
                path.display()
            ));
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
    ids.sort();
    Ok(ids)
}
