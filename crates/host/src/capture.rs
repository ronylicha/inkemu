//! A capture of the panel: a PNG of the image as the user of the device sees it (design of
//! sub-project 2c, C13).
//!
//! The capture holds the panel's own pixels, one per pixel, turned by the rotation of the panel in
//! the device: it does not depend on the size of a window or on a scaling. It is an 8-bit
//! greyscale PNG, compressed with zlib, with no metadata.
use crate::ports::{PanelFrame, Rotation};
use crate::session_file::{Claimed, OutputRefusal, check_output_path, write_through_temporary};
use crate::term::sanitize;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::io::Write as _;
use std::path::Path;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// The pixels of `frame` turned by `rotation`, and their width and height.
#[must_use]
pub fn rotated(frame: &PanelFrame, rotation: Rotation) -> (u32, u32, Vec<u8>) {
    let (width, height) = (frame.width as usize, frame.height as usize);
    let at = |x: usize, y: usize| frame.pixels.get(y * width + x).copied().unwrap_or(0);
    match rotation {
        Rotation::Deg0 => (frame.width, frame.height, frame.pixels.to_vec()),
        Rotation::Deg180 => {
            let pixels = (0..height)
                .flat_map(|y| (0..width).map(move |x| (width - 1 - x, height - 1 - y)))
                .map(|(x, y)| at(x, y))
                .collect();
            (frame.width, frame.height, pixels)
        }
        // Clockwise: the first line of the image is the first column of the panel, read from
        // its bottom.
        Rotation::Deg90 => {
            let pixels = (0..width)
                .flat_map(|x| (0..height).map(move |y| (x, height - 1 - y)))
                .map(|(x, y)| at(x, y))
                .collect();
            (frame.height, frame.width, pixels)
        }
        Rotation::Deg270 => {
            let pixels = (0..width)
                .flat_map(|x| (0..height).map(move |y| (width - 1 - x, y)))
                .map(|(x, y)| at(x, y))
                .collect();
            (frame.height, frame.width, pixels)
        }
    }
}

/// The CRC-32 of PNG (ISO 3309, polynomial `0xEDB88320`, reflected).
fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in parts.iter().flat_map(|part| part.iter()) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[&kind, data]).to_be_bytes());
}

/// The PNG of `frame` as the user of the device sees it.
#[must_use]
pub fn png(frame: &PanelFrame, rotation: Rotation) -> Vec<u8> {
    let (width, height, pixels) = rotated(frame, rotation);
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    // 8 bits, greyscale, deflate, the standard filters, no interlace.
    header.extend_from_slice(&[8, 0, 0, 0, 0]);
    let line = width as usize;
    let mut raw = Vec::with_capacity(pixels.len() + height as usize);
    for row in pixels.chunks(line.max(1)) {
        // Filter 0: the line as it is.
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    // Writing to a `Vec` cannot fail.
    let _ = encoder.write_all(&raw);
    let data = encoder.finish().unwrap_or_default();
    let mut out = Vec::with_capacity(data.len() + 64);
    out.extend_from_slice(&SIGNATURE);
    chunk(&mut out, *b"IHDR", &header);
    chunk(&mut out, *b"IDAT", &data);
    chunk(&mut out, *b"IEND", &[]);
    out
}

/// Why a capture was not written. The messages state the cause, then the remedy; the paths they
/// echo have their control characters escaped.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("the panel has shown no image yet; capture it once it has")]
    NoImage,
    #[error("the capture cannot be written to {path}: {reason}; choose another path")]
    Refused { path: String, reason: String },
    #[error(
        "cannot write the capture {path}: {detail}; check that the directory exists and is writable"
    )]
    Write { path: String, detail: String },
}

