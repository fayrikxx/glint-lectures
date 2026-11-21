// macOS gets its backend in a later change, and Linux is only built for the CI checks.
// Without a backend, the parts that only a backend uses look unused there.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use glint_core::{FrameSlot, RgbaImage, SlotWait};
use serde::Serialize;
use tauri::ipc::Response;
use tauri::{AppHandle, Manager, State};

#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
use win as platform;

#[cfg(not(target_os = "windows"))]
mod platform {
    use std::sync::Arc;

    use super::CaptureFeed;

    pub struct Capture;

    pub fn start(_window_id: u64, _pid: u32, _feed: Arc<CaptureFeed>) -> Result<Capture, String> {
        Err("window capture isn't supported on this system yet".to_string())
    }
}

/// Preview frames are shrunk to fit in this square before they go to the UI.
const PREVIEW_MAX_SIZE: u32 = 960;
/// How long a preview request waits for a new frame before it answers with just the state.
const PREVIEW_WAIT: Duration = Duration::from_secs(1);
const PREVIEW_HEADER_LEN: usize = 20;

// Frame ids keep growing across sessions. If they restarted, a UI still holding an id
// from the previous session could wait forever for a "newer" frame of a static window.
static NEXT_FRAME_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CaptureState {
    Capturing,
    /// Minimized windows don't produce frames, so the preview keeps the last one.
    Minimized,
    /// Hidden without being minimized, or moved to another virtual desktop.
    Hidden,
    WindowClosed,
    Failed {
        message: String,
    },
}

impl CaptureState {
    /// The number a preview reply uses for this state. 0 means there's no capture.
    fn code(&self) -> u32 {
        match self {
            CaptureState::Capturing => 1,
            CaptureState::Minimized => 2,
            CaptureState::Hidden => 3,
            CaptureState::WindowClosed => 4,
            CaptureState::Failed { .. } => 5,
        }
    }
}

/// What a running capture shares with the commands: the newest frame and the state.
pub struct CaptureFeed {
    pub frames: FrameSlot,
    state: Mutex<CaptureState>,
}

impl CaptureFeed {
    fn new() -> Self {
        Self {
            frames: FrameSlot::new(),
            state: Mutex::new(CaptureState::Capturing),
        }
    }

    pub fn state(&self) -> CaptureState {
        lock(&self.state).clone()
    }

    pub fn set_state(&self, state: CaptureState) {
        *lock(&self.state) = state;
    }
}

pub fn next_frame_id() -> u64 {
    NEXT_FRAME_ID.fetch_add(1, Ordering::Relaxed)
}

struct CaptureSession {
    id: u64,
    window_id: u64,
    feed: Arc<CaptureFeed>,
    // Dropped last: stops the OS capture and waits for its thread.
    _capture: platform::Capture,
}

impl CaptureSession {
    fn start(window_id: u64, pid: u32) -> Result<Self, String> {
        let feed = Arc::new(CaptureFeed::new());
        let capture = platform::start(window_id, pid, Arc::clone(&feed))?;
        Ok(Self {
            id: NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed),
            window_id,
            feed,
            _capture: capture,
        })
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        // Answer waiting preview requests now rather than after the capture thread stops.
        self.feed.frames.close();
    }
}

/// The running capture, if any. Only one window is captured at a time.
#[derive(Default)]
pub struct Captures(Mutex<Option<CaptureSession>>);

impl Captures {
    fn replace(&self, session: Option<CaptureSession>) -> Option<CaptureSession> {
        std::mem::replace(&mut *lock(&self.0), session)
    }

    fn take_if(&self, session_id: u64) -> Option<CaptureSession> {
        let mut current = lock(&self.0);
        match current.as_ref() {
            Some(session) if session.id == session_id => current.take(),
            _ => None,
        }
    }

    fn feed(&self) -> Option<Arc<CaptureFeed>> {
        lock(&self.0)
            .as_ref()
            .map(|session| Arc::clone(&session.feed))
    }

