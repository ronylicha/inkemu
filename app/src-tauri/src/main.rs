// No console window behind the application on Windows, in a release build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    match inkemu_app_lib::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: the application could not run: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
