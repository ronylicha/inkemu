//! A session recorded from a run that goes through a software reset, a deep sleep, inputs on two
//! consoles and changes of the USB host, then replayed (G7): the same outcome, to the trace hash,
//! whatever the slices of the replay, read back from its bytes or not; a cut run replays to its
//! cut, wherever the host cut it; a changed input diverges.
//!
//! The recording hands its inputs as a console script does: those before the first wait at
//! power-on, the others when the console shows what they wait for, some of them while the chip
//! sleeps deeply, some at the same instant on one channel, some to a UART whose baud rate changes
//! before they reach it. The replay hands them all at power-on: that the two give the same run is
//! the contract of the machine the replay rests on (R1).
//!
//! A run the host cuts (Ctrl-C, a reader gone, a console that cannot be written) is cut between
//! two calls of the machine, after a call that returned for a reason of its own, as the command
//! line cuts it: the recording knows no deadline, and its session holds the device time it
//! stopped at. That the replay, given that instant as a device-time limit, stops at the same
//! cycles is what a session rests on (R4); it is checked here after every kind of stop.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
use boards::{MachineOptions, SdHandle, SdStart, Wired, WiredOptions};
use chips::sdcard::{Overlay, SdCard, SdCardConfig, SparseBlocks};
use cpu_rv32::asm::{addi, blt, bne, jal, lbu, lui, lw, mul, sw, wfi};
use emu_core::{BoardInput, Cycles};
use session::{
    ConsoleDigests, Cut, CutReason, End, Limits, Outcome, ProfileText, Quantity, Replay,
    ReplayError, RomIdentity, SdCardStart, Session, changes_sha256,
};
use soc_esp32c3::clock::TIME_BASE;
use soc_esp32c3::test_support::{
    HANG, T0, T1, T2, T3, TIMER_WAKE_STATE, flat_rom, load_register, sleep_on_the_rtc_timer,
    stop_watchdogs, store_register,
};
use soc_esp32c3::{
    AcceptedInput, ConsoleChannel, Esp32C3, InputError, RomImage, StopReason, UsbHost,
};

/// `s1`, `a0`, `a1`: registers the helpers of `test_support` leave alone.
const S1: u8 = 9;
const A0: u8 = 10;
const A1: u8 = 11;

/// `UART0.CLKDIV` for about twice the baud rate of reset: 347 and 3/16, where reset gives 694
/// (`0x2B6`).
const CLKDIV_TWICE_AS_FAST: u32 = 0x0030_015B;

fn reg(peripheral: &str, register: &str) -> u32 {
    svd_regs::esp32c3::addr(peripheral, register)
}

/// The 4 KiB page `load_register` addresses `addr` through, and the offset of `addr` in it.
fn page_and_offset(addr: u32) -> (u32, i32) {
    let page = addr.wrapping_add(0x800) & 0xFFFF_F000;
    (page, addr.wrapping_sub(page).cast_signed())
}

/// The instructions that skip `then` unless `T2` holds `life`.
fn in_life(life: i32, then: &[u32]) -> Vec<u32> {
    let skip = i32::try_from(4 * (then.len() + 1)).unwrap();
    let mut program = vec![addi(T1, 0, life), bne(T2, T1, skip)];
    program.extend(then);
    program
}

