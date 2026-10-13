//! The milestones of plan SP1b-3d on firmwares built with ESP-IDF, booted from the real mask ROM:
//! console scripts typed into `uart_select` (UART0, driver, `select`), `usb_serial_jtag_echo`
//! (USB-Serial-JTAG, driver), this repository's `usb_serial_jtag_console` (a console that needs a
//! USB host), and the reference `hello_world`, whose console the ROM mirrors on USB-Serial-JTAG.
//!
//! Each image is named by its own variable and checked against the hash pinned beside its build,
//! `tools/reference-firmware/SHA256SUMS.inputs-*`. The whole suite:
//! `INKEMU_ROM_ELF_DIR=<dir> INKEMU_REF_FLASH=<flash_hello.bin> INKEMU_UART_SELECT_APP=<uart_select.bin> INKEMU_USJ_ECHO_APP=<usb_serial_jtag_echo.bin> INKEMU_USJ_CONSOLE_APP=<usb_serial_jtag_console.bin> cargo test --release -p console-script --test reference_firmware -- --ignored`
//! Build the images with `tools/reference-firmware/build.sh`, which prints their paths.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
#![allow(clippy::disallowed_methods)] // reads the images and the ROM the environment names

use boards::MachineOptions;
use console_script::{Script, ScriptRun, ScriptStatus, VERSION_LINE};
use sha2::{Digest, Sha256};
use soc_esp32c3::test_support::{Bounds, Consoles, Driven, drive};
use soc_esp32c3::{ConsoleChannel, Esp32C3, RomImage, UsbHost};
use std::fmt::Write as _;
use std::path::Path;

const SECOND_NS: u64 = 1_000_000_000;
/// The CPU cycles a run may spend: far more than any of these scripts needs.
const BUDGET: u64 = 2_000_000_000;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

fn rom() -> RomImage {
    let dir = std::env::var("INKEMU_ROM_ELF_DIR").expect(
        "set INKEMU_ROM_ELF_DIR: fetch the ROMs with `inkemu rom fetch --all --rom-dir <dir>`",
    );
    let elf = std::fs::read(Path::new(&dir).join("esp32c3_rev3_rom.elf")).unwrap();
    RomImage::from_elf(&elf).unwrap()
}

/// The SHA-256 pinned for `file` in `tools/reference-firmware/<sums>`, the file the build checks:
/// the one place a hash is written.
fn pinned_sha256(sums: &str, file: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/reference-firmware")
        .join(sums);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; the image is not pinned yet: see tools/reference-firmware/README.md",
            path.display()
        )
    });
    text.lines()
        .find_map(|line| {
            let (hash, name) = line.split_once("  ")?;
            (name == file || name.ends_with(&format!("/{file}"))).then(|| hash.to_owned())
        })
        .unwrap_or_else(|| panic!("{}: no line for {file}", path.display()))
}

/// The image the variable `variable` names, checked against its pinned hash.
fn pinned_image(variable: &str, sums: &str, file: &str) -> Vec<u8> {
    let path = std::env::var(variable).unwrap_or_else(|_| {
        panic!("set {variable} to the {file} of tools/reference-firmware/build.sh")
    });
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(
        hex(&Sha256::digest(&bytes)),
        pinned_sha256(sums, file),
        "{path} is not the pinned {file}; rebuild it with tools/reference-firmware/build.sh"
    );
    bytes
}

/// The X4 with `application` behind the bootloader inkemu ships, as `inkemu run --firmware`
/// composes it.
fn x4_with(application: &[u8], options: MachineOptions) -> Esp32C3 {
    let profile = boards::builtin("x4").unwrap();
    let flash = profile.compose_flash(application, None).unwrap();
    profile
        .instantiate_with_flash(rom(), flash.bytes, options)
        .unwrap()
}

fn script(body: &str) -> Script {
    Script::parse(format!("{VERSION_LINE}\n{body}").as_bytes()).unwrap()
}

