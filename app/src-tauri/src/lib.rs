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

use commands::Shell;
use std::sync::atomic::{AtomicBool, Ordering};
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
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.state::<Shell>().is_on()
            {
                // The window stays until the device is off; closing it again then finds nothing
                // running and goes through.
                api.prevent_close();
                let window = window.clone();
                std::thread::spawn(move || {
                    power_off_telling(window.app_handle());
                    let _ = window.close();
                });
            }
        })
        .build(tauri::generate_context!())?;
    watch_signals(app.handle().clone());
    app.run(|app, event| match event {
        RunEvent::ExitRequested { api, code, .. } if app.state::<Shell>().is_on() => {
            api.prevent_exit();
            let app = app.clone();
            std::thread::spawn(move || {
                power_off_telling(&app);
                app.exit(code.unwrap_or(0));
            });
        }
        // The last resort, when the process ends some other way: nothing is left to tell.
        RunEvent::Exit => {
            let shell = app.state::<Shell>();
            if let Err(detail) = shell.power_off() {
                let (title, text) = commands::closing_dialog(shell.language(), &detail);
                eprintln!("{title}\n{text}");
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
/// application powers the device off, waits for every power-off under way, and exits; a second
/// signal exits at once, losing what a power-off under way had not written yet.
fn watch_signals(app: AppHandle) {
    let signalled = AtomicBool::new(false);
    let watched = ctrlc::set_handler(move || {
        if signalled.swap(true, Ordering::SeqCst) {
            std::process::exit(130);
        }
        // No dialog: the user may be gone (a session that ends); the same words, in the
        // interface's language, go to the terminal.
        let shell = app.state::<Shell>();
        if let Err(detail) = shell.power_off() {
            let (title, text) = commands::closing_dialog(shell.language(), &detail);
            eprintln!("{title}\n{text}");
        }
        app.exit(130);
    });
    if let Err(e) = watched {
        eprintln!(
            "inkemu: signals are not watched ({e}); close the window to power the device off"
        );
    }
}
