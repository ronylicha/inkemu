//! Order-sensitive 64-bit FNV-1a hash used to prove that two runs executed identically.

// FNV-1a 64-bit parameters, http://www.isthe.com/chongo/tech/comp/fnv/
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceHash(u64);

impl Default for TraceHash {
    fn default() -> Self {
        Self::new()
    }
}

impl TraceHash {
    #[must_use]
    pub const fn new() -> Self {
        TraceHash(FNV_OFFSET_BASIS)
    }

    pub fn mix_u32(&mut self, v: u32) {
        self.mix_bytes(&v.to_le_bytes());
    }

    pub fn mix_u64(&mut self, v: u64) {
        self.mix_bytes(&v.to_le_bytes());
    }

    fn mix_bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.mix_byte(*b);
        }
    }

    /// One FNV-1a step: xor the byte in, then multiply by the prime.
    fn mix_byte(&mut self, b: u8) {
        self.0 = (self.0 ^ u64::from(b)).wrapping_mul(FNV_PRIME);
    }

    #[must_use]
    pub const fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_hash_is_fnv_offset_basis() {
        assert_eq!(TraceHash::new().finish(), 0xcbf2_9ce4_8422_2325);
    }
    #[test]
    fn order_matters() {
        let mut a = TraceHash::new();
        a.mix_u32(1);
        a.mix_u32(2);
        let mut b = TraceHash::new();
        b.mix_u32(2);
        b.mix_u32(1);
        assert_ne!(a.finish(), b.finish());
    }

    /// Independent reference, written from the FNV-1a definition (not from the code under test).
    fn fnv1a64_reference(bytes: &[u8]) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    #[test]
    fn reference_matches_published_known_answer() {
        // FNV-1a 64-bit of the single byte 'a'.
        assert_eq!(fnv1a64_reference(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn single_byte_step_matches_known_answer() {
        let mut h = TraceHash::new();
        h.mix_byte(b'a');
        assert_eq!(h.finish(), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn mix_u32_matches_reference_over_little_endian_bytes() {
        let mut h = TraceHash::new();
        h.mix_u32(0x61);
        assert_eq!(h.finish(), fnv1a64_reference(&[0x61, 0x00, 0x00, 0x00]));
    }

    #[test]
    fn mix_u64_matches_reference_over_little_endian_bytes() {
        let mut h = TraceHash::new();
        h.mix_u64(0x0102_0304_0506_0708);
        assert_eq!(
            h.finish(),
            fnv1a64_reference(&[0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01])
        );
    }

    #[test]
    fn mix_u64_equals_two_mix_u32_halves_in_order() {
        let mut whole = TraceHash::new();
        whole.mix_u64(0x0000_0002_0000_0001);
        let mut halves = TraceHash::new();
        halves.mix_u32(0x0000_0001);
        halves.mix_u32(0x0000_0002);
        assert_eq!(whole.finish(), halves.finish());
    }
}
