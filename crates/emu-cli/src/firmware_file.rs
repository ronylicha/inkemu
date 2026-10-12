//! Reads the files given to `inkemu run --firmware` and `--bootloader`, and has the device
//! profile compose the flash the device boots from, before any machine exists (spec §5.1). The
//! checks and the composition belong to the profile (`boards::DeviceProfile::compose_flash`), so
//! that a front end gets the same answers; this module only reads files and names them.
use crate::flash_file::{ReadFailure, read_regular_file};
use crate::flash_state::Input;
use crate::term::sanitize;
use boards::DeviceProfile;
use firmware::{ComposeError, ComposedFlash};
use std::path::Path;

/// Why no flash can be composed from the files given. The messages state the cause, then the
/// remedy; the paths they echo have their control characters escaped.
#[derive(Debug, thiserror::Error)]
pub enum FirmwareFileError {
    #[error(
        "cannot read {path}, given to {input}: {detail}; check the path and its read permissions"
    )]
    Read {
        input: Input,
        path: String,
        detail: String,
    },
    #[error("{path}, given to {input}, is not a regular file; give the path of the file itself")]
    NotAFile { input: Input, path: String },
    #[error(
        "{path}, given to {input}, is {len} bytes, more than the {size} byte flash of the device, so it is no application nor bootloader of this device; give the .bin of a build, or a flash image of this device to --flash"
    )]
    TooLong {
        input: Input,
        path: String,
        len: u64,
        size: u32,
    },
    #[error(
        "{path}, given to {input}, holds at least {at_least} bytes (it grew while it was being read), more than the {size} byte flash of the device; give a file that nothing else writes"
    )]
    Grew {
        input: Input,
        path: String,
        at_least: u64,
        size: u32,
    },
    /// `what` is the file the refusal is about, or the device when it is about its profile.
    #[error("{what}: {source}")]
    Refused {
        what: String,
        #[source]
        source: ComposeError,
    },
}

fn shown(path: &Path) -> String {
    sanitize(&path.display().to_string())
}

/// The content of the file `path`, given to `input`: at most `limit` bytes, the flash size of the
/// device, of which no more than one more byte is read.
fn read(input: Input, path: &Path, limit: u32) -> Result<Vec<u8>, FirmwareFileError> {
    read_regular_file(path, limit).map_err(|failure| match failure {
        ReadFailure::Io(e) => FirmwareFileError::Read {
            input,
            path: shown(path),
            detail: sanitize(&e.to_string()),
        },
        ReadFailure::NotAFile => FirmwareFileError::NotAFile {
            input,
            path: shown(path),
        },
        ReadFailure::TooLong(len) => FirmwareFileError::TooLong {
            input,
            path: shown(path),
            len,
            size: limit,
        },
        ReadFailure::Grew(at_least) => FirmwareFileError::Grew {
            input,
            path: shown(path),
            at_least,
            size: limit,
        },
    })
}

/// The flash `profile` boots `firmware` from, behind `bootloader` when one is given and the
/// bootloader inkemu ships for the device otherwise.
///
/// # Errors
/// [`FirmwareFileError`]: a file cannot be read, or the profile refuses what it holds.
pub fn compose(
    profile: &DeviceProfile,
    firmware: &Path,
    bootloader: Option<&Path>,
) -> Result<ComposedFlash, FirmwareFileError> {
    let limit = profile.flash.size();
    let application = read(Input::Firmware, firmware, limit)?;
    let given = bootloader
        .map(|path| read(Input::Bootloader, path, limit))
        .transpose()?;
    profile
        .compose_flash(&application, given.as_deref())
        .map_err(|source| {
            let device = || format!("device {}", sanitize(&profile.id));
            let what = match (&source, bootloader) {
                (ComposeError::Application(_), _) => shown(firmware),
                (ComposeError::Bootloader(_), Some(path)) => shown(path),
                _ => device(),
            };
            FirmwareFileError::Refused { what, source }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use firmware::builder::{ImageSpec, bootloader_spec, build_flash_image, build_image};
    use firmware::{ApplicationError, BootloaderError};

    fn x4() -> DeviceProfile {
        boards::builtin("x4").unwrap()
    }

    fn file_with(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn an_application_is_composed_behind_the_shipped_bootloader_or_the_one_given() {
        let dir = tempfile::tempdir().unwrap();
        let app = build_image(&ImageSpec::default());
        let firmware = file_with(&dir, "app.bin", &app);
        let flash = compose(&x4(), &firmware, None).unwrap();
        assert_eq!(flash.bytes, x4().compose_flash(&app, None).unwrap().bytes);
        let given = build_image(&bootloader_spec());
        let bootloader = file_with(&dir, "bootloader.bin", &given);
        let flash = compose(&x4(), &firmware, Some(&bootloader)).unwrap();
        assert_eq!(&flash.bytes[..given.len()], &given[..]);
    }

    #[test]
    fn a_refusal_names_the_file_it_is_about() {
        let dir = tempfile::tempdir().unwrap();
        let app = file_with(&dir, "app.bin", &build_image(&ImageSpec::default()));
        let dump = file_with(&dir, "dump.bin", &build_flash_image(&ImageSpec::default()));
        let err = compose(&x4(), &dump, None).unwrap_err();
        assert!(
            matches!(
                &err,
                FirmwareFileError::Refused {
                    source: ComposeError::Application(ApplicationError::FlashImage),
                    what
                } if what.ends_with("dump.bin")
            ),
            "{err}"
        );
        assert!(
            err.to_string()
                .contains("; use it as the flash (`--flash`)"),
            "{err}"
        );
        let wrong = file_with(&dir, "wrong.bin", &build_image(&ImageSpec::default()));
        let err = compose(&x4(), &app, Some(&wrong)).unwrap_err();
        assert!(
            matches!(
                &err,
                FirmwareFileError::Refused {
                    source: ComposeError::Bootloader(BootloaderError::Application),
                    what
                } if what.ends_with("wrong.bin")
            ),
            "{err}"
        );
    }

    #[test]
    fn a_file_that_is_missing_too_long_or_no_file_is_refused_before_it_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let app = file_with(&dir, "app.bin", &build_image(&ImageSpec::default()));
        let err = compose(&x4(), &dir.path().join("none.bin"), None).unwrap_err();
        assert!(
            matches!(
                err,
                FirmwareFileError::Read {
                    input: Input::Firmware,
                    ..
                }
            ),
            "{err}"
        );
        assert!(err.to_string().contains("given to --firmware"), "{err}");
        let err = compose(&x4(), &app, Some(dir.path())).unwrap_err();
        assert!(
            matches!(
                err,
                FirmwareFileError::NotAFile {
                    input: Input::Bootloader,
                    ..
                }
            ),
            "{err}"
        );
        // A sparse file one byte longer than the flash: nothing that large is read.
        let big = dir.path().join("big.bin");
        std::fs::File::create(&big)
            .unwrap()
            .set_len((16 << 20) + 1)
            .unwrap();
        let err = compose(&x4(), &big, None).unwrap_err();
        assert!(
            matches!(
                err,
                FirmwareFileError::TooLong {
                    input: Input::Firmware,
                    len: 16_777_217,
                    size: 16_777_216,
                    ..
                }
            ),
            "{err}"
        );
    }
}
