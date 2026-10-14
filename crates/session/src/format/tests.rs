use super::*;
use crate::outcome::{Cut, CutReason};

/// A device of 1 MiB of flash, the least a profile takes, so that each read inflates little.
const TINY_PROFILE: &str = r#"
id = "tiny"
name = "Tiny test device"
console = "uart0"

[soc]
family = "esp32c3"
revision = "v0.3"

[flash]
size_mib = 1
jedec_id = 0x9D6014

[[partition]]
name = "factory"
type = "app"
subtype = "factory"
offset = 0x10000
size = 0xF0000
"#;

const MIB: usize = 1 << 20;

fn outcome(end: End) -> Outcome {
    Outcome {
        end,
        device_time: Cycles(1_600),
        cycles: 200,
        instructions: 190,
        trace_hash: 0x0123_4567_89ab_cdef,
        flash_sha256: [7; 32],
        consoles: [[1; 32], [2; 32], [3; 32]],
    }
}

/// A session of every kind of field: three consoles and the USB host, both limits, a cut.
fn session() -> Session {
    let mut flash = vec![0xFF; MIB];
    flash[..4].copy_from_slice(&[0xE9, 1, 2, 3]);
    Session {
        rom: RomIdentity {
            revision: 3,
            sha256: [9; 32],
        },
        profile: ProfileText {
            id: "tiny".to_owned(),
            text: TINY_PROFILE.to_owned(),
        },
        flash,
        limits: Limits {
            strict_mmio: true,
            max_cycles: Some(1_000_000),
            max_time_nanos: None,
            cut: Some(Cut {
                at: Cycles(1_600),
                reason: CutReason::ReaderGone,
            }),
        },
        usb_host: UsbHost::Attached,
        inputs: vec![
            AcceptedInput::Console {
                channel: ConsoleChannel::Uart0,
                at: Cycles(10),
                bytes: b"a\r".to_vec(),
            },
            AcceptedInput::UsbHost {
                at: Cycles(20),
                state: UsbHost::Open,
            },
            AcceptedInput::Console {
                channel: ConsoleChannel::UsbSerialJtag,
                at: Cycles(30),
                bytes: Vec::new(),
            },
            AcceptedInput::Console {
                channel: ConsoleChannel::Uart1,
                at: Cycles(5),
                bytes: b"z".to_vec(),
            },
        ],
        outcome: outcome(End::Cut),
    }
}

/// Where the fields of `bytes`, written from [`session`], begin.
struct Layout {
    id_len: usize,
    text_len: usize,
    profile_sha: usize,
    flash_size: usize,
    flash_sha: usize,
    deflated_len: usize,
    strict: usize,
    max_cycles: usize,
    max_time: usize,
    cut: usize,
    usb_host: usize,
    count: usize,
    first_input: usize,
    end: usize,
}

fn layout(bytes: &[u8]) -> Layout {
    let id_len = MAGIC.len() + 2 + 32;
    let text_len = id_len + 1 + "tiny".len();
    let profile_sha = text_len + 4 + TINY_PROFILE.len();
    let flash_size = profile_sha + 32;
    let flash_sha = flash_size + 4;
    let deflated_len = flash_sha + 32;
    let deflated = u32::from_le_bytes(bytes[deflated_len..deflated_len + 4].try_into().unwrap());
    let strict = deflated_len + 4 + deflated as usize;
    Layout {
        id_len,
        text_len,
        profile_sha,
        flash_size,
        flash_sha,
        deflated_len,
        strict,
        max_cycles: strict + 1,
        max_time: strict + 10,
        cut: strict + 11,
        usb_host: strict + 20,
        count: strict + 21,
        first_input: strict + 25,
        end: bytes.len() - OUTCOME_BYTES,
    }
}

fn read(bytes: &[u8]) -> ReadError {
    Session::from_bytes(bytes).unwrap_err()
}

