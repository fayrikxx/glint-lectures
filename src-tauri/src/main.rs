// Keeps Windows from opening a console window next to the app in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(err) = tauri::Builder::default().run(tauri::generate_context!()) {
        eprintln!("failed to start glint: {err}");
        std::process::exit(1);
    }
}
