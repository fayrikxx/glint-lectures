use glint_core::Rect;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    COMPLEXREGION, CreateRectRgn, DeleteObject, GetMonitorInfoW, GetWindowRgn,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetLayeredWindowAttributes, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowRect, IsIconic, IsWindowVisible, IsZoomed,
    LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA, LWA_COLORKEY, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::window_list::{is_cloaked, top_level_windows};

/// Whether other windows hide every pixel of `hwnd` that is on screen. Chrome, Edge and
/// Firefox-based browsers stop drawing a window like that, so its capture freezes.
pub fn is_covered(hwnd: HWND) -> bool {
    let (Ok(windows), Some(area)) = (top_level_windows(), window_area(hwnd)) else {
        return false;
    };
    // The list goes front to back, so the windows before `hwnd` are the ones on top of it.
    let above = windows.into_iter().take_while(|&other| other != hwnd);
    area.intersect(&virtual_screen())
        .is_covered_by(above.filter_map(covering_area))
}

/// The screen area `hwnd` hides from the windows below it, if it counts as covering them.
/// The checks follow the ones Chromium and Firefox make before they stop drawing a window,
/// so Glint warns about the same windows they pause.
fn covering_area(hwnd: HWND) -> Option<Rect> {
    // SAFETY: plain queries that fail harmlessly if the window is gone.
    let (visible, style, ex_style) = unsafe {
        (
            IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool(),
            GetWindowLongPtrW(hwnd, GWL_STYLE) as u32,
            GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32,
        )
    };
    // Clicks fall through WS_EX_TRANSPARENT windows, so they are overlays.
    if !visible || ex_style & WS_EX_TRANSPARENT.0 != 0 {
        return None;
    }
    // Tool windows and popups are often much bigger than what they draw. The taskbar is one
    // of them, but it does hide what's under it.
    let floating = ex_style & WS_EX_TOOLWINDOW.0 != 0 || style & WS_POPUP.0 != 0;
    if floating && !is_taskbar(hwnd) {
        return None;
    }
    if ex_style & WS_EX_LAYERED.0 != 0 && !is_opaque_layered(hwnd) {
        return None;
    }
    if has_complex_region(hwnd) || is_cloaked(hwnd) {
        return None;
    }
    window_area(hwnd).filter(|area| !area.is_empty())
}

/// The window's rectangle. A maximized window hangs over the edges of its monitor by the
/// width of its border, so that part is cut off.
fn window_area(hwnd: HWND) -> Option<Rect> {
    let mut rect = RECT::default();
    // SAFETY: plain queries that fail harmlessly if the window is gone. `monitor` is a local
    // with its size filled in, as GetMonitorInfoW wants.
    unsafe {
        GetWindowRect(hwnd, &mut rect).ok()?;
        let area = to_rect(rect);
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if IsZoomed(hwnd).as_bool()
            && GetMonitorInfoW(
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
            .as_bool()
        {
            return Some(area.intersect(&to_rect(monitor.rcWork)));
        }
        Some(area)
    }
}

fn virtual_screen() -> Rect {
    // SAFETY: plain queries.
    unsafe {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        Rect {
            left,
            top,
            right: left + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: top + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

fn is_taskbar(hwnd: HWND) -> bool {
    let mut class = [0u16; 16];
    // SAFETY: GetClassNameW never writes past the buffer it gets.
    let len = unsafe { GetClassNameW(hwnd, &mut class) };
    class[..len.max(0) as usize]
        .iter()
        .copied()
        .eq("Shell_TrayWnd".encode_utf16())
}

/// Layered windows can be see-through. The ones drawn with UpdateLayeredWindow have no
/// attributes to read, and the browsers treat those as see-through too.
fn is_opaque_layered(hwnd: HWND) -> bool {
    let mut alpha = 0u8;
    let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS::default();
    // SAFETY: both pointers go to locals that outlive the call.
    let read = unsafe {
        GetLayeredWindowAttributes(hwnd, None, Some(&raw mut alpha), Some(&raw mut flags))
    };
    read.is_ok() && !(flags.contains(LWA_ALPHA) && alpha < 255) && !flags.contains(LWA_COLORKEY)
}

/// A window with an odd shape doesn't hide its whole rectangle.
fn has_complex_region(hwnd: HWND) -> bool {
    // SAFETY: the region is created, filled in and deleted right here.
    unsafe {
        let region = CreateRectRgn(0, 0, 0, 0);
        let kind = GetWindowRgn(hwnd, region);
        let _ = DeleteObject(region.into());
        kind == COMPLEXREGION
    }
}

fn to_rect(rect: RECT) -> Rect {
    Rect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}
