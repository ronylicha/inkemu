//! Reads the file given to `inkemu run --flash` and refuses, before any machine exists, what cannot
//! be the flash content of the device (spec §5.1).
use crate::term::sanitize;
use firmware::{ChipId, FirmwareError};
use std::io::Read;
use std::path::Path;

/// Why the file given to `--flash` cannot be the flash of the device. The messages state the cause,
/// then the remedy; the path they echo has its control characters escaped.
#[derive(Debug, thiserror::Error)]
pub enum FlashFileError {
    #[error(
        "cannot read the flash image {path}: {detail}; check the path and its read permissions"
    )]
    Read { path: String, detail: String },
    #[error(
        "the flash image {path} is not a regular file; use the path of a flash dump or of a merged image"
    )]
    NotAFile { path: String },
    #[error(
        "the flash image {path} is empty; use a full flash dump, or an image merged with `esptool.py merge_bin`"
    )]
    Empty { path: String },
    #[error(
        "the flash image {path} is {len} bytes, longer than the {size} byte flash of the device; use an image built for this flash size, or a device whose flash is large enough"
    )]
    TooLong { path: String, len: u64, size: u32 },
    #[error("{path}: {source}")]
    Refused {
        path: String,
        #[source]
        source: FirmwareError,
    },
}

/// The content of the regular file `path`, which must hold at most `flash_size` bytes and pass the
/// checks of a full ESP32-C3 flash image. A path that is not a regular file is refused without being
/// opened (a pipe would block), and no more than `flash_size + 1` bytes are ever read.
///
/// # Errors
/// [`FlashFileError`], whose message names the file, the cause and the remedy.
pub fn load(path: &Path, flash_size: u32) -> Result<Vec<u8>, FlashFileError> {
    let shown = sanitize(&path.display().to_string());
    let read_error = |e: &std::io::Error| FlashFileError::Read {
        path: shown.clone(),
        detail: sanitize(&e.to_string()),
    };
    let metadata = std::fs::metadata(path).map_err(|e| read_error(&e))?;
    if !metadata.is_file() {
        return Err(FlashFileError::NotAFile { path: shown });
    }
    if metadata.len() > u64::from(flash_size) {
        return Err(FlashFileError::TooLong {
            path: shown,
            len: metadata.len(),
            size: flash_size,
        });
    }
    let file = std::fs::File::open(path).map_err(|e| read_error(&e))?;
    // The path may have been replaced between the two calls: judge what was actually opened.
    if !file.metadata().map_err(|e| read_error(&e))?.is_file() {
        return Err(FlashFileError::NotAFile { path: shown });
    }
    let mut bytes = Vec::new();
    // One byte past the flash: a file that grew since the metadata was read is still caught.
    file.take(u64::from(flash_size) + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| read_error(&e))?;
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if len > u64::from(flash_size) {
        return Err(FlashFileError::TooLong {
            path: shown,
            len,
            size: flash_size,
        });
    }
    if bytes.is_empty() {
        return Err(FlashFileError::Empty { path: shown });
    }
    firmware::check_flash_image(&bytes, ChipId::Esp32C3, u64::from(flash_size)).map_err(
        |source| FlashFileError::Refused {
            path: shown,
            source,
        },
    )?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use firmware::builder::{ImageSpec, build_flash_image};

    const MIB16: u32 = 16 << 20;

    fn file_with(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flash.bin");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    #[test]
    fn a_merged_image_is_loaded_as_it_is() {
        let image = build_flash_image(&ImageSpec::default());
        let (_dir, path) = file_with(&image);
        assert_eq!(load(&path, MIB16).unwrap(), image);
    }

    #[test]
    fn a_missing_file_says_which_and_what_to_check() {
        let err = load(Path::new("/nonexistent/flash.bin"), MIB16)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("/nonexistent/flash.bin") && err.contains("; check the path"),
            "{err}"
        );
    }

    #[test]
    fn empty_file_and_directory_are_refused() {
        let (dir, path) = file_with(&[]);
        let err = load(&path, MIB16).unwrap_err();
        assert!(matches!(err, FlashFileError::Empty { .. }), "{err}");
        assert!(err.to_string().contains("; use a full flash dump"), "{err}");
        let err = load(dir.path(), MIB16).unwrap_err();
        assert!(matches!(err, FlashFileError::NotAFile { .. }), "{err}");
    }

    #[test]
    fn image_longer_than_the_flash_is_refused_with_both_sizes() {
        // A sparse file one byte longer than the flash: nothing that large is read into memory.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(u64::from(MIB16) + 1)
            .unwrap();
        let err = load(&path, MIB16).unwrap_err();
        assert!(
            matches!(
                err,
                FlashFileError::TooLong {
                    len: 16_777_217,
                    size: MIB16,
                    ..
                }
            ),
            "{err}"
        );
        let text = err.to_string();
        assert!(
            text.contains("16777217") && text.contains("16777216") && text.contains("; use"),
            "{text}"
        );
    }

    #[test]
    fn a_refusal_of_the_content_names_the_file_and_keeps_the_reason() {
        let (_dir, path) = file_with(&build_flash_image(&ImageSpec {
            chip: 9,
            ..ImageSpec::default()
        }));
        let err = load(&path, MIB16).unwrap_err();
        assert!(
            matches!(
                err,
                FlashFileError::Refused {
                    source: firmware::FirmwareError::ChipMismatch { .. },
                    ..
                }
            ),
            "{err}"
        );
        let text = err.to_string();
        assert!(
            text.contains("flash.bin") && text.contains("ESP32-S3") && text.contains("ESP32-C3"),
            "{text}"
        );
    }

    #[test]
    fn a_path_with_control_characters_is_shown_escaped() {
        let err = load(Path::new("/nonexistent/\u{1b}[31mred.bin"), MIB16)
            .unwrap_err()
            .to_string();
        assert!(!err.contains('\u{1b}'), "{err:?}");
    }
}
