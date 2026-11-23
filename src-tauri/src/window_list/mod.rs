use serde::Serialize;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "windows")]
mod win;

#[cfg(target_os = "macos")]
pub(crate) use mac::{WindowDict, float, number};
#[cfg(target_os = "windows")]
pub(crate) use win::{is_cloaked, top_level_windows};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    /// HWND on Windows, CGWindowID on macOS. Capture uses it to find the window again.
    pub id: u64,
    /// Owner process. Window ids get reused, so capture checks that this still matches.
    pub pid: u32,
    pub title: String,
    pub app_name: String,
    /// PNG as a data URL, `None` when the app doesn't expose an icon we can read.
    pub icon: Option<String>,
}

/// Top-level windows a lecture could be playing in, in z-order (front first).
pub fn list() -> Result<Vec<WindowInfo>, String> {
    #[cfg(target_os = "windows")]
    return win::list();
    #[cfg(target_os = "macos")]
    return mac::list();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    Err("window capture only works on Windows and macOS".to_string())
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn png_data_url(png: &[u8]) -> String {
    use base64::Engine;

    let encoded = base64::engine::general_purpose::STANDARD.encode(png);
    format!("data:image/png;base64,{encoded}")
}
