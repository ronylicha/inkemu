//! The two CRCs of the SD protocol (PLS 9.10 §4.5): CRC7 over a command and over the CID and
//! CSD registers, CRC16 over a data block. Both start from zero, most significant bit first.

/// CRC7, polynomial x⁷ + x³ + 1, of `bytes`: the seven bits, in the low bits of the result.
#[must_use]
pub fn crc7(bytes: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &byte in bytes {
        for bit in (0..8).rev() {
            let incoming = (byte >> bit) & 1;
            let top = (crc >> 6) & 1;
            crc = (crc << 1) & 0x7F;
            if incoming ^ top == 1 {
                crc ^= 0x09;
            }
        }
    }
    crc
}

/// The last byte of a command or of a register: its CRC7 and the end bit.
#[must_use]
pub fn crc7_byte(bytes: &[u8]) -> u8 {
    crc7(bytes) << 1 | 1
}

/// The table of the CRC16: the remainder of each byte.
const CRC16_TABLE: [u16; 256] = {
    let mut table = [0u16; 256];
    let mut byte = 0;
    while byte < 256 {
        // INVARIANT: `byte` is below 256.
        #[allow(clippy::cast_possible_truncation)]
        let mut crc = (byte as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000 != 0 {
                crc << 1 ^ 0x1021
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[byte] = crc;
        byte += 1;
    }
    table
};

/// CRC16 (CCITT), polynomial x¹⁶ + x¹² + x⁵ + 1, of `bytes`.
#[must_use]
pub fn crc16(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0u16, |crc, &byte| {
        let index = usize::from(crc >> 8 ^ u16::from(byte));
        crc << 8 ^ CRC16_TABLE.get(index).copied().unwrap_or(0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crc7_of_the_commands_a_host_sends_without_computing_it() {
        // PLS §7.2.2: "A valid reset command is: 0x40, 0x0, 0x0, 0x0, 0x0, 0x95".
        assert_eq!(crc7(&[0x40, 0, 0, 0, 0]), 0x4A);
        assert_eq!(crc7_byte(&[0x40, 0, 0, 0, 0]), 0x95);
        // SdFat sends 87h with CMD8 1AAh (`SdSpiCard.cpp:281`).
        assert_eq!(crc7_byte(&[0x48, 0, 0, 0x01, 0xAA]), 0x87);
        assert_eq!(crc7_byte(&[0x77, 0, 0, 0, 0]), 0x65);
        assert_eq!(crc7_byte(&[0x69, 0x40, 0, 0, 0]), 0x77);
        assert_eq!(crc7(&[]), 0);
    }

    #[test]
    fn the_crc16_of_known_blocks() {
        assert_eq!(crc16(&[0xFF; 512]), 0x7FA1);
        assert_eq!(crc16(&[0; 512]), 0);
        let counting: Vec<u8> = (0..=255).chain(0..=255).collect();
        assert_eq!(crc16(&counting), 0x40DA);
        // The check value of CRC-16/XMODEM, the same polynomial from zero.
        assert_eq!(crc16(b"123456789"), 0x31C3);
    }
}
