// Keeps Windows from opening a console window next to the app in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod window_list;

use window_list::WindowInfo;

#[tauri::command]
async fn list_windows() -> Result<Vec<WindowInfo>, String> {
    // Reading icons sends a message to every window, so keep it off the async workers.
    tauri::async_runtime::spawn_blocking(window_list::list)
        .await
        .map_err(|err| format!("window listing stopped: {err}"))?
}

fn main() {
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![list_windows])
        .run(tauri::generate_context!());
    if let Err(err) = app {
        eprintln!("failed to start glint: {err}");
        std::process::exit(1);
    }
}
