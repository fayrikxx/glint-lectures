use std::collections::HashMap;
use std::ffi::c_void;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, HBITMAP, ReleaseDC,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, EnumWindows, GCLP_HICON, GW_OWNER, GWL_EXSTYLE, GetClassLongPtrW, GetIconInfo,
    GetWindow, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    HICON, ICON_BIG, ICON_SMALL2, ICONINFO, IsWindowVisible, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
    WM_GETICON, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, HSTRING, PWSTR, w};

use super::{WindowInfo, png_data_url};

/// What we need to know about a window to decide whether it belongs in the picker.
#[derive(Debug, Default)]
struct WindowFlags<'a> {
    title: &'a str,
    visible: bool,
    cloaked: bool,
    tool_window: bool,
    no_activate: bool,
    app_window: bool,
    has_owner: bool,
    own_process: bool,
}

/// Roughly the same rules Alt+Tab uses, plus skipping Glint itself.
fn is_pickable(window: &WindowFlags) -> bool {
    // Cloaked windows are UWP apps that are suspended or live on another virtual desktop.
    if !window.visible || window.cloaked || window.own_process || window.title.trim().is_empty() {
        return false;
    }
    // No-activate windows are usually click-through overlays, like a full-screen
    // transparent layer some Electron apps keep on top.
    window.app_window || (!window.tool_window && !window.no_activate && !window.has_owner)
}

struct App {
    name: String,
    exe: Option<HSTRING>,
}

pub fn list() -> Result<Vec<WindowInfo>, String> {
    let mut handles: Vec<HWND> = Vec::new();
    // SAFETY: the callback only runs during this call, while `handles` is alive.
    unsafe { EnumWindows(Some(collect_handle), LPARAM(&raw mut handles as isize)) }
        .map_err(|err| format!("failed to enumerate windows: {err}"))?;

    let own_pid = std::process::id();
    let mut apps: HashMap<u32, App> = HashMap::new();
    let mut windows = Vec::new();

    for hwnd in handles {
        let title = window_title(hwnd);
        let mut pid = 0;
        // SAFETY: plain query on a handle we just got from EnumWindows. If the window
        // is already gone, these calls fail and we skip it below.
        let (visible, ex_style, has_owner) = unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            (
                IsWindowVisible(hwnd).as_bool(),
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32,
                GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()),
            )
        };

        let flags = WindowFlags {
            title: &title,
            visible,
            cloaked: is_cloaked(hwnd),
            tool_window: ex_style & WS_EX_TOOLWINDOW.0 != 0,
            no_activate: ex_style & WS_EX_NOACTIVATE.0 != 0,
            app_window: ex_style & WS_EX_APPWINDOW.0 != 0,
            has_owner,
            own_process: pid == own_pid,
        };
        if !is_pickable(&flags) {
            continue;
        }

        let app = apps.entry(pid).or_insert_with(|| app_for_process(pid));
        windows.push(WindowInfo {
            id: hwnd.0 as usize as u64,
            pid,
            title,
            app_name: app.name.clone(),
            icon: window_icon(hwnd, app.exe.as_ref()).map(|png| png_data_url(&png)),
        });
    }

    Ok(windows)
}

unsafe extern "system" fn collect_handle(hwnd: HWND, handles: LPARAM) -> BOOL {
    // SAFETY: `list` passes a pointer to its Vec and keeps it alive during EnumWindows.
    let handles = unsafe { &mut *(handles.0 as *mut Vec<HWND>) };
    handles.push(hwnd);
    true.into()
}

fn window_title(hwnd: HWND) -> String {
    // SAFETY: the buffer is sized from GetWindowTextLengthW and GetWindowTextW
    // never writes past the slice it gets.
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..copied.max(0) as usize])
    }
}

pub(crate) fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: DWMWA_CLOAKED writes a single u32.
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            size_of::<u32>() as u32,
        )
    };
    result.is_ok() && cloaked != 0
}