/// How a scripted run ended, and what its consoles sent.
struct Scripted {
    consoles: Consoles,
    status: ScriptStatus,
    driven: Driven,
}

impl Scripted {
    /// Whether the script went through before the machine stopped or the bounds ran out.
    fn through(&self) -> bool {
        (&self.status, &self.driven) == (&ScriptStatus::Complete, &Driven::Done)
    }

    /// What a failed assertion prints: how the run ended, and both consoles of the script, so
    /// that a log of the CI says which one went wrong without a second run.
    fn report(&self) -> String {
        format!(
            "status: {:?}\ndriven: {:?}\n--- UART0 ---\n{}\n--- USB-Serial-JTAG ---\n{}",
            self.status,
            self.driven,
            self.consoles.text(ConsoleChannel::Uart0),
            self.consoles.text(ConsoleChannel::UsbSerialJtag)
        )
    }
}

/// Runs `machine` with `script` in calls of `slice` cycles until the script is through, the
/// machine stops, or `max_nanos` of device time have passed.
fn scripted(machine: &mut Esp32C3, script: &Script, slice: u64, max_nanos: u64) -> Scripted {
    let mut run = ScriptRun::start(script, machine).unwrap();
    let mut consoles = Consoles::default();
    let bounds = Bounds {
        slice,
        max_cycles: BUDGET,
        max_nanos,
    };
    let driven = drive(machine, bounds, &mut consoles, |machine, taken, _| {
        run.observe(machine, taken).unwrap();
        run.status(machine.device_time()) == ScriptStatus::Complete
    });
    Scripted {
        consoles,
        status: run.status(machine.device_time()),
        driven,
    }
}

/// The milliseconds an ESP-IDF log line begins with: `I (5321) tag: ...`.
fn log_millis(line: &str) -> u64 {
    let rest = line.split_once(" (").unwrap().1;
    rest.split_once(')').unwrap().0.parse().unwrap()
}

const TIMEOUT_LINE: &str =
    "uart_select_example: Timeout has been reached and nothing has been received";
const UART_SELECT_SCRIPT: &str = "\
wait-for uart0 \"Timeout has been reached and nothing has been received\\r\\n\"
send uart0 \"a\"
wait-for uart0 \"Received: a\\r\\n\"
wait 500ms
send uart0 \"bc\\r\\n\"
wait-for uart0 \"Timeout has been reached and nothing has been received\\r\\n\"
";

fn uart_select(trace_hash: bool) -> Esp32C3 {
    let application = pinned_image(
        "INKEMU_UART_SELECT_APP",
        "SHA256SUMS.inputs-uart-select",
        "uart_select.bin",
    );
    x4_with(
        &application,
        MachineOptions {
            strict_mmio: true,
            trace_hash,
            usb_host: UsbHost::Open,
        },
    )
}

/// Milestone 1 (UR §9.2): the timeout line comes 5 s of device time after `app_main` returns,
/// `a` typed after it gives exactly `Received: a`, and `bc\r\n` typed in one burst gives the four
/// lines in order, each byte its own line: the 174 bytes they make are more than the transmit
/// FIFO holds, so the writer waits for `TXFIFO_EMPTY`. `--strict-mmio` stops at nothing.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_UART_SELECT_APP"]
fn uart_select_reference_firmware_times_out_then_reads_each_byte_typed() {
    let mut machine = uart_select(false);
    let run = scripted(
        &mut machine,
        &script(UART_SELECT_SCRIPT),
        1_000_000,
        30 * SECOND_NS,
    );
    assert!(run.through(), "{}", run.report());
    let console = run.consoles.text(ConsoleChannel::Uart0);
    let lines: Vec<&str> = console.lines().collect();
    let returned = lines
        .iter()
        .position(|line| line.contains("main_task: Returned from app_main()"))
        .unwrap_or_else(|| panic!("{}", run.report()));
    let after: Vec<&str> = lines[returned + 1..]
        .iter()
        .map(|line| line.split_once(") ").map_or(*line, |(_, text)| text))
        .collect();
    assert_eq!(
        after,
        [
            TIMEOUT_LINE,
            "uart_select_example: Received: a",
            "uart_select_example: Received: b",
            "uart_select_example: Received: c",
            "uart_select_example: Received: ",
            "",
            "uart_select_example: Received: ",
            "",
            TIMEOUT_LINE,
        ],
        "{}",
        run.report()
    );
    let waited = log_millis(lines[returned + 1]) - log_millis(lines[returned]);
    assert!(
        (4_990..=5_020).contains(&waited),
        "the first timeout came {waited} ms after app_main returned:\n{}",
        run.report()
    );
}

