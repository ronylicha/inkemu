//! The desktop application of inkemu: a Tauri 2 shell that puts the `host` crate behind the
//! commands the interface calls (design of sub-project 2c, C11). It holds no logic of its own:
//! what a run is, how it starts and what it refuses all belong to `host`.
//!
//! **Closing.** The application never closes on a running device: closing the window, quitting
//! the application or a `SIGINT`/`SIGTERM` first powers it off, as its power button would, so
//! that its last life writes its flash file and its session (2c-R17). A power-off that could not
//! keep them is told to the user before the window goes, in the language the interface speaks
//! (`set_language`), in a dialog or, for a signal, on the terminal. Each waits for every
//! power-off under way, an emulator a new start replaced included: the shell stays "on" until the
//! last one has written its files. A second signal exits at once, with 130: a power-off still
//! under way then loses what its last life had not written yet (its flash file, its session).
mod commands;
mod signals;

use commands::{ExitDecision, Shell};
use std::io::Write as _;
use std::sync::Arc;
use tauri::{AppHandle, Manager as _, RunEvent, WindowEvent};
use tauri_plugin_dialog::{DialogExt as _, MessageDialogKind};

/// Runs the application until its last window closes.
///
/// # Errors
/// [`tauri::Error`] when the application cannot start.
pub fn run() -> Result<(), tauri::Error> {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Shell::default())
        .invoke_handler(tauri::generate_handler![
            commands::devices,
            commands::power_on,
            commands::open_replay,
            commands::command,
            commands::frame,
            commands::notices,
            commands::set_language,
            commands::inspect,
        ])
        .on_window_event(|window, event| {
            let shell = window.state::<Shell>();
            if let WindowEvent::CloseRequested { api, .. } = event
                && (shell.is_on() || shell.is_closing())
            {
                // The window stays until the device is off and its dialog answered; closing it
                // again meanwhile only waits for the first closing, whose dialog must show.
                api.prevent_close();
                if let Some(closing) = shell.begin_closing() {
                    let window = window.clone();
                    std::thread::spawn(move || {
                        power_off_telling(window.app_handle());
                        // Ended before the window goes; a panic above ends it too.
                        drop(closing);
                        let _ = window.close();
                    });
                }
            }
        })
        .build(tauri::generate_context!())?;
    watch_signals(app.handle().clone());
    app.run(|app, event| match event {
        RunEvent::ExitRequested { api, code, .. } => {
            match app.state::<Shell>().on_exit_request(code) {
                ExitDecision::Exit => {}
                // After a signal, a window closed later exits with the signal's status.
                ExitDecision::ExitSignalled => {
                    api.prevent_exit();
                    app.exit(signals::SIGNALLED);
                }
                ExitDecision::Prevent => {
                    api.prevent_exit();
                    if let Some(closing) = app.state::<Shell>().begin_closing() {
                        let app = app.clone();
                        std::thread::spawn(move || {
                            power_off_telling(&app);
                            drop(closing);
                            app.exit(code.unwrap_or(0));
                        });
                    }
                }
            }
        }
        // The last resort, when the process ends some other way: nothing is left to tell.
        RunEvent::Exit => {
            let shell = app.state::<Shell>();
            if let Err(detail) = shell.power_off() {
                tell_the_terminal(&shell, &detail);
            }
        }
        _ => {}
    });
    Ok(())
}

/// Powers the device off, and tells the user what its last life could not keep. Called off the
/// main thread: the dialog waits for the user.
fn power_off_telling(app: &AppHandle) {
    let shell = app.state::<Shell>();
    if let Err(detail) = shell.power_off() {
        let (title, text) = commands::closing_dialog(shell.language(), &detail);
        app.dialog()
            .message(text)
            .title(title)
            .kind(MessageDialogKind::Error)
            .blocking_show();
    }
}

/// On `SIGINT` or `SIGTERM` (Ctrl-C in the terminal that started it, a session that ends), the
/// application powers the device off, waits for every power-off under way, and exits, on a thread
/// of its own; a second signal exits at once, losing what a power-off under way had not written
/// yet ([`signals::Signals`]).
fn watch_signals(app: AppHandle) {
    let powering_off = app.clone();
    let signals = signals::Signals::new(
        // No dialog: the user may be gone (a session that ends); the same words, in the
        // interface's language, go to the terminal. A closing of the window under way does not
        // hold the exit back: `power_off` waits for every power-off, not for a dialog.
        Arc::new(move || {
            let shell = powering_off.state::<Shell>();
            if let Err(detail) = shell.power_off() {
                tell_the_terminal(&shell, &detail);
            }
        }),
        // Marked first: the exit it asks for then goes through even while a closing of the
        // window waits on its dialog, whose own exit would be swallowed otherwise.
        Arc::new(move |code| {
            app.state::<Shell>().mark_signalled();
            app.exit(code);
        }),
        Arc::new(|code| std::process::exit(code)),
    );
    let watched = ctrlc::set_handler(move || signals.receive());
    if let Err(e) = watched {
        // A terminal that is gone (`EPIPE`, `EIO`) is not a reason to stop.
        let _ = writeln!(
            std::io::stderr(),
            "inkemu: signals are not watched ({e}); close the window to power the device off"
        );
    }
}

/// Writes what the last life could not keep to the terminal, in the interface's language. A
/// terminal that is gone (`EPIPE`, `EIO`) takes nothing, and nothing panics for it.
fn tell_the_terminal(shell: &Shell, detail: &str) {
    let (title, text) = commands::closing_dialog(shell.language(), detail);
    let _ = writeln!(std::io::stderr(), "{title}\n{text}");
}