fn app_for_process(pid: u32) -> App {
    let Some(exe) = process_exe(pid) else {
        return App {
            name: String::new(),
            exe: None,
        };
    };
    let path = exe.to_string_lossy();
    let name = file_description(&exe)
        .or_else(|| {
            Path::new(&path)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    App {
        name,
        exe: Some(exe),
    }
}

fn process_exe(pid: u32) -> Option<HSTRING> {
    // SAFETY: the handle is closed right after the query, and the buffer size is passed in.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        Some(HSTRING::from_wide(&buf[..len as usize]))
    }
}

/// The "File description" from the exe's version info, e.g. "Microsoft Teams" for ms-teams.exe.
fn file_description(exe: &HSTRING) -> Option<String> {
    // SAFETY: VerQueryValueW returns pointers into `data`, which outlives every read here,
    // and the lengths it reports are in characters for strings and bytes for Translation.
    unsafe {
        let size = GetFileVersionInfoSizeW(exe, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(exe, None, size, data.as_mut_ptr().cast()).ok()?;

        let mut value: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let found = VerQueryValueW(
            data.as_ptr().cast(),
            w!("\\VarFileInfo\\Translation"),
            &mut value,
            &mut len,
        );
        if !found.as_bool() || len < 4 {
            return None;
        }
        let translation = value as *const u16;
        let (language, codepage) = (*translation, *translation.add(1));

        let key = HSTRING::from(format!(
            "\\StringFileInfo\\{language:04x}{codepage:04x}\\FileDescription"
        ));
        let found = VerQueryValueW(data.as_ptr().cast(), &key, &mut value, &mut len);
        if !found.as_bool() || len == 0 {
            return None;
        }
        let text = std::slice::from_raw_parts(value as *const u16, len as usize);
        let text = String::from_utf16_lossy(text);
        let text = text.trim_end_matches('\0').trim();
        (!text.is_empty()).then(|| text.to_string())
    }
}

fn window_icon(hwnd: HWND, exe: Option<&HSTRING>) -> Option<Vec<u8>> {
    // Icons from WM_GETICON and the window class belong to the window, so we must not destroy them.
    let window_icon = message_icon(hwnd, ICON_BIG)
        .or_else(|| message_icon(hwnd, ICON_SMALL2))
        .or_else(|| class_icon(hwnd));
    if let Some(png) = window_icon.and_then(icon_to_png) {
        return Some(png);
    }

    let mut icon = HICON::default();
    // SAFETY: asks for one large icon and we destroy it ourselves afterwards.
    let count = unsafe { ExtractIconExW(exe?, 0, Some(&mut icon), None, 1) };
    if count == 0 || icon.is_invalid() {
        return None;
    }
    let png = icon_to_png(icon);
    // SAFETY: ExtractIconExW gave us ownership of this icon.
    unsafe {
        let _ = DestroyIcon(icon);
    }
    png
}

fn message_icon(hwnd: HWND, kind: u32) -> Option<HICON> {
    let mut result = 0usize;
    // A hung app would block us forever on a plain SendMessage, so give up quickly instead.
    // SAFETY: WM_GETICON has no pointer arguments.
    let sent = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_GETICON,
            WPARAM(kind as usize),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            100,
            Some(&mut result),
        )
    };
    (sent.0 != 0 && result != 0).then_some(HICON(result as *mut c_void))
}

fn class_icon(hwnd: HWND) -> Option<HICON> {
    // SAFETY: plain query, returns 0 when the class has no icon.
    let icon = unsafe { GetClassLongPtrW(hwnd, GCLP_HICON) };
    (icon != 0).then_some(HICON(icon as *mut c_void))
}

fn icon_to_png(icon: HICON) -> Option<Vec<u8>> {
    let mut info = ICONINFO::default();
    // SAFETY: GetIconInfo fills `info` with copies of the icon bitmaps, which we delete below.
    unsafe { GetIconInfo(icon, &mut info) }.ok()?;
    let (color, mask) = (info.hbmColor, info.hbmMask);

    // Monochrome icons have no color bitmap. They are rare enough to just skip.
    let png = if color.is_invalid() {
        None
    } else {
        color_icon_to_png(color, mask)
    };

    // SAFETY: both bitmaps were created for us by GetIconInfo.
    unsafe {
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
    }
    png
}

