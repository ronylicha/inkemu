//! Header enumerations of ESP-IDF images (values from esptool and `esp_app_format.h`).
use std::fmt;

/// Target `SoC`, from `chip_id` in the image header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipId {
    Esp32,
    Esp32S2,
    Esp32C3,
    Esp32S3,
    Esp32C2,
    Esp32C6,
    Esp32H2,
    Unknown(u16),
}

impl ChipId {
    #[must_use]
    pub const fn from_raw(raw: u16) -> ChipId {
        match raw {
            0 => ChipId::Esp32,
            2 => ChipId::Esp32S2,
            5 => ChipId::Esp32C3,
            9 => ChipId::Esp32S3,
            12 => ChipId::Esp32C2,
            13 => ChipId::Esp32C6,
            16 => ChipId::Esp32H2,
            other => ChipId::Unknown(other),
        }
    }

    #[must_use]
    pub fn name(&self) -> String {
        match self {
            ChipId::Esp32 => "ESP32".into(),
            ChipId::Esp32S2 => "ESP32-S2".into(),
            ChipId::Esp32C3 => "ESP32-C3".into(),
            ChipId::Esp32S3 => "ESP32-S3".into(),
            ChipId::Esp32C2 => "ESP32-C2".into(),
            ChipId::Esp32C6 => "ESP32-C6".into(),
            ChipId::Esp32H2 => "ESP32-H2".into(),
            ChipId::Unknown(id) => format!("unknown chip (id {id})"),
        }
    }
}

impl fmt::Display for ChipId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// Flash size, from the high nibble of header byte 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FlashSize {
    Mb1,
    Mb2,
    Mb4,
    Mb8,
    Mb16,
    Mb32,
    Mb64,
    Mb128,
}

impl FlashSize {
    /// Codes 0..=7 map to 1 MB..=128 MB (powers of two); other codes are invalid.
    #[must_use]
    pub const fn from_nibble(n: u8) -> Option<FlashSize> {
        Some(match n {
            0 => FlashSize::Mb1,
            1 => FlashSize::Mb2,
            2 => FlashSize::Mb4,
            3 => FlashSize::Mb8,
            4 => FlashSize::Mb16,
            5 => FlashSize::Mb32,
            6 => FlashSize::Mb64,
            7 => FlashSize::Mb128,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn megabytes(self) -> u32 {
        1 << (self as u32)
    }

    #[must_use]
    pub const fn bytes(self) -> u64 {
        (self.megabytes() as u64) * 1024 * 1024
    }
}

/// SPI flash read mode, from header byte 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpiMode {
    Qio,
    Qout,
    Dio,
    Dout,
    FastRead,
    SlowRead,
}

impl SpiMode {
    #[must_use]
    pub const fn from_raw(raw: u8) -> Option<SpiMode> {
        Some(match raw {
            0 => SpiMode::Qio,
            1 => SpiMode::Qout,
            2 => SpiMode::Dio,
            3 => SpiMode::Dout,
            4 => SpiMode::FastRead,
            5 => SpiMode::SlowRead,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_chip_ids_map_to_their_names() {
        let expected = [
            (0, "ESP32"),
            (2, "ESP32-S2"),
            (5, "ESP32-C3"),
            (9, "ESP32-S3"),
            (12, "ESP32-C2"),
            (13, "ESP32-C6"),
            (16, "ESP32-H2"),
        ];
        for (raw, name) in expected {
            assert_eq!(ChipId::from_raw(raw).name(), name);
        }
    }

    #[test]
    fn unknown_chip_id_is_named_with_its_number() {
        assert_eq!(ChipId::from_raw(7), ChipId::Unknown(7));
        assert_eq!(ChipId::Unknown(7).to_string(), "unknown chip (id 7)");
    }

    #[test]
    fn flash_size_codes_are_powers_of_two_megabytes() {
        let sizes = [
            (0, 1),
            (1, 2),
            (2, 4),
            (3, 8),
            (4, 16),
            (5, 32),
            (6, 64),
            (7, 128),
        ];
        for (nibble, mb) in sizes {
            let size = FlashSize::from_nibble(nibble).unwrap();
            assert_eq!(size.megabytes(), mb);
            assert_eq!(size.bytes(), u64::from(mb) * 1024 * 1024);
        }
        assert_eq!(FlashSize::from_nibble(8), None);
    }

    #[test]
    fn spi_mode_codes_follow_esptool_order() {
        assert_eq!(SpiMode::from_raw(0), Some(SpiMode::Qio));
        assert_eq!(SpiMode::from_raw(1), Some(SpiMode::Qout));
        assert_eq!(SpiMode::from_raw(4), Some(SpiMode::FastRead));
        assert_eq!(SpiMode::from_raw(3), Some(SpiMode::Dout));
        assert_eq!(SpiMode::from_raw(5), Some(SpiMode::SlowRead));
        assert_eq!(SpiMode::from_raw(6), None);
    }
}
