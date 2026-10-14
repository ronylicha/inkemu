//! A session recorded from a run that goes through a software reset, a deep sleep, inputs on two
//! consoles and changes of the USB host, then replayed (G7): the same outcome, to the trace hash,
//! whatever the slices of the replay, read back from its bytes or not; a cut run replays to its
//! cut; a changed input diverges.
//!
//! The recording hands its inputs as a console script does: those before the first wait at
//! power-on, the others when the console shows what they wait for. The replay hands them all at
//! power-on: that the two give the same run is the contract of the machine the replay rests on.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
use boards::MachineOptions;
use cpu_rv32::asm::{addi, blt, bne, jal, lbu, lui, lw, sw};
use emu_core::Cycles;
use session::{
    ConsoleDigests, Cut, CutReason, End, Limits, Outcome, ProfileText, Quantity, Replay,
    ReplayError, RomIdentity, Session,
};
use soc_esp32c3::clock::TIME_BASE;
use soc_esp32c3::test_support::{
    HANG, T0, T1, T2, T3, TIMER_WAKE_STATE, flat_rom, load_register, sleep_on_the_rtc_timer,
    stop_watchdogs, store_register,
};
use soc_esp32c3::{AcceptedInput, ConsoleChannel, InputError, RomImage, StopReason, UsbHost};

/// `s1`, `a0`, `a1`: registers the helpers of `test_support` leave alone.
const S1: u8 = 9;
const A0: u8 = 10;
const A1: u8 = 11;

fn reg(peripheral: &str, register: &str) -> u32 {
    svd_regs::esp32c3::addr(peripheral, register)
}

/// The 4 KiB page `load_register` addresses `addr` through, and the offset of `addr` in it.
fn page_and_offset(addr: u32) -> (u32, i32) {
    let page = addr.wrapping_add(0x800) & 0xFFFF_F000;
    (page, addr.wrapping_sub(page).cast_signed())
}

/// A firmware of three lives. Each counts itself in `RTC_CNTL.STORE0`, prints the count on
/// UART0, then echoes on UART0 what UART0 and USB-Serial-JTAG receive, until UART0 receives a
/// carriage return. Life 1 then resets the core by software, life 2 sleeps 0x2000 slow-clock
/// counts (60 ms) on the RTC timer, life 3 sleeps with no wake source.
fn three_lives() -> Vec<u32> {
    let uart0 = reg("UART0", "FIFO");
    let store0 = reg("RTC_CNTL", "STORE0");
    let (ep1, ep1_conf) = (reg("USB_DEVICE", "EP1"), reg("USB_DEVICE", "EP1_CONF"));
    let usb = ep1 & 0xFFFF_F000;
    let offset = |addr: u32| (addr - usb).cast_signed();
    let mut program = stop_watchdogs();
    program.extend(load_register(store0));
    program.extend([
        addi(T2, T1, 1),
        sw(T2, T0, page_and_offset(store0).1),
        lui(T3, uart0),
        addi(T1, T2, i32::from(b'0')),
        sw(T1, T3, 0),
        lui(S1, usb),
        // `EP1_CONF` bit 2: the OUT FIFO holds a byte of the host; bits above it read 0.
        addi(A0, 0, 4),
        addi(A1, 0, i32::from(b'\r')),
        // Poll: a byte in the receive FIFO of UART0 (`STATUS`, its count in the low byte)?
        lbu(T1, T3, 0x1C),
        bne(T1, 0, 24),
        // A byte of the host on USB-Serial-JTAG? Echo it on UART0.
        lw(T1, S1, offset(ep1_conf)),
        blt(T1, A0, -12),
        lw(T1, S1, offset(ep1)),
        sw(T1, T3, 0),
        jal(0, -24),
        // The byte of UART0, echoed; a carriage return ends the life.
        lw(T1, T3, 0),
        sw(T1, T3, 0),
        bne(T1, A1, -36),
    ]);
    let mut reset = store_register(reg("RTC_CNTL", "OPTIONS0"), 1 << 31).to_vec();
    reset.push(HANG);
    let timed = sleep_on_the_rtc_timer(0x2000, TIMER_WAKE_STATE);
    for (life, then) in [(1, reset), (2, timed)] {
        let skip = i32::try_from(4 * (then.len() + 1)).unwrap();
        program.extend([addi(T1, 0, life), bne(T2, T1, skip)]);
        program.extend(then);
    }
    program.extend(sleep_on_the_rtc_timer(0x2000, 0));
    program
}