fn patched(at: usize, value: &[u8]) -> Vec<u8> {
    let mut bytes = session().to_bytes().unwrap();
    bytes[at..at + value.len()].copy_from_slice(value);
    bytes
}

/// The bytes of [`session`] with the byte at `at` inverted.
fn flipped(at: usize) -> Vec<u8> {
    let mut bytes = session().to_bytes().unwrap();
    bytes[at] ^= 0xFF;
    bytes
}

#[test]
fn a_session_goes_to_bytes_and_back_unchanged() {
    let session = session();
    let bytes = session.to_bytes().unwrap();
    assert!(bytes.starts_with(b"inkemu-session 1\n"));
    assert_eq!(Session::from_bytes(&bytes).unwrap(), session);
    // The erased flash deflates to a little: the file is small.
    assert!(bytes.len() < 8 * 1024, "{}", bytes.len());
    // The same session gives the same bytes.
    assert_eq!(session.to_bytes().unwrap(), bytes);
}

#[test]
fn a_session_of_another_version_or_of_no_session_is_refused() {
    let mut other = session().to_bytes().unwrap();
    other[MAGIC.len() - 2] = b'2';
    assert_eq!(
        read(&other),
        ReadError::OtherVersion {
            found: "inkemu-session 2".to_owned()
        }
    );
    assert_eq!(read(b"hello"), ReadError::NotASession);
    assert_eq!(read(b"inkemu-session\n"), ReadError::NotASession);
    let message = read(&other).to_string();
    assert!(
        message.contains("reads `inkemu-session 1` only"),
        "{message}"
    );
}

#[test]
fn a_session_cut_short_anywhere_is_refused_with_the_field_it_ends_in() {
    let bytes = session().to_bytes().unwrap();
    let layout = layout(&bytes);
    for len in 0..bytes.len() {
        match read(&bytes[..len]) {
            ReadError::Truncated { .. } => {}
            other => panic!("cut at {len}: {other}"),
        }
    }
    for (len, field) in [
        (0, Field::Version),
        (5, Field::Version),
        (layout.id_len, Field::ProfileId),
        (layout.text_len + 6, Field::ProfileText),
        (layout.flash_sha + 1, Field::FlashSha256),
        (layout.deflated_len + 5, Field::DeflatedFlash),
        (layout.max_cycles + 3, Field::MaxCycles),
        (layout.cut + 1, Field::Cut),
        (layout.first_input + 2, Field::Input(0)),
        (layout.end + 3, Field::DeviceTime),
        (bytes.len() - 1, Field::ConsoleSha256(ConsoleChannel::Uart1)),
    ] {
        assert_eq!(read(&bytes[..len]), ReadError::Truncated { field }, "{len}");
    }
    assert_eq!(
        read(&bytes[..layout.end + 3]).to_string(),
        "the file ends inside the device time; it was cut short: record the run again"
    );
}