/// Milestone 5: the same script gives the same run, to the trace hash, whatever the calls the
/// run is cut into.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_UART_SELECT_APP"]
fn uart_select_reference_firmware_typed_into_is_the_same_run_whatever_the_slices() {
    let outcome = |slice: u64| {
        let mut machine = uart_select(true);
        let run = scripted(
            &mut machine,
            &script(UART_SELECT_SCRIPT),
            slice,
            30 * SECOND_NS,
        );
        assert!(run.through(), "calls of {slice} cycles: {}", run.report());
        (
            run.consoles.bytes(ConsoleChannel::Uart0).to_vec(),
            run.consoles.bytes(ConsoleChannel::UsbSerialJtag).to_vec(),
            machine.trace_hash(),
            machine.device_time(),
            machine.instructions(),
        )
    };
    assert_eq!(outcome(1_000_000), outcome(7_919));
}

/// The address of the buffer an `ESP_LOG_BUFFER_HEXDUMP` row dumps: `... Recv str: : 0x3fc8a1b0   68 69 ...`.
fn dump_address(line: &str) -> &str {
    line.split_once(": 0x")
        .unwrap()
        .1
        .split_whitespace()
        .next()
        .unwrap()
}

/// `stream` without the first occurrence of `echo`, or `None` when `echo` is not in it.
fn without_first(stream: &[u8], echo: &[u8]) -> Option<Vec<u8>> {
    let at = stream
        .windows(echo.len())
        .position(|window| window == echo)?;
    Some([&stream[..at], &stream[at + echo.len()..]].concat())
}

