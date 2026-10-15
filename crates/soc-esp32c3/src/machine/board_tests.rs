//! The inputs of the board and its supply (ADR 0013): dated, journaled, applied by the machine at
//! one instant past their date, the same whether a front end gives them as it goes or a replay
//! gives them all at power-on.
use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::AcceptedInput;
use crate::test_support::{HANG, T1, T2, flat_rom, load_register, store_register};
use cpu_rv32::asm::{addi, beq, wfi};
use emu_core::{AnalogSource, GpioInput, GpioOutput, Level, Nanos};

/// The key of the tests (target 0) on GPIO3, to ground, and a power latch on GPIO13 (target 1 is
/// the cable): the supply holds while the cable is in, the latch is not driven low, or the key
/// is held.
#[derive(Debug, Default)]
struct Board {
    pressed: bool,
    cable: bool,
    latch_low: bool,
    applied: Vec<(BoardInput, u64)>,
}

impl BoardInputs for Board {
    fn check(&self, input: BoardInput) -> Result<(), BoardInputError> {
        match input {
            BoardInput {
                target: 0 | 1,
                value: 0 | 1,
            } => Ok(()),
            BoardInput {
                target: 0 | 1,
                value,
            } => Err(BoardInputError::Value {
                target: input.target,
                value,
            }),
            BoardInput { target, .. } => Err(BoardInputError::UnknownTarget { target }),
        }
    }

    fn apply(&mut self, input: BoardInput, at: Nanos) {
        match input.target {
            0 => self.pressed = input.value == 1,
            _ => self.cable = input.value == 1,
        }
        self.applied.push((input, at.get()));
    }
}

impl GpioInput for Board {
    fn level(&mut self, _at: Nanos) -> Option<Level> {
        self.pressed.then_some(Level::Low)
    }
}

impl GpioOutput for Board {
    fn driven(&mut self, level: Option<Level>, _at: Nanos) {
        self.latch_low = level == Some(Level::Low);
    }
}

impl Supply for Board {
    fn powered(&mut self, _at: Nanos) -> bool {
        self.cable || !self.latch_low || self.pressed
    }
}

/// The key on a ladder too: GPIO1 reads the rail released, ground pressed.
impl AnalogSource for Board {
    fn millivolts(&mut self, _at: Nanos) -> u32 {
        if self.pressed { 0 } else { 3_300 }
    }
}

const fn key(pressed: bool) -> BoardInput {
    BoardInput {
        target: 0,
        value: pressed as u32,
    }
}

fn gpio(name: &str) -> u32 {
    svd_regs::esp32c3::addr("GPIO", name)
}

/// GPIO3 as a GPIO input with its pull-up (function 1, `FUN_IE`, `FUN_WPU`), then a loop that
/// reads `GPIO.IN` until it differs from its first reading, then parks.
fn wait_for_the_key() -> Vec<u32> {
    let mut program = Vec::new();
    program.extend(store_register(
        svd_regs::esp32c3::addr("IO_MUX", "GPIO3"),
        (1 << 12) | (1 << 9) | (1 << 8),
    ));
    program.extend(load_register(gpio("IN")));
    program.push(addi(T2, T1, 0));
    program.extend(load_register(gpio("IN")));
    program.push(beq(T1, T2, -8));
    program.push(HANG);
    program
}

/// A machine with no watchdog at reset, the trace hash kept, `board` attached to GPIO3 (the key),
/// GPIO13 (the latch) and as the board's inputs and supply.
fn wired(program: &[u32], board: &Rc<RefCell<Board>>) -> Esp32C3 {
    let config = SocConfig {
        strapping: 0,
        trace_hash: true,
        ..SocConfig::default()
    };
    let mut m = Esp32C3::new(flat_rom(program), &config);
    m.attach_input(3, Box::new(Rc::clone(board))).unwrap();
    m.attach_output(13, Box::new(Rc::clone(board))).unwrap();
    m.attach_board_inputs(Box::new(Rc::clone(board)));
    m.attach_supply(Box::new(Rc::clone(board)));
    m
}

