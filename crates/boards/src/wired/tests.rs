//! The wired X4 run by hand-written programs: its display hears its reset and drives BUSY, its
//! ladders read the codes `FreeInk` decodes as their keys, its power-on gesture is journaled and its
//! supply holds as the board's lines say.
use super::*;
use crate::builtin;
use chips::sdcard::SparseBlocks;
use cpu_rv32::asm;
use soc_esp32c3::test_support::{
    HANG, T1, flat_rom, load_register, stop_watchdogs, store_register,
};
use soc_esp32c3::{AcceptedInput, StopReason};

const A0: u8 = 10;
const A1: u8 = 11;
const MILLISECOND: u64 = 1_000_000;

fn x4() -> DeviceProfile {
    builtin("x4").unwrap()
}

fn power_on(profile: &DeviceProfile, program: &[u32], wired: WiredOptions) -> Wired {
    Wired::power_on(
        profile,
        flat_rom(program),
        None,
        MachineOptions::default(),
        wired,
    )
    .unwrap()
}

/// Runs until device time reaches `nanos`.
fn run_to(machine: &mut Esp32C3, nanos: u64) -> StopReason {
    machine.run_within(Cycles(u64::MAX), Some(nanos))
}

fn cycles(nanos: u64) -> Cycles {
    TIME_BASE.nanos_to_cycles(u128::from(nanos))
}

fn no_hold() -> WiredOptions {
    WiredOptions {
        power_hold_ms: Some(0),
        ..WiredOptions::default()
    }
}

fn register(peripheral: &str, name: &str) -> u32 {
    svd_regs::esp32c3::addr(peripheral, name)
}

/// `IO_MUX` `MCU_SEL` 1 (GPIO) and `FUN_IE`.
const GPIO_FUNCTION: u32 = 1 << 12;
const FUN_IE: u32 = 1 << 9;

/// Asserts the display's RST (GPIO5 enabled as an output, `OUT` low since reset), releases it,
/// then copies `GPIO_IN` into `a0` for ever.
fn reset_the_display_and_watch_busy() -> Vec<u32> {
    let mut program = stop_watchdogs();
    program.extend(store_register(register("IO_MUX", "GPIO5"), GPIO_FUNCTION));
    program.extend(store_register(
        register("IO_MUX", "GPIO6"),
        GPIO_FUNCTION | FUN_IE,
    ));
    program.extend(store_register(register("GPIO", "ENABLE_W1TS"), 1 << 5));
    program.extend(store_register(register("GPIO", "OUT_W1TS"), 1 << 5));
    let top = program.len();
    program.extend(load_register(register("GPIO", "IN")));
    program.push(asm::addi(A0, T1, 0));
    let back = i32::try_from((program.len() - top) * 4).unwrap();
    program.push(asm::jal(0, -back));
    program
}

#[test]
fn the_x4_display_hears_its_reset_and_drives_busy_for_the_hardware_reset_time() {
    let mut wired = power_on(&x4(), &reset_the_display_and_watch_busy(), no_hold());
    run_to(&mut wired.machine, 100_000);
    assert_eq!(
        wired.machine.x()[usize::from(A0)] >> 6 & 1,
        1,
        "busy after the reset"
    );
    // SP2a's hardware reset lasts 1 ms; the clocked chip lowers BUSY on its own.
    run_to(&mut wired.machine, 3 * MILLISECOND);
    assert_eq!(wired.machine.x()[usize::from(A0)] >> 6 & 1, 0, "ready");
    let panel = wired.panel.unwrap();
    assert_eq!(panel.rotation(), Rotation::Deg90);
    assert_eq!(panel.lines_mut().take_faults(), []);
    assert_eq!(
        panel.lines().chip().panel_image().generation,
        0,
        "no refresh yet"
    );
}