#[test]
fn each_length_one_past_its_bound_is_refused_before_anything_is_read_from_it() {
    let layout = layout(&session().to_bytes().unwrap());
    let too_large = |at: usize, value: &[u8]| match read(&patched(at, value)) {
        ReadError::TooLarge { field, .. } => field,
        other => panic!("{other}"),
    };
    let id = u8::try_from(MAX_PROFILE_ID_BYTES + 1).unwrap();
    assert_eq!(too_large(layout.id_len, &[id]), Field::ProfileId);
    let text = u32::try_from(MAX_PROFILE_TEXT_BYTES + 1).unwrap();
    assert_eq!(
        too_large(layout.text_len, &text.to_le_bytes()),
        Field::ProfileText
    );
    assert_eq!(
        too_large(layout.flash_size, &(MAX_FLASH_BYTES + 1).to_le_bytes()),
        Field::FlashSize
    );
    let deflated = u32::try_from(MAX_DEFLATED_FLASH_BYTES + 1).unwrap();
    assert_eq!(
        too_large(layout.deflated_len, &deflated.to_le_bytes()),
        Field::DeflatedFlash
    );
    let count = u32::try_from(MAX_INPUTS + 1).unwrap();
    assert_eq!(
        too_large(layout.count, &count.to_le_bytes()),
        Field::InputCount
    );
    let len = u32::try_from(MAX_CONSOLE_BYTES + 1).unwrap();
    assert_eq!(
        too_large(layout.first_input + 9, &len.to_le_bytes()),
        Field::Input(0)
    );
    // A count the file does not hold ends on the first input that is not there: the bytes after
    // the inputs are no input, and then there are none left.
    for count in [5, u32::try_from(MAX_INPUTS).unwrap()] {
        let lying = patched(layout.count, &count.to_le_bytes());
        assert!(
            matches!(
                read(&lying),
                ReadError::Truncated {
                    field: Field::Input(_)
                } | ReadError::Invalid {
                    field: Field::Input(4),
                    ..
                }
            ),
            "{count}: {}",
            read(&lying)
        );
    }
    // A flash below the least a device has.
    assert_eq!(
        read(&patched(
            layout.flash_size,
            &(MIN_FLASH_BYTES - 1).to_le_bytes()
        )),
        ReadError::Invalid {
            field: Field::FlashSize,
            value: u64::from(MIN_FLASH_BYTES - 1)
        }
    );
}

#[test]
fn the_inputs_of_a_channel_or_the_host_beyond_their_bound_are_refused() {
    let mut beyond = session();
    beyond.inputs = vec![
        AcceptedInput::Console {
            channel: ConsoleChannel::Uart0,
            at: Cycles(0),
            bytes: vec![b'x'; MAX_CONSOLE_BYTES],
        },
        AcceptedInput::Console {
            channel: ConsoleChannel::Uart0,
            at: Cycles(1),
            bytes: Vec::new(),
        },
    ];
    let field = Field::ConsoleInputs(ConsoleChannel::Uart0);
    assert!(matches!(
        beyond.to_bytes(),
        Err(WriteError::TooLarge { field: f, .. }) if f == field
    ));
    assert!(matches!(
        read(&beyond.encode().unwrap()),
        ReadError::TooLarge { field: f, .. } if f == field
    ));
    beyond.inputs = (0..=MAX_HOST_CHANGES as u64)
        .map(|at| AcceptedInput::UsbHost {
            at: Cycles(at),
            state: UsbHost::Open,
        })
        .collect();
    assert!(matches!(
        read(&beyond.encode().unwrap()),
        ReadError::TooLarge {
            field: Field::HostChanges,
            ..
        }
    ));
    assert!(matches!(
        beyond.to_bytes(),
        Err(WriteError::TooLarge {
            field: Field::HostChanges,
            ..
        })
    ));
}

#[test]
fn a_sha256_that_does_not_match_is_refused_with_its_field() {
    let layout = layout(&session().to_bytes().unwrap());
    let mismatch = |bytes: Vec<u8>| match read(&bytes) {
        ReadError::Sha256Mismatch { field, .. } => field,
        other => panic!("{other}"),
    };
    // A byte of the text changed, or of its recorded SHA-256.
    assert_eq!(
        mismatch(patched(layout.text_len + 10, b"#")),
        Field::ProfileText
    );
    assert_eq!(mismatch(flipped(layout.profile_sha)), Field::ProfileText);
    assert_eq!(mismatch(flipped(layout.flash_sha)), Field::FlashSha256);
}