#[test]
fn an_input_acts_one_instant_after_its_date_and_is_journaled() {
    let board = Rc::new(RefCell::new(Board::default()));
    let mut m = wired(&wait_for_the_key(), &board);
    m.run_within(Cycles(200), None);
    assert_ne!(m.pc(), RESET_PC + 4 * 10, "the loop waits for the key");
    let at = m.device_time();
    m.send_board_input(at, key(true)).unwrap();
    assert_eq!(m.board_inputs_pending(), 1);
    assert!(!board.borrow().pressed, "not before its instant");
    m.run_within(Cycles(400), None);
    assert_eq!(m.board_inputs_pending(), 0);
    let acted = clock::TIME_BASE.cycles_to_nanos(Cycles(at.get() + 1));
    assert_eq!(board.borrow().applied, [(key(true), acted)]);
    assert_eq!(m.pc(), RESET_PC + 4 * 10, "the loop saw the key and left");
    assert_eq!(
        m.input_journal().inputs(),
        [AcceptedInput::Board {
            at,
            input: key(true)
        }]
    );
}

#[test]
fn an_input_dated_ahead_acts_at_its_instant_even_while_the_cpu_waits() {
    let board = Rc::new(RefCell::new(Board::default()));
    let mut m = wired(&[wfi(), HANG], &board);
    let date = Cycles(16_000);
    m.send_board_input(date, key(true)).unwrap();
    assert_eq!(
        m.run_within(Cycles(u64::MAX), Some(1_000_000)),
        StopReason::Idle
    );
    assert_eq!(
        board.borrow().applied,
        [(key(true), clock::TIME_BASE.cycles_to_nanos(Cycles(16_001)))]
    );
}

#[test]
fn inputs_are_refused_before_now_unknown_without_a_board_or_past_their_bound() {
    let board = Rc::new(RefCell::new(Board::default()));
    let mut m = wired(&[HANG], &board);
    m.run_within(Cycles(10), None);
    assert!(matches!(
        m.send_board_input(Cycles(0), key(true)),
        Err(InputError::InThePast { .. })
    ));
    // So late it would act at the last instant of the time base, where a deep sleep would jump.
    for last in [u64::MAX, u64::MAX - 1] {
        assert!(matches!(
            m.send_board_input(Cycles(last), key(true)),
            Err(InputError::BeyondTime { .. })
        ));
    }
    assert_eq!(m.input_journal().inputs(), []);
    assert_eq!(m.board_inputs_pending(), 0);
    let now = m.device_time();
    assert_eq!(
        m.send_board_input(
            now,
            BoardInput {
                target: 9,
                value: 1
            }
        ),
        Err(InputError::Board(BoardInputError::UnknownTarget {
            target: 9
        }))
    );
    assert_eq!(
        m.send_board_input(
            now,
            BoardInput {
                target: 0,
                value: 2
            }
        )
        .unwrap_err()
        .to_string(),
        "input 0 of the board takes no value 2"
    );
    for n in 0..MAX_PENDING_BOARD_INPUTS as u64 {
        m.send_board_input(Cycles(now.get() + n), key(n % 2 == 0))
            .unwrap();
    }
    assert_eq!(
        m.send_board_input(now, key(true)),
        Err(InputError::TooManyBoardInputs)
    );
    assert_eq!(m.input_journal().inputs().len(), MAX_PENDING_BOARD_INPUTS);
    let mut bare = Esp32C3::new(flat_rom(&[HANG]), &SocConfig::default());
    assert_eq!(
        bare.send_board_input(Cycles(0), key(true)),
        Err(InputError::NoBoard)
    );
    assert_eq!(bare.input_journal().inputs(), []);
}

/// The run of [`wait_for_the_key`] with the key pressed at `date`, given at power-on or when the
/// run reaches it, driven in slices of `slice` cycles: its trace hash, its PC, its device time.
fn run_pressed_at(date: Option<Cycles>, live: bool, slice: u64) -> (u64, u32, Cycles) {
    let board = Rc::new(RefCell::new(Board::default()));
    let mut m = wired(&wait_for_the_key(), &board);
    let mut date = date;
    if !live && let Some(at) = date.take() {
        m.send_board_input(at, key(true)).unwrap();
    }
    let mut cycles = 0;
    while cycles < 2_000 {
        cycles = (cycles + slice).min(2_000);
        m.run_within(Cycles(cycles), None);
        if let Some(at) = date.filter(|at| m.device_time() >= *at) {
            date = None;
            m.send_board_input(m.device_time().max(at), key(true))
                .unwrap();
        }
    }
    assert_eq!(board.borrow().applied.len(), 1);
    (m.trace_hash().unwrap(), m.pc(), m.device_time())
}