/// One-shot conversions of ADC1 at 12 dB on GPIO1 into `a0` and on GPIO2 into `a1`, for ever, as
/// ESP-IDF's `adc_oneshot` makes them: start low, start high, wait for `ADC1_DONE`, read, clear.
fn read_the_ladders() -> Vec<u32> {
    const ADC1_ONETIME: u32 = 1 << 31;
    const ONETIME_START: u32 = 1 << 29;
    const ATTEN_12DB: u32 = 3 << 23;
    const ADC1_DONE: u32 = 1 << 31;
    let sar = |name| register("APB_SARADC", name);
    let mut program = stop_watchdogs();
    // As `adc_oneshot_hal_setup` leaves it: the APB clock divided by 16, SAR_CLK_DIV 1.
    program.extend(store_register(sar("CLKM_CONF"), 15 | (2 << 21)));
    program.extend(store_register(sar("CTRL"), 0x4000_0040 | (1 << 7)));
    let top = program.len();
    for (channel, rd) in [(1, A0), (2, A1)] {
        let sample = ADC1_ONETIME | (channel << 25) | ATTEN_12DB;
        program.extend(store_register(sar("ONETIME_SAMPLE"), sample));
        program.extend(store_register(
            sar("ONETIME_SAMPLE"),
            sample | ONETIME_START,
        ));
        program.extend(load_register(sar("INT_RAW")));
        program.push(asm::blt(T1, 0, 8)); // ADC1_DONE is bit 31: the word is negative
        program.push(asm::jal(0, -12));
        program.extend(load_register(sar("SAR1DATA_STATUS")));
        program.push(asm::addi(rd, T1, 0));
        program.extend(store_register(sar("INT_CLR"), ADC1_DONE));
    }
    let back = i32::try_from((program.len() - top) * 4).unwrap();
    program.push(asm::jal(0, -back));
    program
}

/// `FreeInk`'s decoder (`InputManager.cpp:136-144`): the key `i` whose range holds the code.
fn decode(code: i32, ranges: &[i32]) -> Option<usize> {
    (0..ranges.len() - 1).find(|&i| ranges[i + 1] < code && code <= ranges[i])
}

/// `ADC_RANGES_1` and `ADC_RANGES_2` (`InputManager.cpp:43-44`), `ADC_NO_BUTTON` 3 900 on top.
const RANGES_1: [i32; 5] = [3900, 3100, 2090, 750, i32::MIN];
const RANGES_2: [i32; 3] = [3900, 1120, i32::MIN];

/// The codes three devices gave (`InputManager.cpp:29-33`): the least and the most of each key.
const KEYS: [(&str, bool, usize, i32, i32); 6] = [
    ("back", true, 0, 3470, 3597),
    ("confirm", true, 1, 2655, 2760),
    ("left", true, 2, 1470, 1530),
    ("right", true, 3, 3, 6),
    ("up", false, 0, 2205, 2300),
    ("down", false, 1, 3, 6),
];

fn readings(machine: &Esp32C3) -> (i32, i32) {
    let x = machine.x();
    let code = |reg: u8| i32::try_from(x[usize::from(reg)] & 0xFFF).unwrap();
    (code(A0), code(A1))
}

/// 2d-D7 and task 10: the levels of `x4.toml` come back, through the `SoC`'s ADC and ESP-IDF's
/// calibration of the X4's eFuse, as codes within the spread of the devices `FreeInk` measured.
#[test]
fn each_key_of_the_x4_ladders_reads_a_code_freeink_decodes_as_that_key() {
    let mut wired = power_on(&x4(), &read_the_ladders(), no_hold());
    let buttons = wired.buttons.clone().unwrap();
    run_to(&mut wired.machine, MILLISECOND);
    let (one, two) = readings(&wired.machine);
    assert!(one > 3900 && two > 3900, "idle reads no key: {one}, {two}");
    assert_eq!(decode(one, &RANGES_1), None);
    for (step, (id, first_ladder, key, least, most)) in (1u64..).zip(KEYS) {
        // Both dated before the run: a run ends at the first instruction past its limit.
        let (pressed_at, released_at) = (step * 2 * MILLISECOND, (step * 2 + 1) * MILLISECOND);
        let press = buttons.button_input(id, true).unwrap();
        wired
            .machine
            .send_board_input(cycles(pressed_at), press)
            .unwrap();
        let release = buttons.button_input(id, false).unwrap();
        wired
            .machine
            .send_board_input(cycles(released_at), release)
            .unwrap();
        run_to(&mut wired.machine, released_at - 1_000);
        let (one, two) = readings(&wired.machine);
        let (code, other, ranges) = if first_ladder {
            (one, two, &RANGES_1[..])
        } else {
            (two, one, &RANGES_2[..])
        };
        assert!((least..=most).contains(&code), "{id} reads {code}");
        assert_eq!(decode(code, ranges), Some(key), "{id} reads {code}");
        assert!(
            other > 3900,
            "the other ladder stays idle while {id} is held: {other}"
        );
        run_to(&mut wired.machine, released_at + MILLISECOND / 2);
        let (one, two) = readings(&wired.machine);
        assert!(one > 3900 && two > 3900, "{id} let go: {one}, {two}");
    }
}

