//! The desktop application of inkemu: a Tauri 2 shell that puts the `host` crate behind the
//! commands the interface calls (design of sub-project 2c, C11). It holds no logic of its own:
//! what a run is, how it starts and what it refuses all belong to `host`.
mod commands;

use tauri::{Manager as _, RunEvent};

/// Runs the application until its last window closes.
///
/// # Errors
/// [`tauri::Error`] when the application cannot start.
pub fn run() -> Result<(), tauri::Error> {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::Shell::default())
        .invoke_handler(tauri::generate_handler![
            commands::devices,
            commands::power_on,
            commands::open_replay,
            commands::command,
            commands::frame,
            commands::save_capture,
            commands::notices,
        ])
        .build(tauri::generate_context!())?;
    app.run(|app, event| {
        // Closing the application ends the last life of the device as a power-off does: its
        // flash file and its session are written before the process ends.
        if let RunEvent::Exit = event {
            app.state::<commands::Shell>().power_off();
        }
    });
    Ok(())
}
