//! SHA accelerator in typical (non-DMA) mode: SHA-1, SHA-224, SHA-256 — the three the ESP32-C3 has.
//! Register protocol: reports FB §B.7 and §F, BA §7 (ROM `ets_sha_process`, ESP-IDF `hwcrypto_reg.h`).
//! The compression functions come from the `sha1` and `sha2` crates (decisions D7 and D20); only the
//! register interface is ours.
use super::SHA_BASE;
use crate::diag::Unsupported;
use emu_core::Peripheral;
use std::collections::VecDeque;
use svd_regs::esp32c3::addr;

const MODE: u32 = addr("SHA", "MODE") - SHA_BASE;
const START: u32 = addr("SHA", "START") - SHA_BASE;
const CONTINUE: u32 = addr("SHA", "CONTINUE") - SHA_BASE;
const BUSY: u32 = addr("SHA", "BUSY") - SHA_BASE;
const DMA_BLOCK_NUM: u32 = addr("SHA", "DMA_BLOCK_NUM") - SHA_BASE;
const DMA_START: u32 = addr("SHA", "DMA_START") - SHA_BASE;
const DMA_CONTINUE: u32 = addr("SHA", "DMA_CONTINUE") - SHA_BASE;
const H_MEM0: u32 = addr("SHA", "H_MEM0") - SHA_BASE;
const M_MEM0: u32 = addr("SHA", "M_MEM0") - SHA_BASE;
const STATE_WORDS: usize = 8;
const BLOCK_WORDS: usize = 16;
const BLOCK_BYTES: usize = 64;
const SHA1_WORDS: usize = 5;
// INVARIANT: the SHA-1 state is the first five words of the eight-word register state.
const _: () = assert!(SHA1_WORDS <= STATE_WORDS);

/// Initial hash values, FIPS 180-4 §5.3.1 (SHA-1), §5.3.2 (SHA-224), §5.3.3 (SHA-256).
const SHA1_INITIAL: [u32; SHA1_WORDS] = [
    0x6745_2301,
    0xEFCD_AB89,
    0x98BA_DCFE,
    0x1032_5476,
    0xC3D2_E1F0,
];
const SHA224_INITIAL: [u32; STATE_WORDS] = [
    0xC105_9ED8,
    0x367C_D507,
    0x3070_DD17,
    0xF70E_5939,
    0xFFC0_0B31,
    0x6858_1511,
    0x64F9_8FA7,
    0xBEFA_4FA4,
];
const SHA256_INITIAL: [u32; STATE_WORDS] = [
    0x6A09_E667,
    0xBB67_AE85,
    0x3C6E_F372,
    0xA54F_F53A,
    0x510E_527F,
    0x9B05_688C,
    0x1F83_D9AB,
    0x5BE0_CD19,
];

/// `SHA.MODE` holds the `SHA_TYPE` software selects: `typedef enum { SHA1 = 0, SHA2_224, SHA2_256,
/// SHA_TYPE_MAX } SHA_TYPE;` (ESP-IDF v5.5.2
/// `components/esp_rom/esp32c3/include/esp32c3/rom/sha.h:17-22`), written as it is by
/// `sha_ll_set_mode`: `REG_WRITE(SHA_MODE_REG, sha_type)`
/// (`components/hal/esp32c3/include/hal/sha_ll.h:55`). The value 2 is also measured (FB §B.7).
#[derive(Clone, Copy)]
enum Algorithm {
    Sha1,
    Sha224,
    Sha256,
}