#[test]
fn a_key_given_as_the_run_goes_and_given_at_power_on_make_the_same_run() {
    // The live run stops at the end of a slice and presses there: that instant is the date.
    let board = Rc::new(RefCell::new(Board::default()));
    let mut m = wired(&wait_for_the_key(), &board);
    m.run_within(Cycles(300), None);
    let date = m.device_time();
    let live = run_pressed_at(Some(date), true, 300);
    for slice in [1, 7, 997, 2_000] {
        assert_eq!(
            run_pressed_at(Some(date), false, slice),
            live,
            "given at power-on, run in slices of {slice}"
        );
    }
    assert_eq!(live.1, RESET_PC + 4 * 10, "the loop left on the key");
}

fn saradc(name: &str) -> u32 {
    svd_regs::esp32c3::addr("APB_SARADC", name)
}

/// A one-shot conversion of GPIO1 (ADC1 channel 1, 12 dB) as `adc_oneshot` starts it, the clock
/// as `adc_oneshot_hal_setup` leaves it (APB / 16, `SAR_CLK_DIV` 1), then a park: the reading
/// stays in `SAR1DATA_STATUS`.
fn convert_gpio1() -> Vec<u32> {
    let sample = (1 << 31) | (1 << 25) | (3 << 23);
    let mut program = Vec::new();
    program.extend(store_register(saradc("CLKM_CONF"), 15 | (2 << 21)));
    program.extend(store_register(saradc("CTRL"), 0x4000_0040 | (1 << 7)));
    program.extend(store_register(saradc("ONETIME_SAMPLE"), sample));
    program.extend(store_register(saradc("ONETIME_SAMPLE"), sample | (1 << 29)));
    program.push(HANG);
    program
}

/// The ladder machine: [`wired`] with the board's voltage on GPIO1.
fn ladder(board: &Rc<RefCell<Board>>) -> Esp32C3 {
    let mut m = wired(&convert_gpio1(), board);
    m.attach_analog(1, Box::new(Rc::clone(board))).unwrap();
    m
}

/// The device time the conversion has started by: the program parked.
fn started() -> Cycles {
    let board = Rc::new(RefCell::new(Board::default()));
    let mut m = ladder(&board);
    while m.pc() != RESET_PC + 4 * 16 {
        m.step();
    }
    m.device_time()
}

/// The reading and the trace hash of [`convert_gpio1`] with the key pressed at `date`, given once
/// the conversion has started (`live`, as a script or a window gives a dated key while the run
/// goes) or at power-on (as a replay gives it).
fn convert_pressed_at(date: Cycles, start: Cycles, live: bool) -> (u32, u64) {
    let board = Rc::new(RefCell::new(Board::default()));
    let mut m = ladder(&board);
    if live {
        while m.device_time() < start {
            m.step();
        }
    }
    m.send_board_input(date, key(true)).unwrap();
    m.run_within(Cycles(start.get() + 20_000), None);
    assert_eq!(board.borrow().applied.len(), 1);
    (
        bus_word(&mut m, saradc("SAR1DATA_STATUS")),
        m.trace_hash().unwrap(),
    )
}

/// A key that acts at the very cycle a conversion ends is seen by it whether the key was given
/// as the run went or at power-on: the machine applies the inputs due before it asks an analog
/// chip, whatever order the two events were armed in. The end is found first (the first date
/// whose key the conversion misses), then every date around it is tried, so one of them makes
/// the key act at that very cycle.
#[test]
fn a_conversion_ending_as_a_key_acts_reads_the_same_live_and_replayed() {
    const STEP: u64 = 64;
    let start = started();
    let pressed = |reading: u32| reading & 0xFFF < 100;
    let missed = (0..20_000 / STEP)
        .map(|n| n * STEP)
        .find(|&offset| !pressed(convert_pressed_at(Cycles(start.get() + offset), start, false).0))
        .expect("a key dated after the end of the conversion is missed");
    assert!(missed > 0, "the key dated at the start is seen");
    let mut seen = 0;
    for offset in missed - STEP..=missed {
        let date = Cycles(start.get() + offset);
        let live = convert_pressed_at(date, start, true);
        let replayed = convert_pressed_at(date, start, false);
        assert_eq!(live, replayed, "the key dated {date:?}");
        seen += u64::from(pressed(live.0));
    }
    assert!(
        (1..=STEP).contains(&seen),
        "the window holds the end of the conversion: {seen} of {} dates seen",
        STEP + 1
    );
}