fn board_inputs(machine: &Esp32C3) -> Vec<(Cycles, BoardInput)> {
    machine
        .input_journal()
        .inputs()
        .iter()
        .filter_map(|input| match *input {
            AcceptedInput::Board { at, input } => Some((at, input)),
            _ => None,
        })
        .collect()
}

#[test]
fn powering_the_x4_on_journals_the_press_and_release_of_its_power_key_and_the_cable() {
    let press = |value| BoardInput { target: 0, value };
    let cable = BoardInput {
        target: 7,
        value: 1,
    };
    let wired = power_on(&x4(), &[HANG], WiredOptions::default());
    assert_eq!(
        board_inputs(&wired.machine),
        [
            (Cycles::ZERO, press(1)),
            (cycles(1_000 * MILLISECOND), press(0))
        ],
        "the profile's hold"
    );
    let options = WiredOptions {
        power_hold_ms: Some(20),
        usb_power: true,
        sd: None,
    };
    let wired = power_on(&x4(), &[HANG], options);
    assert_eq!(
        board_inputs(&wired.machine),
        [
            (Cycles::ZERO, press(1)),
            (Cycles::ZERO, cable),
            (cycles(20 * MILLISECOND), press(0))
        ]
    );
    let wired = power_on(&x4(), &[HANG], no_hold());
    assert_eq!(board_inputs(&wired.machine), []);
}

#[test]
fn the_buttons_handle_names_the_targets_and_follows_the_inputs() {
    let mut program = stop_watchdogs();
    program.push(HANG);
    let options = WiredOptions {
        power_hold_ms: Some(20),
        usb_power: true,
        sd: None,
    };
    let mut wired = power_on(&x4(), &program, options);
    let buttons = wired.buttons.clone().unwrap();
    assert_eq!(buttons.target_name(0), Some("power"));
    assert_eq!(buttons.target_name(7), Some(USB_POWER));
    assert_eq!(buttons.target_name(8), None);
    assert_eq!(
        buttons.is_pressed("power"),
        Some(false),
        "not before its date"
    );
    run_to(&mut wired.machine, 10 * MILLISECOND);
    assert_eq!(buttons.is_pressed("power"), Some(true));
    assert_eq!(buttons.cable(), Some(true));
    run_to(&mut wired.machine, 21 * MILLISECOND);
    assert_eq!(buttons.is_pressed("power"), Some(false));
    assert_eq!(buttons.is_pressed("menu"), None);
    assert_eq!(
        wired.machine.send_board_input(
            cycles(30 * MILLISECOND),
            BoardInput {
                target: 8,
                value: 1
            }
        ),
        Err(InputError::Board(BoardInputError::UnknownTarget {
            target: 8
        }))
    );
    assert_eq!(
        wired.machine.send_board_input(
            cycles(30 * MILLISECOND),
            BoardInput {
                target: 1,
                value: 2
            }
        ),
        Err(InputError::Board(BoardInputError::Value {
            target: 1,
            value: 2
        }))
    );
}

/// GPIO13 made a GPIO and enabled as an output: `OUT` low since reset, the latch cuts the
/// battery; then parks.
fn cut_the_latch() -> Vec<u32> {
    let mut program = stop_watchdogs();
    program.extend(store_register(register("IO_MUX", "GPIO13"), GPIO_FUNCTION));
    program.extend(store_register(register("GPIO", "ENABLE_W1TS"), 1 << 13));
    program.push(HANG);
    program
}