fn color_icon_to_png(color: HBITMAP, mask: HBITMAP) -> Option<Vec<u8>> {
    let (width, height, mut pixels) = bitmap_bgra(color)?;
    let (bgra, _) = pixels.as_chunks_mut::<4>();

    // Old icons have no alpha channel and keep transparency in the AND mask instead.
    if bgra.iter().all(|px| px[3] == 0) {
        match bitmap_bgra(mask) {
            Some((_, _, mask_pixels)) => {
                for (px, mask_px) in bgra.iter_mut().zip(mask_pixels.as_chunks::<4>().0) {
                    px[3] = if mask_px[0] == 0 { 255 } else { 0 };
                }
            }
            None => bgra.iter_mut().for_each(|px| px[3] = 255),
        }
    }

    for px in bgra.iter_mut() {
        px.swap(0, 2);
    }
    encode_png(width, height, &pixels)
}

/// Reads a bitmap as top-down 32-bit BGRA, converting from whatever depth it has.
fn bitmap_bgra(bitmap: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    let mut header = BITMAP::default();
    // SAFETY: GetObjectW writes at most size_of::<BITMAP>() bytes into `header`.
    let read = unsafe {
        GetObjectW(
            bitmap.into(),
            size_of::<BITMAP>() as i32,
            Some((&raw mut header).cast()),
        )
    };
    if read == 0 || header.bmWidth <= 0 || header.bmHeight <= 0 {
        return None;
    }
    let (width, height) = (header.bmWidth as u32, header.bmHeight as u32);

    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            // Negative height asks for rows from top to bottom.
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    // SAFETY: `pixels` holds exactly `height` rows of 32-bit pixels, matching `info`.
    let lines = unsafe {
        let dc = GetDC(None);
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            height,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, dc);
        lines
    };
    (lines == height as i32).then_some((width, height, pixels))
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(rgba).ok()?;
    writer.finish().ok()?;
    Some(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normal_window(title: &str) -> WindowFlags<'_> {
        WindowFlags {
            title,
            visible: true,
            ..Default::default()
        }
    }

    #[test]
    fn keeps_regular_app_windows() {
        assert!(is_pickable(&normal_window("Lecture 4 - Zoom")));
    }

    #[test]
    fn skips_hidden_cloaked_and_untitled_windows() {
        let hidden = WindowFlags {
            visible: false,
            ..normal_window("Hidden")
        };
        let cloaked = WindowFlags {
            cloaked: true,
            ..normal_window("Settings")
        };
        assert!(!is_pickable(&hidden));
        assert!(!is_pickable(&cloaked));
        assert!(!is_pickable(&normal_window("")));
        assert!(!is_pickable(&normal_window("   ")));
    }

    #[test]
    fn skips_tool_overlay_and_owned_windows() {
        let tool = WindowFlags {
            tool_window: true,
            ..normal_window("Floating toolbar")
        };
        let overlay = WindowFlags {
            no_activate: true,
            ..normal_window("Claude")
        };
        let owned = WindowFlags {
            has_owner: true,
            ..normal_window("Save as")
        };
        assert!(!is_pickable(&tool));
        assert!(!is_pickable(&overlay));
        assert!(!is_pickable(&owned));
    }

    #[test]
    fn app_window_style_wins_over_owner_and_tool_flags() {
        let window = WindowFlags {
            app_window: true,
            tool_window: true,
            has_owner: true,
            ..normal_window("Meeting")
        };
        assert!(is_pickable(&window));
    }

    #[test]
    fn skips_our_own_windows() {
        let glint = WindowFlags {
            own_process: true,
            ..normal_window("Glint")
        };
        assert!(!is_pickable(&glint));
    }
}
