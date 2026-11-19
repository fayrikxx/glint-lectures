use std::collections::HashMap;

use objc2::AnyThread;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext,
    NSRunningApplication,
};
use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_graphics::{
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGWindowAlpha, kCGWindowLayer, kCGWindowName,
    kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize};

use super::{WindowInfo, png_data_url};

const ICON_SIZE: isize = 64;

type WindowDict = CFDictionary<CFString, CFType>;

pub fn list() -> Result<Vec<WindowInfo>, String> {
    // Called from a worker thread that has no autorelease pool of its own.
    autoreleasepool(|_| list_windows())
}

fn list_windows() -> Result<Vec<WindowInfo>, String> {
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let windows = CGWindowListCopyWindowInfo(options, 0)
        .ok_or_else(|| "failed to read the window list".to_string())?;
    // SAFETY: CGWindowListCopyWindowInfo returns an array of dictionaries with string keys.
    let windows: CFRetained<CFArray<WindowDict>> = unsafe { CFRetained::cast_unchecked(windows) };

    let own_pid = i64::from(std::process::id());
    let mut icons: HashMap<i64, Option<String>> = HashMap::new();
    let mut list = Vec::new();

    for index in 0..windows.len() {
        let Some(window) = windows.get(index) else {
            continue;
        };
        // SAFETY: the kCGWindow* keys are constant strings that live for the whole program.
        let (id, pid, layer, alpha, owner, name) = unsafe {
            (
                number(&window, kCGWindowNumber),
                number(&window, kCGWindowOwnerPID),
                number(&window, kCGWindowLayer),
                float(&window, kCGWindowAlpha),
                string(&window, kCGWindowOwnerName),
                string(&window, kCGWindowName),
            )
        };
        let (Some(id), Some(pid)) = (id, pid) else {
            continue;
        };
        // Layer 0 is normal app windows. Menu bar, Dock and overlays sit on other layers.
        if layer != Some(0) || alpha == Some(0.0) || pid == own_pid {
            continue;
        }

        let app_name = owner.unwrap_or_default();
        // Without Screen Recording permission macOS hides other apps' window titles,
        // so fall back to the app name rather than dropping the window.
        let title = name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| app_name.clone());
        if title.trim().is_empty() {
            continue;
        }

        let icon = icons
            .entry(pid)
            .or_insert_with(|| app_icon(pid).map(|png| png_data_url(&png)))
            .clone();
        list.push(WindowInfo {
            id: id as u64,
            title,
            app_name,
            icon,
        });
    }

    Ok(list)
}

fn number(window: &WindowDict, key: &CFString) -> Option<i64> {
    window.get(key)?.downcast_ref::<CFNumber>()?.as_i64()
}

fn float(window: &WindowDict, key: &CFString) -> Option<f64> {
    window.get(key)?.downcast_ref::<CFNumber>()?.as_f64()
}

fn string(window: &WindowDict, key: &CFString) -> Option<String> {
    Some(window.get(key)?.downcast_ref::<CFString>()?.to_string())
}

/// Draws the app icon into a small bitmap. `NSImage` icons carry every size up to
/// 1024 px, and encoding the image directly would keep all of them.
fn app_icon(pid: i64) -> Option<Vec<u8>> {
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid.try_into().ok()?)?;
    let icon = app.icon()?;

    // SAFETY: null planes make AppKit allocate the pixel buffer itself, and the
    // parameters describe a plain 8-bit RGBA bitmap.
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            ICON_SIZE,
            ICON_SIZE,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)?;

    let previous = NSGraphicsContext::currentContext();
    NSGraphicsContext::setCurrentContext(Some(&context));
    let size = ICON_SIZE as f64;
    icon.drawInRect(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(size, size)));
    context.flushGraphics();
    NSGraphicsContext::setCurrentContext(previous.as_deref());

    // SAFETY: an empty properties dictionary is valid for any file type.
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(png.to_vec())
}
