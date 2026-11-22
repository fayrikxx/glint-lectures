// Keeps Windows from opening a console window next to the app in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod window_list;

use tauri::{Manager, RunEvent};
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
        .manage(capture::Captures::default())
        .invoke_handler(tauri::generate_handler![
            list_windows,
            capture::start_capture,
            capture::stop_capture,
            capture::capture_status,
            capture::set_crop,
            capture::preview_frame,
        ])
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(err) => {
            eprintln!("failed to start glint: {err}");
            std::process::exit(1);
        }
    };
    app.run(|app, event| {
        if let RunEvent::Exit = event {
            app.state::<capture::Captures>().stop();
        }
    });
}