#[test]
fn the_x4_stays_powered_while_its_key_is_held_and_goes_off_when_it_is_let_go() {
    let options = WiredOptions {
        power_hold_ms: Some(5),
        ..WiredOptions::default()
    };
    let mut wired = power_on(&x4(), &cut_the_latch(), options);
    assert_eq!(
        run_to(&mut wired.machine, 50 * MILLISECOND),
        StopReason::PowerOff
    );
    let lost = wired.machine.power_lost_nanos().unwrap();
    assert!(
        (5 * MILLISECOND..5 * MILLISECOND + 100).contains(&lost),
        "off when the key is let go: {lost} ns"
    );
}

#[test]
fn the_cable_keeps_the_x4_powered_with_its_latch_cut() {
    let options = WiredOptions {
        power_hold_ms: Some(0),
        usb_power: true,
        sd: None,
    };
    let mut wired = power_on(&x4(), &cut_the_latch(), options);
    assert_eq!(
        run_to(&mut wired.machine, 20 * MILLISECOND),
        StopReason::TimeBudget
    );
    assert_eq!(wired.machine.power_lost_nanos(), None);
}

#[test]
fn a_cut_latch_without_key_or_cable_turns_the_x4_off_at_once_and_a_released_one_does_not() {
    let mut wired = power_on(&x4(), &cut_the_latch(), no_hold());
    assert_eq!(
        run_to(&mut wired.machine, 20 * MILLISECOND),
        StopReason::PowerOff
    );
    assert!(wired.machine.power_lost_nanos().unwrap() < MILLISECOND);
    let mut program = stop_watchdogs();
    program.push(HANG);
    let mut wired = power_on(&x4(), &program, no_hold());
    assert_eq!(
        run_to(&mut wired.machine, 20 * MILLISECOND),
        StopReason::TimeBudget,
        "a latch the SoC does not drive holds the rail"
    );
}

#[test]
fn the_x4_slot_holds_the_card_it_is_given_and_hands_it_back() {
    let options = WiredOptions {
        sd: Some(SdStart {
            source: Box::new(SparseBlocks::new(2048)),
            config: SdCardConfig::default(),
        }),
        ..no_hold()
    };
    let wired = power_on(&x4(), &[HANG], options);
    let sd = wired.sd.unwrap();
    assert!(sd.has_card());
    let card = sd.take_card().unwrap();
    assert_eq!(card.capacity().bytes(), 2048 * 512);
    assert!(card.into_storage().changes().is_empty());
    assert!(!sd.has_card());
    let empty = power_on(&x4(), &[HANG], no_hold());
    assert!(!empty.sd.unwrap().has_card(), "an empty slot");
}

/// 2d-R4: a device without `[wiring]` is the bare machine, and refuses what it has no line for.
#[test]
fn the_x3_powers_on_bare_and_refuses_what_it_has_no_wiring_for() {
    let x3 = builtin("x3").unwrap();
    let wired = power_on(&x3, &[HANG], WiredOptions::default());
    assert!(wired.panel.is_none() && wired.buttons.is_none() && wired.sd.is_none());
    assert_eq!(board_inputs(&wired.machine), []);
    let refused = |options| {
        Wired::power_on(
            &x3,
            flat_rom(&[HANG]),
            None,
            MachineOptions::default(),
            options,
        )
        .unwrap_err()
        .to_string()
    };
    let cable = refused(WiredOptions {
        usb_power: true,
        ..WiredOptions::default()
    });
    assert!(cable.contains("no USB detect line"), "{cable}");
    let hold = refused(WiredOptions {
        power_hold_ms: Some(10),
        ..WiredOptions::default()
    });
    assert!(hold.contains("no power key"), "{hold}");
    let card = refused(WiredOptions {
        sd: Some(SdStart {
            source: Box::new(SparseBlocks::new(2048)),
            config: SdCardConfig::default(),
        }),
        ..WiredOptions::default()
    });
    assert!(card.contains("no SD card slot"), "{card}");
}