/// Milestone 2 (USJ §F.1): with the host open, a line sent after the init line is echoed on
/// USB-Serial-JTAG and dumped on UART0. Then 90 bytes arrive as two packets, 64 and 26, which the
/// example reads in two calls: the second dump starts again at the first address of the buffer.
///
/// The 90 bytes are sent once the dump of the first line has reached UART0: the example's task
/// is then back in `usb_serial_jtag_read_bytes`, so the handler of the first packet wakes it, and
/// it takes those 64 bytes before the host sends the next packet, `HOST_LATENCY` after the handler
/// emptied the FIFO. Sent while the task still prints that dump, both packets may reach the
/// driver's ring before the task reads it, which then reads the 90 bytes in one call: the
/// emulator did so, and whether a chip does is report USJ §O.13. What Espressif's README shows
/// (64 + 24) is a task waiting in its read when the line comes.
///
/// USB-Serial-JTAG carries more than the echo, and ESP-IDF's secondary console plays no part in
/// it: the example sets `CONFIG_ESP_CONSOLE_SECONDARY_NONE`. The copy is the ROM's. The image is
/// a `MINIMAL_BUILD` that links no VFS (report USJ §F.1, §O.9), so `printf` and `ESP_LOG` go
/// through `esp_system_console_put_char` to the ROM's `uart_tx_one_char`, which sends each
/// character to USB-Serial-JTAG too while `g_usb_print` is set; the bootloader inkemu ships sets
/// it and the application does not set the ROM console again (USJ §B.1, §B.2). Every byte the
/// ROM's output routines print, which is all this image prints on UART0, is thus on
/// USB-Serial-JTAG: its stream is the stream of UART0 with the echoed bytes in it, and the two
/// packets of the echo of the 90 bytes may be apart, so each is waited for on its own. The
/// equality below observes that the image writes by the ROM only.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_USJ_ECHO_APP"]
fn usb_serial_jtag_echo_reference_firmware_echoes_on_usb_and_dumps_on_uart0() {
    let application = pinned_image(
        "INKEMU_USJ_ECHO_APP",
        "SHA256SUMS.inputs-usj-echo",
        "usb_serial_jtag_echo.bin",
    );
    let mut machine = x4_with(&application, MachineOptions::default());
    let first = "hi, espressif\n";
    let ninety = "0123456789".repeat(9);
    let (packet, rest) = ninety.split_at(64);
    let body = format!(
        "wait-for uart0 \"USB_SERIAL_JTAG init done\"\nsend usb-serial-jtag \"hi, espressif\\n\"\nwait-for usb-serial-jtag \"hi, espressif\\n\"\nwait-for uart0 \"|hi, espressif.|\"\nsend usb-serial-jtag \"{ninety}\"\nwait-for usb-serial-jtag \"{packet}\"\nwait-for usb-serial-jtag \"{rest}\"\nwait 100ms\n"
    );
    let run = scripted(&mut machine, &script(&body), 1_000_000, 20 * SECOND_NS);
    assert!(run.through(), "{}", run.report());
    let uart0 = run.consoles.text(ConsoleChannel::Uart0);
    let dumps: Vec<&str> = uart0
        .lines()
        .filter(|line| line.contains("Recv str: "))
        .collect();
    assert_eq!(
        dumps.len(),
        7,
        "one row for the first line, 4 + 2 for the 90 bytes:\n{}",
        run.report()
    );
    assert!(
        dumps[0].ends_with("68 69 2c 20 65 73 70 72  65 73 73 69 66 0a        |hi, espressif.|"),
        "{}",
        run.report()
    );
    assert_eq!(
        dump_address(dumps[1]),
        dump_address(dumps[0]),
        "{}",
        run.report()
    );
    assert_eq!(
        dump_address(dumps[5]),
        dump_address(dumps[0]),
        "26 bytes read on their own:\n{}",
        run.report()
    );
    // The 26 bytes are one read: their second row is the next 16 bytes of the buffer, and not a
    // third read of 10 starting at the buffer again.
    let address = |line: &str| u32::from_str_radix(dump_address(line), 16).unwrap();
    assert_eq!(
        address(dumps[6]),
        address(dumps[0]) + 16,
        "the last 10 bytes in the same read as the 16 before them:\n{}",
        run.report()
    );
    assert!(dumps[6].ends_with("|0123456789|"), "{}", run.report());
    // What was written and not yet sent belongs to each console.
    let mut uart0 = run.consoles.bytes(ConsoleChannel::Uart0).to_vec();
    uart0.extend(machine.flush_console(ConsoleChannel::Uart0));
    let mut usb = run.consoles.bytes(ConsoleChannel::UsbSerialJtag).to_vec();
    usb.extend(machine.flush_console(ConsoleChannel::UsbSerialJtag));
    // The echo is on USB-Serial-JTAG only: were it on UART0 too, the ROM's copy would put it on
    // USB-Serial-JTAG twice and the equality below would hold all the same.
    for echo in [first, packet, rest] {
        assert!(
            !uart0
                .windows(echo.len())
                .any(|window| window == echo.as_bytes()),
            "{echo:?} on UART0:\n{}",
            run.report()
        );
    }
    let mirrored = [first, packet, rest]
        .into_iter()
        .try_fold(usb, |stream, echo| without_first(&stream, echo.as_bytes()));
    assert_eq!(
        mirrored.as_deref().map(String::from_utf8_lossy),
        Some(String::from_utf8_lossy(&uart0)),
        "USB-Serial-JTAG is UART0 with the echo in it:\n{}",
        run.report()
    );
}

