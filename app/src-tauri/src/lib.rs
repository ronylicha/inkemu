//! The desktop application of inkemu: a Tauri 2 shell that puts the `host` crate behind the
//! commands the interface calls (design of sub-project 2c, C11). It holds no logic of its own:
//! what a run is, how it starts and what it refuses all belong to `host`.
//!
//! **Closing.** The application never closes on a running device: closing the window, quitting
//! the application or a `SIGINT`/`SIGTERM` first powers it off, as its power button would, so
//! that its last life writes its flash file and its session (2c-R17). A power-off that could not
//! keep them is told to the user before the window goes.
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
            if let Err(text) = app.state::<Shell>().power_off() {
                eprintln!("inkemu: {text}");
            }
        }
        _ => {}
    });
    Ok(())
}

/// Powers the device off, and tells the user what its last life could not keep. Called off the
/// main thread: the dialog waits for the user.
fn power_off_telling(app: &AppHandle) {
    if let Err(text) = app.state::<Shell>().power_off() {
        app.dialog()
            .message(text)
            .title("inkemu: the device was powered off without keeping everything")
            .kind(MessageDialogKind::Error)
            .blocking_show();
    }
}

/// On `SIGINT` or `SIGTERM` (Ctrl-C in the terminal that started it, a session that ends), the
/// application powers the device off and exits; a second signal exits at once.
fn watch_signals(app: AppHandle) {
    let signalled = AtomicBool::new(false);
    let watched = ctrlc::set_handler(move || {
        if signalled.swap(true, Ordering::SeqCst) {
            std::process::exit(130);
        }
        // No dialog: the user may be gone (a session that ends); the error goes to the terminal.
        if let Err(text) = app.state::<Shell>().power_off() {
            eprintln!("inkemu: {text}");
        }
        app.exit(130);
    });
    if let Err(e) = watched {
        eprintln!(
            "inkemu: signals are not watched ({e}); close the window to power the device off"
        );
    }
}