impl Algorithm {
    const fn from_mode(mode: u32) -> Option<Algorithm> {
        match mode {
            0 => Some(Algorithm::Sha1),
            1 => Some(Algorithm::Sha224),
            2 => Some(Algorithm::Sha256),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Sha {
    mode: u32,
    /// `H_MEM0..7` as the registers read: each is a state word with its bytes swapped.
    state: [u32; STATE_WORDS],
    /// `M_MEM0..15` as written.
    block: [u32; BLOCK_WORDS],
    /// Refusals not taken yet, oldest first: a refusal is never overwritten by a later one.
    unsupported: VecDeque<Unsupported>,
}

/// Index of `offset` in a block of `count` words starting at `first`.
fn word_index(offset: u32, first: u32, count: usize) -> Option<usize> {
    let index = usize::try_from(offset.checked_sub(first)? / 4).ok()?;
    (index < count).then_some(index)
}

impl Sha {
    #[must_use]
    pub fn new() -> Sha {
        Sha::default()
    }

    pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> {
        self.unsupported.pop_front()
    }

    /// Compresses the block in `M_MEM`; `fresh` starts from the algorithm's initial value.
    ///
    /// ASSUMPTION: `CONTINUE` under a `MODE` other than the one the hash was started with compresses
    /// the block with the algorithm `MODE` now selects, from whatever `H_MEM` holds (its first five
    /// words for SHA-1, the three others being left as they are). The ROM writes the mode before
    /// every block, and the same one for a whole hash (FB §B.7), so no measured path decides it;
    /// the SP1b-4 lockstep, or a device, settles it.
    fn compress(&mut self, fresh: bool) {
        let Some(algorithm) = Algorithm::from_mode(self.mode) else {
            self.unsupported
                .push_back(Unsupported::ShaMode { mode: self.mode });
            return;
        };
        // The message bytes are the bytes of the M_MEM words in memory order (FB §F).
        let mut block = [0u8; BLOCK_BYTES];
        for (chunk, word) in block.as_chunks_mut::<4>().0.iter_mut().zip(self.block) {
            *chunk = word.to_le_bytes();
        }
        // The state words are the H_MEM registers with their bytes swapped (FB §F).
        let mut h = self.state.map(u32::swap_bytes);
        match algorithm {
            Algorithm::Sha1 => {
                let mut h1 = if fresh {
                    SHA1_INITIAL
                } else {
                    [h[0], h[1], h[2], h[3], h[4]]
                };
                sha1::block_api::compress(&mut h1, &[block]);
                h[..SHA1_WORDS].copy_from_slice(&h1);
            }
            Algorithm::Sha224 | Algorithm::Sha256 => {
                if fresh {
                    h = if matches!(algorithm, Algorithm::Sha224) {
                        SHA224_INITIAL
                    } else {
                        SHA256_INITIAL
                    };
                }
                sha2::block_api::compress256(&mut h, &[block]);
            }
        }
        self.state = h.map(u32::swap_bytes);
    }
}

impl Peripheral for Sha {
    fn read(&mut self, offset: u32) -> Option<u32> {
        if let Some(index) = word_index(offset, H_MEM0, STATE_WORDS) {
            return self.state.get(index).copied();
        }
        if let Some(index) = word_index(offset, M_MEM0, BLOCK_WORDS) {
            return self.block.get(index).copied();
        }
        match offset {
            MODE => Some(self.mode),
            // A block is hashed inside the write that triggers it: never busy (FB §C row 14).
            BUSY => Some(0),
            _ => None,
        }
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        if let Some(slot) =
            word_index(offset, H_MEM0, STATE_WORDS).and_then(|i| self.state.get_mut(i))
        {
            *slot = value;
            return true;
        }
        if let Some(slot) =
            word_index(offset, M_MEM0, BLOCK_WORDS).and_then(|i| self.block.get_mut(i))
        {
            *slot = value;
            return true;
        }
        match offset {
            MODE => self.mode = value,
            START | CONTINUE => {
                if value & 1 != 0 {
                    self.compress(offset == START);
                }
            }
            // Decision D7: DMA-SHA is not modelled; a write that configures it (the block count) or
            // starts it is reported, never silently dropped.
            DMA_BLOCK_NUM | DMA_START | DMA_CONTINUE => {
                self.unsupported.push_back(Unsupported::ShaDma);
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    const MODE_SHA1: u32 = 0;
    const MODE_SHA224: u32 = 1;
    const MODE_SHA256: u32 = 2;

    /// Writes one 64-byte block the way the ROM's `memcpy` does: 16 little-endian words.
    fn write_block(sha: &mut Sha, block: &[u8]) {
        for (word, chunk) in (0u32..).zip(block.chunks(4)) {
            assert!(sha.write(
                M_MEM0 + 4 * word,
                u32::from_le_bytes(chunk.try_into().unwrap())
            ));
        }
    }
    /// The padded message: 0x80, zeros, 64-bit big-endian bit length (what `ets_sha_finish` builds).
    fn padded(message: &[u8]) -> Vec<u8> {
        let mut bytes = message.to_vec();
        bytes.push(0x80);
        while bytes.len() % 64 != 56 {
            bytes.push(0);
        }
        bytes.extend((message.len() as u64 * 8).to_be_bytes());
        bytes
    }
    /// Hashes `message` through the registers and returns the first `len` digest bytes.
    fn hash(mode: u32, message: &[u8], len: usize) -> Vec<u8> {
        let mut sha = Sha::new();
        for (index, block) in padded(message).chunks(64).enumerate() {
            assert!(sha.write(MODE, mode));
            write_block(&mut sha, block);
            assert_eq!(sha.read(BUSY), Some(0), "the ROM loops while BUSY reads 1");
            assert!(sha.write(if index == 0 { START } else { CONTINUE }, 1));
        }
        assert_eq!(sha.take_unsupported(), None);
        (0..8u32)
            .flat_map(|i| sha.read(H_MEM0 + 4 * i).unwrap().to_le_bytes())
            .take(len)
            .collect()
    }

    #[test]
    fn sha256_of_abc_matches_the_fips_180_4_example() {
        // FIPS 180-4 example: SHA-256("abc") = ba7816bf 8f01cfea 414140de 5dae2223 b00361a3 96177a9c b410ff61 f20015ad.
        let mut sha = Sha::new();
        sha.write(MODE, MODE_SHA256);
        sha.write(M_MEM0, 0x8063_6261); // bytes 61 62 63 80
        sha.write(M_MEM0 + 4 * 15, 0x1800_0000); // bit length 24, big-endian, in the last word
        sha.write(START, 1);
        assert_eq!(
            sha.read(H_MEM0),
            Some(0xBF16_78BA),
            "digest bytes ba 78 16 bf in memory order"
        );
        assert_eq!(sha.read(H_MEM0 + 4 * 7), Some(0xAD15_00F2));
    }

    #[test]
    fn digest_words_are_the_digest_bytes_in_memory_order() {
        // FB §F: H_MEM read as words gives the digest bytes in order (`0x1c59639a` for `9a 63 59 1c…`).
        let message = b"abc";
        assert_eq!(
            hash(MODE_SHA256, message, 32),
            sha2::Sha256::digest(message).to_vec()
        );
    }

    #[test]
    fn the_three_algorithms_of_the_c3_agree_with_their_reference_over_several_blocks() {
        // 200 bytes: four blocks with the padding, so CONTINUE runs three times.
        let message: Vec<u8> = (0..200u8).collect();
        assert_eq!(
            hash(MODE_SHA256, &message, 32),
            sha2::Sha256::digest(&message).to_vec()
        );
        assert_eq!(
            hash(MODE_SHA224, &message, 28),
            sha2::Sha224::digest(&message).to_vec()
        );
        assert_eq!(
            hash(MODE_SHA1, &message, 20),
            sha1::Sha1::digest(&message).to_vec()
        );
    }

    #[test]
    fn sha1_and_sha224_of_abc_match_their_published_digests() {
        // FIPS 180-4 examples: SHA-1 a9993e36 4706816a ba3e2571 7850c26c 9cd0d89d;
        // SHA-224 23097d22 3405d822 8642a477 bda255b3 2aadbce4 bda0b3f7 e36c9da7.
        assert_eq!(hash(MODE_SHA1, b"abc", 4), [0xa9, 0x99, 0x3e, 0x36]);
        assert_eq!(hash(MODE_SHA224, b"abc", 4), [0x23, 0x09, 0x7d, 0x22]);
    }

    #[test]
    fn continue_without_start_hashes_from_the_stored_state() {
        // The ROM restores a saved state by writing H_MEM, then CONTINUE (FB §B.7, note under the table).
        let message: Vec<u8> = (0..100u8).collect();
        let blocks = padded(&message);
        let mut first = Sha::new();
        first.write(MODE, MODE_SHA256);
        write_block(&mut first, &blocks[..64]);
        first.write(START, 1);
        let saved: Vec<u32> = (0..8u32)
            .map(|i| first.read(H_MEM0 + 4 * i).unwrap())
            .collect();

        let mut resumed = Sha::new();
        resumed.write(MODE, MODE_SHA256);
        for (i, word) in (0u32..).zip(&saved) {
            assert!(resumed.write(H_MEM0 + 4 * i, *word));
        }
        write_block(&mut resumed, &blocks[64..128]);
        resumed.write(CONTINUE, 1);
        let digest: Vec<u8> = (0..8u32)
            .flat_map(|i| resumed.read(H_MEM0 + 4 * i).unwrap().to_le_bytes())
            .collect();
        assert_eq!(digest, sha2::Sha256::digest(&message).to_vec());
    }

    fn state(sha: &mut Sha) -> [u32; STATE_WORDS] {
        std::array::from_fn(|i| sha.read(H_MEM0 + 4 * u32::try_from(i).unwrap()).unwrap())
    }

    #[test]
    fn a_restored_sha1_state_continues_and_leaves_the_three_last_state_words_alone() {
        // SHA-1 has five state words; `H_MEM5..7` are not part of it (FB §B.7).
        const MARKERS: [u32; 3] = [0xAAAA_0005, 0xBBBB_0006, 0xCCCC_0007];
        let message: Vec<u8> = (0..100u8).collect();
        let blocks = padded(&message);
        let mut first = Sha::new();
        first.write(MODE, MODE_SHA1);
        write_block(&mut first, &blocks[..64]);
        first.write(START, 1);
        let saved = state(&mut first);

        let mut resumed = Sha::new();
        resumed.write(MODE, MODE_SHA1);
        for (i, word) in (0u32..).zip(saved.iter().take(SHA1_WORDS).chain(&MARKERS)) {
            assert!(resumed.write(H_MEM0 + 4 * i, *word));
        }
        write_block(&mut resumed, &blocks[64..128]);
        resumed.write(CONTINUE, 1);
        let after = state(&mut resumed);
        let digest: Vec<u8> = after[..SHA1_WORDS]
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect();
        assert_eq!(digest, sha1::Sha1::digest(&message).to_vec());
        assert_eq!(after[SHA1_WORDS..], MARKERS, "not SHA-1 state: untouched");
        assert_eq!(resumed.take_unsupported(), None);
    }

    #[test]
    fn continue_under_another_mode_uses_the_new_algorithm_on_the_stored_state() {
        // Pins the ASSUMPTION documented on `Sha::compress`.
        let block: [u8; 64] = padded(b"abc").try_into().unwrap();
        let mut sha = Sha::new();
        sha.write(MODE, MODE_SHA256);
        write_block(&mut sha, &block);
        sha.write(START, 1);
        let started = state(&mut sha);

        // SHA-256 to SHA-224: the same compression function, from the stored state, not from the
        // SHA-224 initial value.
        sha.write(MODE, MODE_SHA224);
        sha.write(CONTINUE, 1);
        let mut expected = started.map(u32::swap_bytes);
        sha2::block_api::compress256(&mut expected, &[block]);
        assert_eq!(state(&mut sha), expected.map(u32::swap_bytes));
        let mut restarted = Sha::new();
        restarted.write(MODE, MODE_SHA224);
        write_block(&mut restarted, &block);
        restarted.write(START, 1);
        assert_ne!(state(&mut sha), state(&mut restarted));

        // To SHA-1: its compression function on the first five words; the others stay.
        let before = state(&mut sha);
        sha.write(MODE, MODE_SHA1);
        sha.write(CONTINUE, 1);
        let mut five: [u32; SHA1_WORDS] = std::array::from_fn(|i| before[i].swap_bytes());
        sha1::block_api::compress(&mut five, &[block]);
        let after = state(&mut sha);
        assert_eq!(after[..SHA1_WORDS], five.map(u32::swap_bytes));
        assert_eq!(after[SHA1_WORDS..], before[SHA1_WORDS..]);
        assert_eq!(
            sha.take_unsupported(),
            None,
            "a known mode is never refused"
        );
    }

    #[test]
    fn a_start_or_continue_written_as_zero_does_nothing() {
        let mut sha = Sha::new();
        sha.write(MODE, MODE_SHA256);
        sha.write(START, 0);
        assert_eq!(sha.read(H_MEM0), Some(0));
    }

    #[test]
    fn unknown_mode_is_reported_and_leaves_the_state() {
        let mut sha = Sha::new();
        sha.write(H_MEM0, 0x1234_5678);
        sha.write(MODE, 5);
        assert!(sha.write(START, 1));
        assert_eq!(
            sha.take_unsupported(),
            Some(Unsupported::ShaMode { mode: 5 })
        );
        assert_eq!(
            sha.read(H_MEM0),
            Some(0x1234_5678),
            "no digest is presented as valid"
        );
        assert_eq!(sha.read(MODE), Some(5));
    }

    #[test]
    fn dma_start_is_reported() {
        let mut sha = Sha::new();
        for name in ["DMA_START", "DMA_CONTINUE", "DMA_BLOCK_NUM"] {
            let offset = svd_regs::esp32c3::addr("SHA", name) - SHA_BASE;
            assert!(sha.write(offset, 1), "{name}");
            assert_eq!(sha.take_unsupported(), Some(Unsupported::ShaDma), "{name}");
        }
    }

    #[test]
    fn refusals_not_taken_yet_are_all_kept_in_order() {
        let mut sha = Sha::new();
        sha.write(MODE, 7);
        sha.write(START, 1);
        sha.write(DMA_START, 1);
        sha.write(CONTINUE, 1);
        assert_eq!(
            sha.take_unsupported(),
            Some(Unsupported::ShaMode { mode: 7 })
        );
        assert_eq!(sha.take_unsupported(), Some(Unsupported::ShaDma));
        assert_eq!(
            sha.take_unsupported(),
            Some(Unsupported::ShaMode { mode: 7 })
        );
        assert_eq!(sha.take_unsupported(), None, "drained");
    }

    #[test]
    fn registers_outside_the_typical_mode_stay_unmodelled() {
        let mut sha = Sha::new();
        for name in ["T_STRING", "T_LENGTH", "CLEAR_IRQ", "IRQ_ENA", "DATE"] {
            let offset = svd_regs::esp32c3::addr("SHA", name) - SHA_BASE;
            assert_eq!(sha.read(offset), None, "{name}");
        }
        assert_eq!(sha.read(START), None, "START is write-only");
    }
}