fn rom() -> RomImage {
    flat_rom(&three_lives())
}

fn ms(millis: u64) -> Cycles {
    Cycles(millis * TIME_BASE.hz() / 1_000)
}

/// More calls than any run here makes: a run that has not ended by then never will.
const MAX_CALLS: u64 = 10_000_000;

/// A recorded run: its session, what UART0 sent, and its resets.
struct Recorded {
    session: Session,
    uart0: Vec<u8>,
    resets: u64,
}

/// Records the run of [`three_lives`], cut by the host at `cut_nanos` of device time when it is
/// given, as Ctrl-C cuts a run of the command line at the end of one of its calls.
fn record(cut_nanos: Option<u64>) -> Recorded {
    let profile = boards::builtin("x4").unwrap();
    let rom = rom();
    let rom_identity = RomIdentity::of(3, &rom);
    let options = MachineOptions {
        strict_mmio: true,
        trace_hash: true,
        usb_host: UsbHost::Attached,
    };
    let mut m = profile.instantiate(rom, options);
    let start = m.flash_contents();
    // The steps before the first wait: a byte on USB-Serial-JTAG that waits for the host to
    // open the port, the port opened, a line typed on UART0.
    m.send_console(ConsoleChannel::UsbSerialJtag, ms(1), b"u")
        .unwrap();
    m.set_usb_host(ms(2), UsbHost::Open).unwrap();
    m.send_console(ConsoleChannel::Uart0, ms(4), b"a\r")
        .unwrap();
    m.watch_console(ConsoleChannel::Uart0, true);
    let mut consoles = ConsoleDigests::default();
    let mut uart0 = Vec::new();
    let mut stage = 0;
    let mut calls = 0;
    let end = loop {
        calls += 1;
        assert!(calls < MAX_CALLS, "the recorded run does not end");
        // The cut is a device-time limit of the recording: it can fall inside a deep sleep.
        let stop = m.run_within(m.cycles() + 10_000, cut_nanos);
        for channel in ConsoleChannel::ALL {
            let bytes = m.take_console(channel);
            consoles.add(channel, &bytes);
            if channel == ConsoleChannel::Uart0 {
                uart0.extend(bytes);
            }
        }
        let now = m.device_time();
        // Each wait of the script: after the second life starts, a line 1 ms later and the
        // cable pulled; after the third, a line and the port opened at once.
        if stage == 0 && uart0.contains(&b'2') {
            let later = now + ms(1).get();
            m.send_console(ConsoleChannel::Uart0, later, b"b\r")
                .unwrap();
            m.set_usb_host(later, UsbHost::Absent).unwrap();
            stage = 1;
        }
        if stage == 1 && uart0.contains(&b'3') {
            m.send_console(ConsoleChannel::Uart0, now, b"c\r").unwrap();
            m.set_usb_host(now, UsbHost::Open).unwrap();
            stage = 2;
        }
        match stop {
            StopReason::DeepSleep => break End::DeepSleep,
            StopReason::TimeBudget if cut_nanos.is_some() => break End::Cut,
            StopReason::CycleBudget | StopReason::Asleep | StopReason::ConsoleOutput => {}
            other => panic!("{other:?}"),
        }
    };
    for channel in ConsoleChannel::ALL {
        let bytes = m.flush_console(channel);
        consoles.add(channel, &bytes);
        if channel == ConsoleChannel::Uart0 {
            uart0.extend(bytes);
        }
    }
    let limits = Limits {
        strict_mmio: true,
        cut: (end == End::Cut).then(|| Cut {
            at: m.device_time(),
            reason: CutReason::Interrupted,
        }),
        ..Limits::default()
    };
    let outcome = Outcome::of(&m, end, consoles).unwrap();
    let profile = ProfileText {
        id: "x4".to_owned(),
        text: boards::builtin_toml("x4").unwrap().to_owned(),
    };
    let session = Session::record(
        rom_identity,
        profile,
        start,
        limits,
        m.input_journal(),
        outcome,
    )
    .unwrap();
    Recorded {
        session,
        uart0,
        resets: m.resets(),
    }
}

/// The outcome of a replay of `session` in steps of `slice` cycles.
fn replay(session: &Session, slice: u64) -> Outcome {
    let mut replay = Replay::start(session, rom()).unwrap();
    for _ in 0..MAX_CALLS {
        if replay.step(slice).end.is_some() {
            return replay.finish().unwrap();
        }
    }
    panic!("the replay in steps of {slice} cycles does not end");
}

