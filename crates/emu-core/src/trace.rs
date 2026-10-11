//! Order-sensitive 64-bit FNV-1a hash used to prove that two runs executed identically.
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
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(FNV_PRIME);
        }
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
}
