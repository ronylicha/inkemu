//! Human-readable description of a firmware image.
use firmware::{ChipId, FirmwareImage};
use std::fmt::Write as _;

/// ESP32-C3 address ranges (Technical Reference Manual, "System and Memory").
const ESP32C3_REGIONS: &[(u32, u32, &str)] = &[
    (0x3C00_0000, 0x3C80_0000, "DROM (flash data)"),
    (0x3FC8_0000, 0x3FCE_0000, "DRAM"),
    (0x4037_C000, 0x403E_0000, "IRAM"),
    (0x4200_0000, 0x4280_0000, "IROM (flash code)"),
    (0x5000_0000, 0x5000_2000, "RTC fast memory"),
];

/// Names the memory region holding `addr`; other chips show `-` until their chip model exists (SP7).
fn region(chip: ChipId, addr: u32) -> &'static str {
    if chip != ChipId::Esp32C3 {
        return "-";
    }
    ESP32C3_REGIONS
        .iter()
        .find(|(lo, hi, _)| (*lo..*hi).contains(&addr))
        .map_or("unknown", |&(_, _, name)| name)
}

/// Escapes control characters (C0, DEL, C1) so untrusted text cannot drive the terminal.
///
/// Firmware metadata and paths are attacker-controlled; a raw ESC or CSI byte in them
/// could inject escape sequences. Printable text, non-ASCII letters included, is unchanged.
#[must_use]
pub fn sanitize(text: &str) -> String {
    text.chars()
        .fold(String::with_capacity(text.len()), |mut out, c| {
            if c.is_control() {
                out.extend(c.escape_default());
            } else {
                out.push(c);
            }
            out
        })
}

/// Describes a validated image: target chip, flash settings, application and segments.
#[must_use]
pub fn render(img: &FirmwareImage, file_name: &str, file_len: usize) -> String {
    let file_name = sanitize(file_name);
    let mut s = String::new();
    // Writing to a String cannot fail, so the results are ignored.
    let _ = writeln!(s, "File:        {file_name} ({file_len} bytes)");
    let _ = writeln!(s, "Format:      ESP-IDF application image");
    let _ = writeln!(s, "Chip:        {}", img.chip);
    let _ = writeln!(s, "Entry point: {:#010x}", img.entry);
    let _ = writeln!(
        s,
        "Flash:       {} MB, {:?}, frequency code {:#x}",
        img.flash_size.megabytes(),
        img.spi_mode,
        img.flash_freq_raw
    );
    let hash = if img.hash_appended {
        "SHA-256 appended (verified)"
    } else {
        "none (checksum verified)"
    };
    let _ = writeln!(s, "Hash:        {hash}");
    match &img.app_desc {
        Some(d) => {
            let _ = writeln!(
                s,
                "App:         {} {} (ESP-IDF {}), built {} {}",
                sanitize(&d.project_name),
                sanitize(&d.version),
                sanitize(&d.idf_ver),
                sanitize(&d.date),
                sanitize(&d.time)
            );
        }
        None => {
            let _ = writeln!(s, "App:         no application descriptor");
        }
    }
    let _ = writeln!(s, "Segments:");
    let _ = writeln!(s, "  #  load address  size       region");
    for (i, seg) in img.segments.iter().enumerate() {
        let _ = writeln!(
            s,
            "  {i:<2} {:#010x}    {:<10} {}",
            seg.load_addr,
            seg.data.len(),
            region(img.chip, seg.load_addr)
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_escapes_every_kind_of_control_character() {
        assert_eq!(sanitize("a\u{1b}[31m"), "a\\u{1b}[31m");
        assert_eq!(sanitize("\u{7}"), "\\u{7}");
        assert_eq!(sanitize("x\r"), "x\\r");
        assert_eq!(sanitize("\u{9b}"), "\\u{9b}");
        assert_eq!(sanitize("\u{7f}"), "\\u{7f}");
    }

    #[test]
    fn sanitize_leaves_printable_text_unchanged() {
        assert_eq!(sanitize("café 1.0"), "café 1.0");
    }

    #[test]
    fn esp32c3_regions_are_named_and_gaps_are_unknown() {
        assert_eq!(region(ChipId::Esp32C3, 0x3C00_0020), "DROM (flash data)");
        assert_eq!(region(ChipId::Esp32C3, 0x4038_0000), "IRAM");
        assert_eq!(region(ChipId::Esp32C3, 0x1000_0000), "unknown");
    }

    #[test]
    fn other_chips_have_no_region_names_yet() {
        assert_eq!(region(ChipId::Esp32S3, 0x3C00_0020), "-");
    }
}
