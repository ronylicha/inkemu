//! `svd-gen`: turns the vendored ESP32-C3 SVD into `crates/svd-regs/src/esp32c3.rs`.
//!
//! `cargo run -p svd-gen` writes the file; `cargo run -p svd-gen -- --check` exits 1 when
//! the file on disk differs from what the generator would write.
mod emit;
mod expand;

use expand::{Alias, GenError};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{fs, io};

/// Repository-relative path of the SVD, as written in the generated header.
const SVD_SOURCE_NAME: &str = "third_party/espressif-svd/esp32c3.svd";
/// Repository-relative path of the output, as shown in messages.
const OUTPUT_DISPLAY_NAME: &str = "crates/svd-regs/src/esp32c3.rs";
/// Both paths below are relative to this crate's manifest directory.
const SVD_PATH_FROM_MANIFEST: &str = "../../third_party/espressif-svd/esp32c3.svd";
const OUTPUT_PATH_FROM_MANIFEST: &str = "../svd-regs/src/esp32c3.rs";
const CHECK_FLAG: &str = "--check";
/// Digest of `espressif/svd` release 2024-05-09 (`third_party/espressif-svd/PROVENANCE.md`).
const ESP32C3_SVD_SHA256: &str = "9ef33dbff56d807996fd8edfa4aa4ad008b5a861c241d36280caa0dedbbca41a";

const EXIT_STALE: u8 = 1;
const EXIT_FAILURE: u8 = 2;

/// `RNG_DATA_REG` and `SYSCON_RND_DATA_REG` are the same register in ESP-IDF; the SVD
/// lists it under both `APB_CTRL.RND_DATA` and `RNG.DATA`, and the dedicated RNG
/// peripheral wins. Any other address collision is an error: a future SVD bump must
/// force a conscious decision (research report, section 4.2).
const ALIASES: [Alias; 1] = [Alias {
    addr: 0x6002_60B0,
    peripheral: "RNG",
    register: "DATA",
}];

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("unknown argument `{arg}`; the only accepted argument is {CHECK_FLAG}")]
    Usage { arg: String },
    #[error("cannot read {path} ({source}); run the command from a complete repository checkout")]
    Read { path: String, source: io::Error },
    #[error("cannot write {path} ({source}); check that its directory exists and is writable")]
    Write { path: String, source: io::Error },
    #[error(
        "{SVD_SOURCE_NAME} has sha256 {found}, expected {ESP32C3_SVD_SHA256}; restore the file from the URL in PROVENANCE.md"
    )]
    SvdDigest { found: String },
    #[error(transparent)]
    Generate(#[from] GenError),
    #[error("{OUTPUT_DISPLAY_NAME} is missing or stale; run cargo run -p svd-gen")]
    Stale,
}

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Write,
    Check,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Mode, CliError> {
    let mut mode = Mode::Write;
    for arg in args {
        if arg != CHECK_FLAG {
            return Err(CliError::Usage { arg });
        }
        mode = Mode::Check;
    }
    Ok(mode)
}

/// Lower-case hexadecimal of `bytes`, the form the pinned digest is written in.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(2 * bytes.len());
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// Verifies the SVD digest, expands it and renders the register table.
fn generate(svd_xml: &str) -> Result<String, CliError> {
    let found = hex(&Sha256::digest(svd_xml.as_bytes()));
    if found != ESP32C3_SVD_SHA256 {
        return Err(CliError::SvdDigest { found });
    }
    let registers = expand::expand(svd_xml, &ALIASES)?;
    Ok(emit::emit(SVD_SOURCE_NAME, ESP32C3_SVD_SHA256, &registers))
}

fn manifest_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn run(mode: &Mode) -> Result<(), CliError> {
    let svd_path = manifest_path(SVD_PATH_FROM_MANIFEST);
    let svd_xml = fs::read_to_string(&svd_path).map_err(|source| CliError::Read {
        path: svd_path.display().to_string(),
        source,
    })?;
    let generated = generate(&svd_xml)?;

    let output_path = manifest_path(OUTPUT_PATH_FROM_MANIFEST);
    match mode {
        Mode::Write => fs::write(&output_path, generated).map_err(|source| CliError::Write {
            path: output_path.display().to_string(),
            source,
        }),
        Mode::Check => check(&generated, &output_path),
    }
}

/// `--check`: the file at `output_path` must be exactly `generated`. A file that does not exist is as
/// stale as one that differs: the generator has to be run.
fn check(generated: &str, output_path: &Path) -> Result<(), CliError> {
    match fs::read_to_string(output_path) {
        Ok(on_disk) if on_disk == generated => Ok(()),
        Ok(_) => Err(CliError::Stale),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(CliError::Stale),
        Err(source) => Err(CliError::Read {
            path: output_path.display().to_string(),
            source,
        }),
    }
}

fn main() -> ExitCode {
    let outcome = parse_args(std::env::args().skip(1)).and_then(|mode| run(&mode));
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("svd-gen: {error}");
            ExitCode::from(if matches!(error, CliError::Stale) {
                EXIT_STALE
            } else {
                EXIT_FAILURE
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vendored_svd() -> String {
        fs::read_to_string(manifest_path(SVD_PATH_FROM_MANIFEST)).unwrap()
    }

    #[test]
    fn vendored_svd_generates_the_documented_2605_registers() {
        let generated = generate(&vendored_svd()).unwrap();
        assert!(
            generated.contains("[RegisterInfo; 2605] = ["),
            "{}",
            &generated[..400]
        );
        assert!(generated.contains(
            "RegisterInfo { addr: 0x6002_60b0, peripheral: \"RNG\", name: \"DATA\", reset_value: 0x0000_0000, reset_explicit: false },"
        ));
        assert!(!generated.contains("name: \"RND_DATA\""));
    }

    #[test]
    fn vendored_svd_registers_are_sorted_with_unique_addresses() {
        let registers = expand::expand(&vendored_svd(), &ALIASES).unwrap();
        assert!(registers.windows(2).all(|pair| pair[0].addr < pair[1].addr));
    }

    #[test]
    fn a_modified_svd_is_refused_before_anything_is_generated() {
        let tampered = vendored_svd() + "<!-- edited -->";
        assert!(matches!(
            generate(&tampered),
            Err(CliError::SvdDigest { .. })
        ));
    }

    #[test]
    fn check_accepts_the_identical_file_and_calls_a_different_or_missing_one_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("esp32c3.rs");
        assert!(
            matches!(check("table", &path), Err(CliError::Stale)),
            "missing"
        );
        fs::write(&path, "an older table").unwrap();
        assert!(
            matches!(check("table", &path), Err(CliError::Stale)),
            "different"
        );
        fs::write(&path, "table").unwrap();
        assert!(check("table", &path).is_ok());
        let message = check("table", &dir.path().join("absent"))
            .unwrap_err()
            .to_string();
        assert!(message.contains("run cargo run -p svd-gen"), "{message}");
    }

    #[test]
    fn arguments_select_the_mode_and_reject_anything_else() {
        assert_eq!(parse_args([]).unwrap(), Mode::Write);
        assert_eq!(parse_args(["--check".to_owned()]).unwrap(), Mode::Check);
        let err = parse_args(["--force".to_owned()]).unwrap_err().to_string();
        assert!(err.contains("--force") && err.contains("--check"), "{err}");
    }
}
