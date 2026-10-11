//! `esp_app_desc_t`, found at the start of the first segment (ESP-IDF `esp_app_format.h`).

/// `ESP_APP_DESC_MAGIC_WORD`: first word of the descriptor.
pub const APP_DESC_MAGIC: u32 = 0xABCD_5432;
/// Size of `esp_app_desc_t`; a segment shorter than this cannot hold one.
const LEN: usize = 256;
const MAGIC: usize = 0;
const SECURE_VERSION: usize = 4;
/// String fields are `(offset, field size)`, NUL-terminated within the field.
const VERSION: (usize, usize) = (16, 32);
const PROJECT_NAME: (usize, usize) = (48, 32);
const TIME: (usize, usize) = (80, 16);
const DATE: (usize, usize) = (96, 16);
const IDF_VER: (usize, usize) = (112, 32);
const ELF_SHA256: usize = 144;
const SHA256_LEN: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppDesc {
    pub secure_version: u32,
    pub version: String,
    pub project_name: String,
    pub time: String,
    pub date: String,
    pub idf_ver: String,
    pub elf_sha256: [u8; 32],
}

fn c_string(bytes: &[u8], (at, len): (usize, usize)) -> String {
    let field = &bytes[at..at + len];
    let end = field.iter().position(|&b| b == 0).unwrap_or(len);
    String::from_utf8_lossy(&field[..end]).into_owned()
}

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

impl AppDesc {
    /// Returns `None` when the segment does not start with an app descriptor.
    #[must_use]
    pub fn parse(segment: &[u8]) -> Option<AppDesc> {
        if segment.len() < LEN || le_u32(segment, MAGIC) != APP_DESC_MAGIC {
            return None;
        }
        let mut elf_sha256 = [0u8; SHA256_LEN];
        elf_sha256.copy_from_slice(&segment[ELF_SHA256..ELF_SHA256 + SHA256_LEN]);
        Some(AppDesc {
            secure_version: le_u32(segment, SECURE_VERSION),
            version: c_string(segment, VERSION),
            project_name: c_string(segment, PROJECT_NAME),
            time: c_string(segment, TIME),
            date: c_string(segment, DATE),
            idf_ver: c_string(segment, IDF_VER),
            elf_sha256,
        })
    }
}