/// Writes the PNG of `frame`, turned by `rotation`, to `path` through a temporary file beside it,
/// unless the path is one of `claimed`, the files the run reads or writes: a capture never
/// replaces the flash file, its lock, the ROM or a session.
///
/// # Errors
/// [`CaptureError`] when the path is refused or the file cannot be written.
pub fn save(
    frame: &PanelFrame,
    rotation: Rotation,
    path: &Path,
    claimed: &[Claimed],
) -> Result<(), CaptureError> {
    let shown = sanitize(&path.display().to_string());
    check_output_path(path, claimed).map_err(|refusal| CaptureError::Refused {
        path: shown.clone(),
        reason: match refusal {
            OutputRefusal::NoFileName => "it names no file".to_owned(),
            OutputRefusal::NotAFileToWrite => "a directory or a link stands there".to_owned(),
            OutputRefusal::NoDirectory => "its directory does not exist".to_owned(),
            OutputRefusal::Claimed { what } => format!("it is {what}"),
            OutputRefusal::TemporaryClaimed { temporary, what } => {
                format!("it is written first to {temporary}, which is {what}")
            }
        },
    })?;
    // A capture is a picture of the screen, which its user shares: readable as the umask allows.
    write_through_temporary(path, &png(frame, rotation), 0o644).map_err(|detail| {
        CaptureError::Write {
            path: shown,
            detail,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::RefreshKind;
    use flate2::read::ZlibDecoder;
    use std::io::Read as _;
    use std::sync::Arc;

    /// 3 × 2: a, b, c / d, e, f.
    fn frame() -> PanelFrame {
        PanelFrame {
            width: 3,
            height: 2,
            gray_levels: 4,
            generation: 1,
            refresh: RefreshKind::Full,
            region: None,
            duration_nanos: None,
            pixels: Arc::from([1u8, 2, 3, 4, 5, 6].as_slice()),
        }
    }

    #[test]
    fn a_rotation_turns_the_image_clockwise() {
        assert_eq!(
            rotated(&frame(), Rotation::Deg0),
            (3, 2, vec![1, 2, 3, 4, 5, 6])
        );
        // a b c      d a
        // d e f  ->  e b
        //            f c
        assert_eq!(
            rotated(&frame(), Rotation::Deg90),
            (2, 3, vec![4, 1, 5, 2, 6, 3])
        );
        assert_eq!(
            rotated(&frame(), Rotation::Deg180),
            (3, 2, vec![6, 5, 4, 3, 2, 1])
        );
        assert_eq!(
            rotated(&frame(), Rotation::Deg270),
            (2, 3, vec![3, 6, 2, 5, 1, 4])
        );
    }

    /// The CRC of the IEND chunk is fixed by the standard.
    #[test]
    fn the_crc_is_that_of_png() {
        assert_eq!(crc32(&[b"IEND"]), 0xAE42_6082);
        assert_eq!(crc32(&[b"123456789"]), 0xCBF4_3926);
    }

    /// The PNG holds the signature, a greyscale header of the turned size, the pixels behind a
    /// filter byte per line, and the end.
    #[test]
    fn the_png_holds_the_turned_pixels() {
        let bytes = png(&frame(), Rotation::Deg90);
        assert_eq!(bytes[..8], SIGNATURE);
        assert_eq!(&bytes[12..16], b"IHDR");
        assert_eq!(bytes[16..20], 2u32.to_be_bytes());
        assert_eq!(bytes[20..24], 3u32.to_be_bytes());
        assert_eq!(bytes[24..29], [8, 0, 0, 0, 0]);
        assert_eq!(&bytes[37..41], b"IDAT");
        let len = u32::from_be_bytes(bytes[33..37].try_into().unwrap()) as usize;
        let mut raw = Vec::new();
        ZlibDecoder::new(&bytes[41..41 + len])
            .read_to_end(&mut raw)
            .unwrap();
        assert_eq!(raw, [0, 4, 1, 0, 5, 2, 0, 6, 3]);
        assert_eq!(&bytes[bytes.len() - 8..bytes.len() - 4], b"IEND");
    }
}