/// GPIO13 made a GPIO (function 1), then enabled as an output: `OUT` holds 0 since reset, so the
/// enable drives the latch low, at the eighth word; then parks.
fn cut_the_latch() -> Vec<u32> {
    let mut program = Vec::new();
    program.extend(store_register(
        svd_regs::esp32c3::addr("IO_MUX", "GPIO13"),
        1 << 12,
    ));
    program.extend(store_register(gpio("ENABLE_W1TS"), 1 << 13));
    program.push(HANG);
    program
}

#[test]
fn the_board_cutting_its_supply_stops_the_run_for_good() {
    let program = cut_the_latch();
    let board = Rc::new(RefCell::new(Board::default()));
    let mut m = wired(&program, &board);
    // Pressed from power-on: the key bridges the rail while it is held.
    m.send_board_input(Cycles(0), key(true)).unwrap();
    let release = Cycles(1_600);
    m.send_board_input(release, key(false)).unwrap();
    assert_eq!(
        m.run_within(Cycles(u64::MAX), Some(1_000_000)),
        StopReason::PowerOff
    );
    assert!(board.borrow().latch_low);
    let lost = m.power_lost_nanos().unwrap();
    assert_eq!(
        lost,
        clock::TIME_BASE.cycles_to_nanos(Cycles(release.get() + 1)),
        "off when the key is let go"
    );
    let at = m.device_time();
    assert_eq!(m.run_within(Cycles(u64::MAX), None), StopReason::PowerOff);
    assert_eq!(m.device_time(), at, "nothing runs any more");
    assert_eq!(
        m.send_board_input(at, key(true)),
        Err(InputError::PoweredOff)
    );
    assert_eq!(
        m.send_console(ConsoleChannel::Uart0, at, b"x"),
        Err(InputError::PoweredOff)
    );
}

#[test]
fn a_write_that_cuts_the_supply_stops_the_run_at_that_instruction() {
    let program = cut_the_latch();
    let board = Rc::new(RefCell::new(Board::default()));
    let mut m = wired(&program, &board);
    assert_eq!(
        m.run_within(Cycles(u64::MAX), Some(1_000_000)),
        StopReason::PowerOff
    );
    assert_eq!(
        m.pc(),
        RESET_PC + 4 * 8,
        "the store of the eighth word was the last instruction"
    );
    // A board on a cable keeps the chip powered whatever the latch.
    let cabled = Rc::new(RefCell::new(Board {
        cable: true,
        ..Board::default()
    }));
    let mut on_cable = wired(&program, &cabled);
    assert_eq!(
        on_cable.run_within(Cycles(500), None),
        StopReason::CycleBudget
    );
    assert_eq!(on_cable.power_lost_nanos(), None);
}

fn rtc(name: &str) -> u32 {
    svd_regs::esp32c3::addr("RTC_CNTL", name)
}

/// The index of the word the program of [`sleep_until_the_key`] parks at once woken.
const WOKEN: usize = 24;

/// A program that, on a chip woken from a deep sleep (`SLP_WAKEUP_CAUSE` not 0), parks at word
/// [`WOKEN`]; otherwise arms GPIO3 low as its one wake source, as `esp_deep_sleep_enable_gpio_wakeup`
/// and `gpio_deep_sleep_wakeup_prepare` do (the pull-up of the pad), and sleeps.
fn sleep_until_the_key() -> Vec<u32> {
    use cpu_rv32::asm::bne;
    let mut program = Vec::new();
    program.extend(load_register(rtc("SLP_WAKEUP_CAUSE")));
    program.push(bne(T1, 0, i32::try_from(4 * (WOKEN - 2)).unwrap()));
    program.extend(store_register(
        svd_regs::esp32c3::addr("IO_MUX", "GPIO3"),
        (1 << 12) | (1 << 9) | (1 << 8),
    ));
    program.extend(store_register(
        rtc("GPIO_WAKEUP"),
        1 << (31 - 3) | 4 << (23 - 9) | 1 << 7,
    ));
    program.extend(store_register(rtc("WAKEUP_STATE"), (1 << 2) << 15));
    program.extend(store_register(
        rtc("DIG_PWC"),
        0x0055_5010 | 1 << 31 | 1 << 30 | 1 << 27,
    ));
    program.extend(store_register(rtc("STATE0"), 1 << 31));
    program.push(HANG);
    assert_eq!(program.len(), WOKEN);
    program.push(HANG);
    program
}