#[test]
fn the_recorded_run_goes_through_a_reset_a_deep_sleep_and_changes_of_the_usb_host() {
    let recorded = record(None);
    let mut from = 0;
    for expected in *b"1ua2b3c" {
        let at = recorded.uart0[from..]
            .iter()
            .position(|&byte| byte == expected)
            .unwrap_or_else(|| {
                panic!(
                    "`{}` is missing, or out of order: {:?}",
                    char::from(expected),
                    String::from_utf8_lossy(&recorded.uart0)
                )
            });
        from += at + 1;
    }
    assert_eq!(recorded.resets, 2, "the software reset and the wake");
    let session = &recorded.session;
    assert_eq!(session.outcome.end, End::DeepSleep);
    assert_eq!(session.usb_host, UsbHost::Attached);
    let changes = session
        .inputs
        .iter()
        .filter(|input| matches!(input, AcceptedInput::UsbHost { .. }))
        .count();
    assert_eq!((session.inputs.len(), changes), (7, 3));
}

#[test]
fn a_session_replays_to_its_outcome_whatever_the_slices() {
    let session = record(None).session;
    for slice in [u64::MAX, 1, 7, 997, 7_919] {
        assert_eq!(
            replay(&session, slice),
            session.outcome,
            "slices of {slice}"
        );
    }
}

#[test]
fn a_session_read_back_from_its_bytes_replays_to_the_same_outcome_twice() {
    let session = record(None).session;
    let read = Session::from_bytes(&session.to_bytes().unwrap()).unwrap();
    assert_eq!(read, session);
    let first = replay(&read, 1_000_000);
    assert_eq!(first, session.outcome);
    assert_eq!(replay(&read, 1_000_000).trace_hash, first.trace_hash);
}

#[test]
fn a_cut_session_replays_to_its_cut() {
    // 3 ms: the first life polls its consoles, the line on UART0 not typed yet.
    let session = record(Some(3_000_000)).session;
    assert_eq!(session.outcome.end, End::Cut);
    for slice in [1_000_000, 997] {
        assert_eq!(
            replay(&session, slice),
            session.outcome,
            "slices of {slice}"
        );
    }
    // Cut during the deep sleep of the second life.
    let asleep = record(Some(30_000_000)).session;
    assert_eq!(asleep.outcome.end, End::Cut);
    assert_eq!(replay(&asleep, 7_919), asleep.outcome);
}

#[test]
fn a_changed_input_byte_diverges_and_names_the_first_quantity() {
    let mut session = record(None).session;
    let Some(AcceptedInput::Console { bytes, .. }) = session.inputs.first_mut() else {
        panic!("the first input is the byte on USB-Serial-JTAG")
    };
    bytes[0] = b'v';
    let replayed = replay(&session, 1_000_000);
    let differences = session.outcome.differences(&replayed);
    // The echo takes the same path whatever the byte: only what UART0 sent differs.
    assert_eq!(
        differences.iter().map(|d| d.quantity).collect::<Vec<_>>(),
        [Quantity::Console(ConsoleChannel::Uart0)]
    );
}

#[test]
fn the_wrong_rom_or_an_input_the_machine_refuses_is_refused_before_anything_runs() {
    let mut session = record(Some(1_000_000)).session;
    assert!(matches!(
        Replay::start(&session, flat_rom(&[HANG])),
        Err(ReplayError::Rom { revision: 3, .. })
    ));
    session.inputs.push(AcceptedInput::Console {
        channel: ConsoleChannel::Uart1,
        at: Cycles(u64::MAX),
        bytes: b"x".to_vec(),
    });
    let index = session.inputs.len() - 1;
    assert!(matches!(
        Replay::start(&session, rom()),
        Err(ReplayError::Input {
            index: i,
            source: InputError::BeyondTheLine { .. }
        }) if i == index
    ));
}

#[test]
fn a_replay_has_no_outcome_before_its_end_and_runs_nothing_after_it() {
    let session = record(Some(1_000_000)).session;
    let unfinished = Replay::start(&session, rom()).unwrap();
    assert!(matches!(unfinished.finish(), Err(ReplayError::Unfinished)));
    let mut replay = Replay::start(&session, rom()).unwrap();
    while replay.step(u64::MAX).end.is_none() {}
    let cycles = replay.machine().cycles();
    let after = replay.step(1_000);
    assert_eq!((after.taken.len(), after.end), (0, Some(End::Cut)));
    assert_eq!(replay.machine().cycles(), cycles);
}