#[test]
fn a_flash_that_does_not_inflate_to_its_size_or_is_of_another_device_is_refused() {
    let layout = layout(&session().to_bytes().unwrap());
    let two = u32::try_from(2 * MIB).unwrap();
    assert_eq!(
        read(&patched(layout.flash_size, &two.to_le_bytes())),
        ReadError::InflatedSize {
            expected: two,
            found: MIB
        }
    );
    // A deflated stream that does not start as one.
    let garbage = patched(layout.deflated_len + 4, &[0xFF; 8]);
    assert!(matches!(
        read(&garbage),
        ReadError::Inflate { .. } | ReadError::InflatedSize { .. }
    ));
    let mut other = session();
    other.flash = vec![0xFF; 2 * MIB];
    assert_eq!(
        read(&other.encode().unwrap()),
        ReadError::FlashOfAnotherDevice {
            flash: two,
            device: u32::try_from(MIB).unwrap()
        }
    );
    assert!(matches!(
        other.to_bytes(),
        Err(WriteError::FlashOfAnotherDevice { .. })
    ));
}

#[test]
fn a_value_no_field_takes_is_refused_with_its_field() {
    let bytes = session().to_bytes().unwrap();
    let layout = layout(&bytes);
    for (at, value, field) in [
        (layout.strict, 2u8, Field::StrictMmio),
        (layout.max_cycles, 2, Field::MaxCycles),
        (layout.max_time, 7, Field::MaxTime),
        (layout.cut, 4, Field::Cut),
        (layout.usb_host, 3, Field::UsbHost),
        (layout.first_input, 4, Field::Input(0)),
        (layout.end, 6, Field::End),
    ] {
        assert_eq!(
            read(&patched(at, &[value])),
            ReadError::Invalid {
                field,
                value: value.into()
            },
            "{field}"
        );
    }
    // The host of input 1 (after the 2 bytes of input 0).
    let host = layout.first_input + 13 + 2 + 9;
    assert_eq!(
        read(&patched(host, &[9])),
        ReadError::Invalid {
            field: Field::Input(1),
            value: 9
        }
    );
    // A profile id that is not UTF-8.
    assert!(matches!(
        read(&patched(layout.id_len + 1, &[0xFF])),
        ReadError::Invalid {
            field: Field::ProfileId,
            ..
        }
    ));
}

#[test]
fn bytes_after_the_end_or_a_file_longer_than_any_session_are_refused() {
    let mut bytes = session().to_bytes().unwrap();
    bytes.push(0);
    assert_eq!(read(&bytes), ReadError::TrailingBytes { count: 1 });
    assert_eq!(read(&vec![0u8; MAX_SESSION_BYTES + 1]), ReadError::TooLong);
}

#[test]
fn an_end_that_does_not_agree_with_the_cut_is_refused() {
    let mut uncut = session();
    uncut.limits.cut = None;
    assert_eq!(
        read(&uncut.encode().unwrap()),
        ReadError::EndAndCut {
            end: End::Cut,
            cut: false
        }
    );
    assert!(matches!(
        uncut.to_bytes(),
        Err(WriteError::EndAndCut { .. })
    ));
    let mut cut = session();
    cut.outcome.end = End::TimeLimit;
    assert_eq!(
        read(&cut.encode().unwrap()),
        ReadError::EndAndCut {
            end: End::TimeLimit,
            cut: true
        }
    );
}

#[test]
fn a_profile_the_session_cannot_rebuild_is_refused() {
    let mut other = session();
    other.profile.id = "x4".to_owned();
    assert!(matches!(
        read(&other.encode().unwrap()),
        ReadError::Profile {
            source: ProfileError::IdMismatch { .. }
        }
    ));
    assert!(matches!(other.to_bytes(), Err(WriteError::Profile { .. })));
}

#[test]
fn the_longest_session_is_within_the_bound_the_front_end_reads() {
    // Each channel at its bound in one-byte inputs, the costliest way to spend it.
    const PER_CHANNEL: usize = MAX_CONSOLE_BYTES * (CONSOLE_INPUT_HEAD + 1);
    const { assert!(MAX_SESSION_BYTES > 3 * PER_CHANNEL + MAX_DEFLATED_FLASH_BYTES) };
    // A front end may hold a whole file in memory.
    const { assert!(MAX_SESSION_BYTES < 64 * MIB) };
}