fn quiet_wired(program: &[u32], board: &Rc<RefCell<Board>>) -> Esp32C3 {
    let mut m = wired(program, board);
    // The board on a cable: the latch plays no part here.
    board.borrow_mut().cable = true;
    m.inputs_changed();
    m
}

#[test]
fn a_key_pressed_during_a_deep_sleep_wakes_the_chip_at_its_instant() {
    let board = Rc::new(RefCell::new(Board::default()));
    let mut m = quiet_wired(&sleep_until_the_key(), &board);
    let press = Cycles(2 * crate::clock::SECOND);
    m.send_board_input(press, key(true)).unwrap();
    // A call gives control back after each second of sleep (`StopReason::Asleep`).
    while m.run_within(Cycles(u64::MAX), Some(3_000_000_000)) == StopReason::Asleep {}
    assert_eq!(m.pc(), RESET_PC + 4 * u32::try_from(WOKEN).unwrap());
    let resets: Vec<_> = m
        .drain_diagnostics()
        .into_iter()
        .filter_map(|d| match d {
            Diagnostic::Reset { cause, at_us, .. } => Some((cause, at_us)),
            _ => None,
        })
        .collect();
    assert_eq!(
        resets,
        [(ResetCause::DeepSleep, (press.get() + 1) * 25 / 4 / 1_000)],
        "woken at the instant the key acted"
    );
    assert_eq!(
        bus_word(&mut m, rtc("GPIO_WAKEUP")) & 0x3F,
        1 << 3,
        "GPIO3 woke it"
    );
}

#[test]
fn a_key_already_held_at_the_entry_wakes_the_chip_at_once() {
    let board = Rc::new(RefCell::new(Board::default()));
    let mut m = quiet_wired(&sleep_until_the_key(), &board);
    m.send_board_input(Cycles(0), key(true)).unwrap();
    m.run_within(Cycles(400), None);
    assert_eq!(m.pc(), RESET_PC + 4 * u32::try_from(WOKEN).unwrap());
    assert!(m.deep_sleep().is_none());
}

#[test]
fn a_sleep_nothing_can_end_but_a_key_ends_the_run_and_resumes_with_the_key() {
    let board = Rc::new(RefCell::new(Board::default()));
    let mut m = quiet_wired(&sleep_until_the_key(), &board);
    assert_eq!(
        m.run_within(Cycles(u64::MAX), Some(1_000_000_000)),
        StopReason::DeepSleep
    );
    let state = m.deep_sleep().unwrap();
    assert_eq!(state.wake_sources, DeepSleep::GPIO);
    let at = m.device_time();
    m.send_board_input(at, key(true)).unwrap();
    m.run_within(Cycles(u64::MAX), Some(m.device_time_nanos() + 1_000_000));
    assert_eq!(m.pc(), RESET_PC + 4 * u32::try_from(WOKEN).unwrap());
}

/// An aligned word read straight off the bus.
fn bus_word(m: &mut Esp32C3, addr: u32) -> u32 {
    use emu_core::{AccessKind, AccessSize, Bus};
    m.bus
        .read(addr, AccessSize::Word, AccessKind::Load)
        .unwrap()
        .value
}

#[test]
fn a_supply_off_at_power_on_runs_nothing() {
    let board = Rc::new(RefCell::new(Board {
        latch_low: true,
        ..Board::default()
    }));
    let config = SocConfig {
        strapping: 0,
        ..SocConfig::default()
    };
    let mut m = Esp32C3::new(flat_rom(&[HANG]), &config);
    m.attach_supply(Box::new(Rc::clone(&board)));
    assert_eq!(m.run_within(Cycles(100), None), StopReason::PowerOff);
    assert_eq!(m.instructions(), 0);
    assert_eq!(m.power_lost_nanos(), Some(0));
}