/// A firmware of three lives. Each counts itself in `RTC_CNTL.STORE0`, prints the count on
/// UART0, then echoes on UART0 what UART0 and USB-Serial-JTAG receive, until UART0 receives a
/// carriage return. Life 2 first doubles the baud rate of UART0. Life 1 then resets the core by
/// software, life 2 sleeps 0x2000 slow-clock counts (60 ms) on the RTC timer, life 3 sleeps with
/// no wake source.
fn three_lives() -> Vec<u32> {
    let uart0 = reg("UART0", "FIFO");
    let store0 = reg("RTC_CNTL", "STORE0");
    let (ep1, ep1_conf) = (reg("USB_DEVICE", "EP1"), reg("USB_DEVICE", "EP1_CONF"));
    let usb = ep1 & 0xFFFF_F000;
    let offset = |addr: u32| (addr - usb).cast_signed();
    let mut program = stop_watchdogs();
    program.extend(load_register(store0));
    program.extend([addi(T2, T1, 1), sw(T2, T0, page_and_offset(store0).1)]);
    program.extend(in_life(
        2,
        &store_register(reg("UART0", "CLKDIV"), CLKDIV_TWICE_AS_FAST),
    ));
    program.extend([
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
    program.extend(in_life(1, &reset));
    program.extend(in_life(
        2,
        &sleep_on_the_rtc_timer(0x2000, TIMER_WAKE_STATE),
    ));
    program.extend(sleep_on_the_rtc_timer(0x2000, 0));
    program
}

/// A firmware that prints `x` on UART0, then waits for interrupts that never come: the CPU
/// sleeps through the bytes the host sends, which no interrupt reports, and is idle once they
/// have all arrived.
fn waiting() -> Vec<u32> {
    let mut program = stop_watchdogs();
    program.extend([
        lui(T3, reg("UART0", "FIFO")),
        addi(T1, 0, i32::from(b'x')),
        sw(T1, T3, 0),
        wfi(),
        jal(0, -4),
    ]);
    program
}

/// The X4's latch (GPIO13) held high, then the power key (GPIO3, pulled up) awaited released,
/// then pressed: the latch is then cut, and the X4 goes off when the key is let go.
fn power_off_on_the_key() -> Vec<u32> {
    const GPIO_FUNCTION: u32 = 1 << 12;
    const FUN_WPU: u32 = 1 << 8;
    const FUN_IE: u32 = 1 << 9;
    let mut program = stop_watchdogs();
    program.extend(store_register(reg("IO_MUX", "GPIO13"), GPIO_FUNCTION));
    program.extend(store_register(reg("GPIO", "OUT_W1TS"), 1 << 13));
    program.extend(store_register(reg("GPIO", "ENABLE_W1TS"), 1 << 13));
    program.extend(store_register(
        reg("IO_MUX", "GPIO3"),
        GPIO_FUNCTION | FUN_IE | FUN_WPU,
    ));
    // Bit 3 of `GPIO_IN` times 2^28 is the sign bit: negative while the key is released.
    program.push(lui(T3, 1 << 28));
    let gpio_in = load_register(reg("GPIO", "IN"));
    program.extend(gpio_in);
    program.extend([mul(T1, T1, T3), blt(T1, 0, 8), jal(0, -16)]);
    program.extend(gpio_in);
    program.extend([mul(T1, T1, T3), blt(T1, 0, -12)]);
    program.extend(store_register(reg("GPIO", "OUT_W1TC"), 1 << 13));
    program.push(HANG);
    program
}

const fn ms(millis: u64) -> Cycles {
    Cycles(millis * TIME_BASE.hz() / 1_000)
}

fn nanos(instant: Cycles) -> u64 {
    TIME_BASE.cycles_to_nanos(instant)
}

/// More calls than any run here makes: a run that has not ended by then never will.
const MAX_CALLS: u64 = 10_000_000;
/// The CPU cycles one call of a recording runs at most.
const CALL_CYCLES: u64 = 10_000;

/// Where the host cuts a recorded run.
#[derive(Clone, Copy, Debug)]
enum Cutting {
    /// Nowhere: the run goes to an end of its own.
    Not,
    /// At this many nanoseconds of device time, a device-time limit of every call.
    AtDeadline(u64),
    /// After call `n` (from 0) returned, whatever it returned, as the command line cuts a run:
    /// Ctrl-C between two calls, or a console that fails in the last flush of a run that ended.
    AfterCall(usize),
}

/// A call of a recorded run: why it returned, and the state it left.
#[derive(Clone, Debug)]
struct Call {
    stop: StopReason,
    device_time: Cycles,
    /// The CPU waits for an interrupt.
    asleep: bool,
    /// The chip sleeps deeply.
    deep_sleep: bool,
}

/// A recorded run: its session, what UART0 sent, its resets, and each of its calls.
struct Recorded {
    session: Session,
    uart0: Vec<u8>,
    resets: u64,
    calls: Vec<Call>,
}

/// The firmware a run starts, and the inputs its host gives it.
struct Firmware {
    program: fn() -> Vec<u32>,
    /// The inputs handed at power-on.
    at_power_on: fn(&mut Esp32C3),
    /// The inputs handed after a call, given what UART0 sent so far and the stage the host is
    /// at: the waits of a console script. Returns the next stage.
    after_call: fn(&mut Esp32C3, &[u8], u32) -> u32,
    /// The device time a call runs at most: the host looks at the machine that often.
    pace: Option<Cycles>,
    /// How long the X4's power key is held at power-on, in milliseconds (0: not pressed).
    power_hold_ms: u32,
    /// An SD card in the slot, holding a boot sector.
    card: bool,
}

const THREE_LIVES: Firmware = Firmware {
    program: three_lives,
    at_power_on: |m| {
        // The steps before the first wait: a byte and a packet of no byte at the same instant on
        // USB-Serial-JTAG, which wait for the host to open the port, the port opened, a line
        // typed on UART0 in two pieces at the same instant.
        m.send_console(ConsoleChannel::UsbSerialJtag, ms(1), b"u")
            .unwrap();
        m.send_console(ConsoleChannel::UsbSerialJtag, ms(1), b"")
            .unwrap();
        m.set_usb_host(ms(2), UsbHost::Open).unwrap();
        m.send_console(ConsoleChannel::Uart0, ms(4), b"a").unwrap();
        m.send_console(ConsoleChannel::Uart0, ms(4), b"\r").unwrap();
    },
    after_call: |m, uart0, stage| {
        let now = m.device_time();
        match stage {
            // Life 2 has started, and doubled the baud rate of UART0: a line 1 ms later, which
            // the UART takes at the new rate, and the cable pulled.
            0 if uart0.contains(&b'2') => {
                let later = now + ms(1).get();
                m.send_console(ConsoleChannel::Uart0, later, b"b\r")
                    .unwrap();
                m.set_usb_host(later, UsbHost::Absent).unwrap();
                1
            }
            // The chip sleeps deeply: on each console a byte dated inside the sleep, and one
            // dated from the wake on, which life 3 echoes; the cable plugged in and the port
            // opened at the same instant inside the sleep.
            1 => {
                let Some(sleep) = m.deep_sleep() else {
                    return stage;
                };
                let wake = sleep
                    .timer_wake_nanos
                    .expect("the RTC timer wakes the chip");
                let after_the_wake =
                    |millis| TIME_BASE.nanos_to_cycles(u128::from(wake)) + ms(millis).get();
                let inside = now + ms(10).get();
                m.send_console(ConsoleChannel::Uart0, now + ms(5).get(), b"z")
                    .unwrap();
                m.send_console(ConsoleChannel::Uart0, after_the_wake(1), b"y")
                    .unwrap();
                m.send_console(ConsoleChannel::UsbSerialJtag, now, b"w")
                    .unwrap();
                m.send_console(ConsoleChannel::UsbSerialJtag, after_the_wake(0), b"v")
                    .unwrap();
                m.set_usb_host(inside, UsbHost::Attached).unwrap();
                m.set_usb_host(inside, UsbHost::Open).unwrap();
                2
            }
            // Life 3 has started: a line and the port opened again, at once.
            2 if uart0.contains(&b'3') => {
                m.send_console(ConsoleChannel::Uart0, now, b"c\r").unwrap();
                m.set_usb_host(now, UsbHost::Open).unwrap();
                3
            }
            _ => stage,
        }
    },
    pace: Some(ms(1)),
    power_hold_ms: 0,
    card: false,
};

const WAITING: Firmware = Firmware {
    program: waiting,
    // Two bytes far apart: the CPU sleeps more than a second before each.
    at_power_on: |m| {
        m.send_console(ConsoleChannel::Uart0, ms(1_500), b"1")
            .unwrap();
        m.send_console(ConsoleChannel::Uart0, ms(3_000), b"2")
            .unwrap();
    },
    after_call: |_, _, stage| stage,
    pace: None,
    power_hold_ms: 0,
    card: false,
};

/// The power key: the gesture holds it 5 ms; the host presses it again at 20 ms and lets it go at
/// 30 ms, the firmware having cut the latch in between. A card sits in the slot.
const POWER: Firmware = Firmware {
    program: power_off_on_the_key,
    at_power_on: |m| {
        let key = |value| BoardInput { target: 0, value };
        m.send_board_input(ms(20), key(1)).unwrap();
        m.send_board_input(ms(30), key(0)).unwrap();
    },
    after_call: |_, _, stage| stage,
    pace: None,
    power_hold_ms: 5,
    card: true,
};

fn rom(firmware: &Firmware) -> RomImage {
    flat_rom(&(firmware.program)())
}

/// The wired X4 `firmware` runs on, with its card when it has one, as the command line powers it
/// on: the machine, the slot, and the card as the session records it.
fn power_on(
    firmware: &Firmware,
    rom: RomImage,
) -> (Esp32C3, Option<SdHandle>, Option<SdCardStart>) {
    let options = MachineOptions {
        strict_mmio: true,
        trace_hash: true,
        usb_host: UsbHost::Attached,
    };
    let card = firmware.card.then(|| {
        let mut blocks = SparseBlocks::new(2048);
        blocks.insert(0, [0xEB; 512]).unwrap();
        SdCardStart::read(&mut blocks, false).unwrap()
    });
    let wired = WiredOptions {
        power_hold_ms: Some(firmware.power_hold_ms),
        usb_power: false,
        sd: card.as_ref().map(|card| SdStart {
            source: Box::new(card.source()),
            config: SdCardConfig::default(),
        }),
    };
    let profile = boards::builtin("x4").unwrap();
    let Wired { machine, sd, .. } = Wired::power_on(&profile, rom, None, options, wired).unwrap();
    (machine, sd, card)
}

/// Records the run of `firmware`, cut by the host as `cutting` says.
fn record(firmware: &Firmware, cutting: Cutting) -> Recorded {
    let rom = rom(firmware);
    let rom_identity = RomIdentity::of(3, &rom);
    let (mut m, slot, card) = power_on(firmware, rom);
    let start = m.flash_contents();
    (firmware.at_power_on)(&mut m);
    m.watch_console(ConsoleChannel::Uart0, true);
    let mut consoles = ConsoleDigests::default();
    let mut uart0 = Vec::new();
    let mut calls = Vec::new();
    let mut stage = 0;
    let end = loop {
        assert!(
            (calls.len() as u64) < MAX_CALLS,
            "the recorded run does not end"
        );
        let paced = firmware
            .pace
            .map(|pace| nanos(m.device_time() + pace.get()));
        let deadline = match (cutting, paced) {
            (Cutting::AtDeadline(cut), Some(paced)) => Some(cut.min(paced)),
            (Cutting::AtDeadline(cut), None) => Some(cut),
            (_, paced) => paced,
        };
        let stop = m.run_within(m.cycles() + CALL_CYCLES, deadline);
        for channel in ConsoleChannel::ALL {
            let bytes = m.take_console(channel);
            consoles.add(channel, &bytes);
            if channel == ConsoleChannel::Uart0 {
                uart0.extend(bytes);
            }
        }
        calls.push(Call {
            stop: stop.clone(),
            device_time: m.device_time(),
            asleep: m.is_asleep(),
            deep_sleep: m.deep_sleep().is_some(),
        });
        stage = (firmware.after_call)(&mut m, &uart0, stage);
        let own_end = match stop {
            StopReason::DeepSleep => Some(End::DeepSleep),
            StopReason::PowerOff => Some(End::PowerOff),
            StopReason::Idle => Some(End::Idle),
            StopReason::TimeBudget if matches!(cutting, Cutting::AtDeadline(cut) if nanos(m.device_time()) >= cut) => {
                Some(End::Cut)
            }
            StopReason::CycleBudget
            | StopReason::Asleep
            | StopReason::ConsoleOutput
            | StopReason::TimeBudget => None,
            StopReason::StrictMmio(violation) => panic!("{violation}"),
        };
        // The host cuts the run after the call: between two calls, or in the last flush of a
        // run that ended.
        if matches!(cutting, Cutting::AfterCall(n) if n + 1 == calls.len()) {
            break End::Cut;
        }
        if let Some(end) = own_end {
            break end;
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
        // The command line records a cut at the device time the machine stopped at.
        cut: (end == End::Cut).then(|| Cut {
            at: m.device_time(),
            reason: CutReason::Interrupted,
        }),
        ..Limits::default()
    };
    let storage = slot
        .and_then(|slot| slot.take_card())
        .map(SdCard::into_storage);
    let changes = storage.as_ref().map(Overlay::changes);
    let outcome = Outcome::of(&m, end, consoles, changes.as_ref()).unwrap();
    let profile = ProfileText {
        id: "x4".to_owned(),
        text: boards::builtin_toml("x4").unwrap().to_owned(),
    };
    let session = Session::record(
        rom_identity,
        profile,
        start,
        card,
        limits,
        m.input_journal(),
        outcome,
    )
    .unwrap();
    Recorded {
        session,
        uart0,
        resets: m.resets(),
        calls,
    }
}

/// The outcome of a replay of `session` of `firmware` in steps of `slice` cycles.
fn replay(firmware: &Firmware, session: &Session, slice: u64) -> Outcome {
    let mut replay = Replay::start(session, rom(firmware)).unwrap();
    for _ in 0..MAX_CALLS {
        if replay.step(slice).end.is_some() {
            return replay.finish().unwrap();
        }
    }
    panic!("the replay in steps of {slice} cycles does not end");
}

/// Records `firmware` cut after each call `calls` names, and replays each cut session in steps
/// of each of `slices` cycles: the outcome recorded, every time.
fn cuts_replay_to_their_cut(firmware: &Firmware, calls: &[usize], slices: &[u64]) {
    for &call in calls {
        let recorded = record(firmware, Cutting::AfterCall(call));
        let session = &recorded.session;
        assert_eq!(session.outcome.end, End::Cut, "after call {call}");
        assert_eq!(recorded.calls.len(), call + 1);
        for &slice in slices {
            assert_eq!(
                replay(firmware, session, slice),
                session.outcome,
                "cut after call {call}, {:?}, replayed in slices of {slice}",
                recorded.calls[call]
            );
        }
    }
}

/// The first call of `calls` that `matches` takes.
fn first(calls: &[Call], matches: impl Fn(&Call) -> bool) -> usize {
    calls
        .iter()
        .position(matches)
        .expect("the run has such a call")
}

#[test]
fn the_recorded_run_goes_through_a_reset_a_deep_sleep_and_changes_of_the_usb_host() {
    let recorded = record(&THREE_LIVES, Cutting::Not);
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
    // What the host gave during the deep sleep, dated from the wake on, reached the third life.
    for given in *b"yv" {
        assert!(
            recorded.uart0.contains(&given),
            "`{}` is missing: {:?}",
            char::from(given),
            String::from_utf8_lossy(&recorded.uart0)
        );
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
    assert_eq!((session.inputs.len(), changes), (15, 5));
    assert!(
        recorded.calls.iter().any(|call| call.deep_sleep),
        "a call returned inside the deep sleep"
    );
}

#[test]
fn a_session_replays_to_its_outcome_whatever_the_slices() {
    let session = record(&THREE_LIVES, Cutting::Not).session;
    for slice in [u64::MAX, 1, 7, 997, 7_919] {
        assert_eq!(
            replay(&THREE_LIVES, &session, slice),
            session.outcome,
            "slices of {slice}"
        );
    }
}

/// ADR 0013 and the design of sub-project 2e, §7: the gesture and the key of the board, the
/// supply cut by the latch and the card of the slot replay to the same power-off, whatever the
/// slices, from the session or from its bytes.
#[test]
fn a_wired_session_with_board_inputs_and_a_card_replays_to_its_power_off() {
    let session = record(&POWER, Cutting::Not).session;
    assert_eq!(session.outcome.end, End::PowerOff);
    let board: Vec<(Cycles, BoardInput)> = session
        .inputs
        .iter()
        .filter_map(|input| match *input {
            AcceptedInput::Board { at, input } => Some((at, input)),
            _ => None,
        })
        .collect();
    let key = |value| BoardInput { target: 0, value };
    assert_eq!(
        board,
        [
            (Cycles(0), key(1)),
            (ms(5), key(0)),
            (ms(20), key(1)),
            (ms(30), key(0))
        ],
        "the gesture, then the host"
    );
    // The release acts one cycle of the time base after its date, the instruction under way
    // ended: the run stops at the end of that instruction.
    let off = session.outcome.device_time;
    assert!(
        ms(30) < off && off <= ms(30) + 16,
        "off once the key is let go: {off:?}"
    );
    let card = session.sd.as_ref().unwrap();
    assert_eq!((card.blocks(), card.ranges().len()), (2048, 1));
    let nothing_changed = changes_sha256(&Overlay::new(SparseBlocks::new(1)).changes());
    assert_eq!(session.outcome.sd_changes, Some(nothing_changed));
    let read = Session::from_bytes(&session.to_bytes().unwrap()).unwrap();
    assert_eq!(read, session);
    for slice in [u64::MAX, 1, 7, 997, 7_919] {
        assert_eq!(
            replay(&POWER, &read, slice),
            session.outcome,
            "slices of {slice}"
        );
    }
}

#[test]
fn a_session_read_back_from_its_bytes_replays_to_the_same_outcome_twice() {
    let session = record(&THREE_LIVES, Cutting::Not).session;
    let read = Session::from_bytes(&session.to_bytes().unwrap()).unwrap();
    assert_eq!(read, session);
    let first = replay(&THREE_LIVES, &read, 1_000_000);
    assert_eq!(first, session.outcome);
    assert_eq!(
        replay(&THREE_LIVES, &read, 1_000_000).trace_hash,
        first.trace_hash
    );
}

#[test]
fn a_session_cut_at_a_deadline_of_its_recording_replays_to_its_cut() {
    // 3 ms: the first life polls its consoles, the line on UART0 not typed yet.
    let session = record(&THREE_LIVES, Cutting::AtDeadline(3_000_000)).session;
    assert_eq!(session.outcome.end, End::Cut);
    for slice in [1_000_000, 997] {
        assert_eq!(
            replay(&THREE_LIVES, &session, slice),
            session.outcome,
            "slices of {slice}"
        );
    }
    // Cut during the deep sleep of the second life.
    let asleep = record(&THREE_LIVES, Cutting::AtDeadline(30_000_000)).session;
    assert_eq!(asleep.outcome.end, End::Cut);
    assert_eq!(replay(&THREE_LIVES, &asleep, 7_919), asleep.outcome);
}

#[test]
fn a_run_cut_between_two_calls_replays_to_its_cut_after_every_kind_of_stop() {
    let calls = record(&THREE_LIVES, Cutting::Not).calls;
    let mut cut_after = vec![
        // The cycle budget of a call, while the first life polls.
        first(&calls, |call| {
            call.stop == StopReason::CycleBudget && call.device_time > ms(2)
        }),
        // A watched byte of UART0 handed to the host.
        first(&calls, |call| call.stop == StopReason::ConsoleOutput),
        // Inside the deep sleep of the second life.
        first(&calls, |call| call.deep_sleep),
        // The end of the run: a console that fails in the last flush.
        calls.len() - 1,
    ];
    // The first call after the wake from the deep sleep, the inputs given during it pending.
    let slept = first(&calls, |call| call.deep_sleep);
    cut_after.push(slept + first(&calls[slept..], |call| !call.deep_sleep));
    assert_eq!(calls[calls.len() - 1].stop, StopReason::DeepSleep);
    cuts_replay_to_their_cut(&THREE_LIVES, &cut_after, &[u64::MAX, 997]);
}

#[test]
fn a_run_cut_while_the_cpu_waits_for_an_interrupt_replays_to_its_cut() {
    let recorded = record(&WAITING, Cutting::Not);
    let calls = &recorded.calls;
    assert_eq!(recorded.session.outcome.end, End::Idle);
    let output = first(calls, |call| call.stop == StopReason::ConsoleOutput);
    let asleep = first(calls, |call| call.stop == StopReason::Asleep && call.asleep);
    assert!(calls[asleep].device_time > ms(1_000));
    let cut_after = [
        output,
        asleep,
        // The second sleep of more than a second.
        asleep + 1 + first(&calls[asleep + 1..], |call| call.stop == StopReason::Asleep),
        // The CPU is idle: a console that fails in the last flush.
        calls.len() - 1,
    ];
    assert_eq!(calls[calls.len() - 1].stop, StopReason::Idle);
    cuts_replay_to_their_cut(&WAITING, &cut_after, &[u64::MAX, 1, 997]);
    // Uncut, it replays to its end.
    assert_eq!(
        replay(&WAITING, &recorded.session, 997),
        recorded.session.outcome
    );
}

#[test]
fn a_changed_input_byte_diverges_and_names_the_first_quantity() {
    let mut session = record(&THREE_LIVES, Cutting::Not).session;
    let Some(AcceptedInput::Console { bytes, .. }) = session.inputs.first_mut() else {
        panic!("the first input is the byte on USB-Serial-JTAG")
    };
    bytes[0] = b'v';
    let replayed = replay(&THREE_LIVES, &session, 1_000_000);
    let differences = session.outcome.differences(&replayed);
    // The echo takes the same path whatever the byte: only what UART0 sent differs.
    assert_eq!(
        differences.iter().map(|d| d.quantity).collect::<Vec<_>>(),
        [Quantity::Console(ConsoleChannel::Uart0)]
    );
}

#[test]
fn the_wrong_rom_or_an_input_the_machine_refuses_is_refused_before_anything_runs() {
    let mut session = record(&THREE_LIVES, Cutting::AtDeadline(1_000_000)).session;
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
        Replay::start(&session, rom(&THREE_LIVES)),
        Err(ReplayError::Input {
            index: i,
            source: InputError::BeyondTheLine { .. }
        }) if i == index
    ));
}

#[test]
fn a_replay_has_no_outcome_before_its_end_runs_nothing_after_it_and_keeps_no_journal() {
    let session = record(&THREE_LIVES, Cutting::AtDeadline(1_000_000)).session;
    let unfinished = Replay::start(&session, rom(&THREE_LIVES)).unwrap();
    assert!(matches!(unfinished.finish(), Err(ReplayError::Unfinished)));
    let mut replay = Replay::start(&session, rom(&THREE_LIVES)).unwrap();
    // The session holds the inputs; the machine does not hold them a second time.
    assert!(replay.machine().input_journal().is_forgone());
    assert_eq!(replay.machine().input_journal().inputs(), []);
    while replay.step(u64::MAX).end.is_none() {}
    let cycles = replay.machine().cycles();
    let after = replay.step(1_000);
    assert_eq!((after.taken.len(), after.end), (0, Some(End::Cut)));
    assert_eq!(replay.machine().cycles(), cycles);
}