fn usb_serial_jtag_console(host: UsbHost) -> Esp32C3 {
    let application = pinned_image(
        "INKEMU_USJ_CONSOLE_APP",
        "SHA256SUMS.inputs-usj-console",
        "usb_serial_jtag_console.bin",
    );
    x4_with(
        &application,
        MachineOptions {
            usb_host: host,
            ..MachineOptions::default()
        },
    )
}

/// Milestone 3 (USJ §F.2): the console of this firmware works only with a host that reads the
/// port. Open: `ready`, then the line typed, echoed. Absent: no start-of-frame ever comes, the
/// console discards what is printed, and USB-Serial-JTAG carries nothing, two seconds after the
/// bootloader, which prints on UART0, has started the application.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_USJ_CONSOLE_APP"]
fn usb_serial_jtag_console_reference_firmware_needs_an_open_host() {
    let mut open = usb_serial_jtag_console(UsbHost::Open);
    let body = "wait-for usb-serial-jtag \"ready\\r\\n\"\nsend usb-serial-jtag \"hello\\r\"\nwait-for usb-serial-jtag \"echo: hello\\r\\n\"\n";
    let run = scripted(&mut open, &script(body), 1_000_000, 10 * SECOND_NS);
    assert!(run.through(), "{}", run.report());
    let usb = run.consoles.text(ConsoleChannel::UsbSerialJtag);
    assert!(
        usb.ends_with("ready\necho: hello\n"),
        "{usb:?}\n{}",
        run.report()
    );

    let mut absent = usb_serial_jtag_console(UsbHost::Absent);
    // The last line the bootloader prints on UART0 before it jumps to the application, as the
    // reference boot pins it (`soc-esp32c3/tests/reference_boot.rs`): a firmware that never
    // started would leave USB-Serial-JTAG empty too.
    let body = "wait-for uart0 \"boot: Disabling RNG early entropy source...\"\nwait 2s\n";
    let run = scripted(&mut absent, &script(body), 1_000_000, 10 * SECOND_NS);
    assert!(run.through(), "{}", run.report());
    assert_eq!(
        run.consoles.text(ConsoleChannel::UsbSerialJtag),
        "",
        "{}",
        run.report()
    );
}

/// Milestone 4 (USJ §F.3): with the host open, the ROM mirrors on USB-Serial-JTAG every byte the
/// reference `hello_world` prints on UART0: the two consoles carry the same bytes.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn hello_world_reference_firmware_prints_the_same_bytes_on_usb_serial_jtag_as_on_uart0() {
    let flash = pinned_image("INKEMU_REF_FLASH", "SHA256SUMS.output", "flash_hello.bin");
    let profile = boards::builtin("x4").unwrap();
    let mut machine = profile
        .instantiate_with_flash(rom(), flash, MachineOptions::default())
        .unwrap();
    let body = "wait-for uart0 \"Restarting in 5 seconds...\"\n";
    let run = scripted(&mut machine, &script(body), 1_000_000, 20 * SECOND_NS);
    assert!(run.through(), "{}", run.report());
    // What was written and not yet sent belongs to each console.
    let mut uart0 = run.consoles.bytes(ConsoleChannel::Uart0).to_vec();
    uart0.extend(machine.flush_console(ConsoleChannel::Uart0));
    let mut usb = run.consoles.bytes(ConsoleChannel::UsbSerialJtag).to_vec();
    usb.extend(machine.flush_console(ConsoleChannel::UsbSerialJtag));
    assert_eq!(
        String::from_utf8_lossy(&usb),
        String::from_utf8_lossy(&uart0),
        "{}",
        run.report()
    );
}