    /// Stops the running capture. Tauri doesn't drop managed state when the app exits,
    /// so this has to be called from the exit handler.
    pub fn stop(&self) {
        drop(self.replace(None));
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStatus {
    session_id: u64,
    window_id: u64,
    state: CaptureState,
}

/// Starts capturing a window from `list_windows` and returns the session id.
#[tauri::command]
pub async fn start_capture(
    window_id: u64,
    pid: u32,
    captures: State<'_, Captures>,
) -> Result<u64, String> {
    let previous = captures.replace(None);
    let session = blocking(move || {
        drop(previous);
        CaptureSession::start(window_id, pid)
    })
    .await??;
    let id = session.id;
    if let Some(replaced) = captures.replace(Some(session)) {
        blocking(move || drop(replaced)).await?;
    }
    Ok(id)
}

/// Stops the capture, unless a newer one has replaced it since.
#[tauri::command]
pub async fn stop_capture(session_id: u64, captures: State<'_, Captures>) -> Result<(), String> {
    match captures.take_if(session_id) {
        Some(session) => blocking(move || drop(session)).await,
        None => Ok(()),
    }
}

/// Lets the UI pick up a capture that is still running, e.g. after a page reload.
#[tauri::command]
pub fn capture_status(captures: State<Captures>) -> Option<CaptureStatus> {
    lock(&captures.0).as_ref().map(|session| CaptureStatus {
        session_id: session.id,
        window_id: session.window_id,
        state: session.feed.state(),
    })
}

/// Waits for a frame newer than `after` and returns it shrunk for the preview.
/// See [`preview_reply`] for the layout.
#[tauri::command]
pub async fn preview_frame(
    after: u64,
    app: AppHandle,
    captures: State<'_, Captures>,
) -> Result<Response, String> {
    let Some(feed) = captures.feed() else {
        return Ok(Response::new(preview_reply(0, None)));
    };
    let window = app.get_webview_window("main");
    let reply = blocking(move || {
        // Nobody can see the preview while Glint is minimized. Capture keeps going.
        if window.is_some_and(|window| window.is_minimized().unwrap_or(false)) {
            thread::sleep(PREVIEW_WAIT);
            return preview_reply(feed.state().code(), None);
        }
        let frame = match feed.frames.wait_newer(after, PREVIEW_WAIT) {
            SlotWait::Frame { id, frame } => {
                Some((id, frame.shrink_to_rgba(PREVIEW_MAX_SIZE, PREVIEW_MAX_SIZE)))
            }
            SlotWait::TimedOut | SlotWait::Closed => None,
        };
        preview_reply(
            feed.state().code(),
            frame.as_ref().map(|(id, image)| (*id, image)),
        )
    })
    .await?;
    Ok(Response::new(reply))
}

/// Frame id (u64), width (u32), height (u32) and state (u32), all little-endian, then
/// `width * height` RGBA pixels. Frame id 0 means there's no new frame in this reply.
fn preview_reply(state: u32, frame: Option<(u64, &RgbaImage)>) -> Vec<u8> {
    let (id, width, height, pixels) = match frame {
        Some((id, image)) => (id, image.width, image.height, image.pixels.as_slice()),
        None => (0, 0, 0, [].as_slice()),
    };
    let mut reply = Vec::with_capacity(PREVIEW_HEADER_LEN + pixels.len());
    reply.extend_from_slice(&id.to_le_bytes());
    reply.extend_from_slice(&width.to_le_bytes());
    reply.extend_from_slice(&height.to_le_bytes());
    reply.extend_from_slice(&state.to_le_bytes());
    reply.extend_from_slice(pixels);
    reply
}

/// Runs `work` on Tauri's blocking pool: starting and stopping capture wait on threads,
/// and preview requests wait for frames.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|err| format!("background task failed: {err}"))
}

// Every change made under these locks is a single assignment, so a panic elsewhere
// can't leave them half-updated and a poisoned lock is still fine to use.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_reply_without_frame_is_just_the_header() {
        let reply = preview_reply(CaptureState::Minimized.code(), None);
        assert_eq!(reply.len(), PREVIEW_HEADER_LEN);
        assert_eq!(&reply[0..8], &0u64.to_le_bytes());
        assert_eq!(&reply[16..20], &2u32.to_le_bytes());
    }

    #[test]
    fn preview_reply_puts_pixels_after_the_header() {
        let image = RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![1, 2, 3, 255, 4, 5, 6, 255],
        };
        let reply = preview_reply(1, Some((42, &image)));
        assert_eq!(&reply[0..8], &42u64.to_le_bytes());
        assert_eq!(&reply[8..12], &2u32.to_le_bytes());
        assert_eq!(&reply[12..16], &1u32.to_le_bytes());
        assert_eq!(&reply[16..20], &1u32.to_le_bytes());
        assert_eq!(&reply[PREVIEW_HEADER_LEN..], image.pixels.as_slice());
    }
}
