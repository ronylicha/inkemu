//! The inputs of the board and its supply (ADR 0013): dated, journaled, applied by the machine at
//! one instant past their date, the same whether a front end gives them as it goes or a replay
//! gives them all at power-on.
use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::AcceptedInput;
use crate::test_support::{HANG, T1, T2, flat_rom, load_register, store_register};
use cpu_rv32::asm::{addi, beq, wfi};
use emu_core::{GpioInput, GpioOutput, Level, Nanos};

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
