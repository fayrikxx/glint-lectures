use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use glint_core::{BgraFrame, Rect};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_app_kit::NSWorkspace;
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGRect,
};
use objc2_core_graphics::{
    CGPreflightScreenCaptureAccess, CGRectMakeWithDictionaryRepresentation,
    CGRequestScreenCaptureAccess, CGWindowListCopyWindowInfo, CGWindowListOption, kCGWindowAlpha,
    kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer,
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelFormatType_32BGRA, kCVReturnSuccess,
};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol, NSURL, ns_string};
use objc2_screen_capture_kit::{
    SCContentFilter, SCFrameStatus, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamDelegate, SCStreamErrorCode, SCStreamFrameInfoScaleFactor, SCStreamFrameInfoStatus,
    SCStreamOutput, SCStreamOutputType, SCWindow,
};

use super::{CaptureFeed, CaptureState, lock, next_frame_id};
use crate::window_list::{WindowDict, float, number};

/// How often the capture thread checks on the window. Frames don't wait for it, they come
/// straight from ScreenCaptureKit.
const TICK: Duration = Duration::from_millis(250);
/// ScreenCaptureKit answers within milliseconds. This only keeps a hang from blocking Glint.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

enum Signal {
    Stop,
    /// The user stopped sharing from the menu bar.
    StoppedByUser,
    /// macOS stopped the stream, for example around sleep. Worth one restart.
    Interrupted,
    /// The stream stopped for any other reason, the window closing among them.
    Failed(String),
}

/// A running capture. Dropping it stops the capture and waits for its thread.
pub struct Capture {
    signals: Sender<Signal>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.signals.send(Signal::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn start(window_id: u64, pid: u32, feed: Arc<CaptureFeed>) -> Result<Capture, String> {
    if !has_screen_access() {
        // macOS asks only once, and a yes only counts after Glint restarts.
        let _ = CGRequestScreenCaptureAccess();
        return Err("macOS hasn't let Glint record the screen yet".to_string());
    }
    let window_id = u32::try_from(window_id)
        .map_err(|_| "that window is gone, refresh the list".to_string())?;

    let (signals, signal_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let stream_signals = signals.clone();
    // ScreenCaptureKit objects aren't Send, so they are created and dropped on this thread.
    let thread = thread::Builder::new()
        .name("glint-capture".to_string())
        .spawn(move || run(window_id, pid, feed, signal_rx, stream_signals, ready_tx))
        .map_err(|err| format!("failed to start the capture thread: {err}"))?;

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Capture {
            signals,
            thread: Some(thread),
        }),
        Ok(Err(err)) => {
            let _ = thread.join();
            Err(err)
        }
        Err(_) => {
            let _ = thread.join();
            Err("the capture thread stopped before capture started".to_string())
        }
    }
}

/// Whether macOS lets Glint record other windows.
pub fn has_screen_access() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Opens the Screen Recording page of System Settings.
pub fn open_screen_settings() -> Result<(), String> {
    let opened = NSURL::URLWithString(ns_string!(
        "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
    ))
    .is_some_and(|url| NSWorkspace::sharedWorkspace().openURL(&url));
    if opened {
        Ok(())
    } else {
        Err("failed to open System Settings".to_string())
    }
}

fn run(
    window_id: u32,
    pid: u32,
    feed: Arc<CaptureFeed>,
    signals: Receiver<Signal>,
    stream_signals: Sender<Signal>,
    ready: Sender<Result<(), String>>,
) {
    let started = autoreleasepool(|_| Stream::start(window_id, pid, &feed, &stream_signals));
    let mut stream = match started {
        Ok(stream) => stream,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    let _ = ready.send(Ok(()));

    let mut restarted = false;
    loop {
        let signal = signals.recv_timeout(TICK);
        // Checked first, so that a stream that stopped because the window closed says so.
        let Some(window) = autoreleasepool(|_| window_spot(window_id)) else {
            feed.set_state(CaptureState::WindowClosed);
            break;
        };
        match signal {
            Ok(Signal::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Signal::StoppedByUser) => {
                feed.set_state(failed("screen recording was stopped from the menu bar"));
                break;
            }
            Ok(Signal::Failed(message)) => {
                feed.set_state(failed(&message));
                break;
            }
            Ok(Signal::Interrupted) => {
                if restarted {
                    feed.set_state(failed("macOS stopped the capture twice"));
                    break;
                }
                restarted = true;
                let restart =
                    autoreleasepool(|_| Stream::start(window_id, pid, &feed, &stream_signals));
                match restart {
                    Ok(new) => stream = new,
                    Err(err) => {
                        feed.set_state(failed(&format!("failed to restart capture: {err}")));
                        break;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
        }

        feed.set_state(autoreleasepool(|_| window_state(window_id, &window)));
        autoreleasepool(|_| stream.fit_to(&window));
    }
    autoreleasepool(|_| drop(stream));
    feed.frames.close();
}

fn failed(message: &str) -> CaptureState {
    CaptureState::Failed {
        message: message.to_string(),
    }
}

/// One ScreenCaptureKit stream and the object it hands frames to.
struct Stream {
    stream: Retained<SCStream>,
    config: Retained<SCStreamConfiguration>,
    handler: Retained<StreamHandler>,
    /// The size frames come in, in pixels.
    size: (usize, usize),
    _queue: DispatchRetained<DispatchQueue>,
}

impl Stream {
    fn start(
        window_id: u32,
        pid: u32,
        feed: &Arc<CaptureFeed>,
        signals: &Sender<Signal>,
    ) -> Result<Self, String> {
        let window = find_window(window_id, pid)?;
        // SAFETY: `window` comes from the current shareable content, and the getters only
        // read the new filter.
        let (filter, scale, size) = unsafe {
            let filter = SCContentFilter::initWithDesktopIndependentWindow(
                SCContentFilter::alloc(),
                &window,
            );
            let scale = f64::from(filter.pointPixelScale());
            let size = pixel_size(filter.contentRect(), scale);
            (filter, scale, size)
        };

        // SAFETY: plain setters on a configuration nothing else uses yet.
        let config = unsafe {
            let config = SCStreamConfiguration::new();
            config.setWidth(size.0);
            config.setHeight(size.1);
            config.setPixelFormat(kCVPixelFormatType_32BGRA);
            // Four frames a second is plenty for slides. A change in between isn't lost: the
            // next frame shows it.
            config.setMinimumFrameInterval(CMTime::new(1, 4));
            // Mouse movement isn't a slide change.
            config.setShowsCursor(false);
            config.setIgnoreShadowsSingleWindow(true);
            config
        };

        let handler = StreamHandler::new(Arc::clone(feed), signals.clone(), scale);
        let queue = DispatchQueue::new("glint-capture-frames", None);
        // SAFETY: the filter and configuration are complete. `queue` is serial, so frames
        // reach the handler one at a time.
        let stream = unsafe {
            let stream = SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                Some(ProtocolObject::from_ref(&*handler)),
            );
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    ProtocolObject::from_ref(&*handler),
                    SCStreamOutputType::Screen,
                    Some(&*queue),
                )
                .map_err(|err| {
                    format!("failed to set up capture: {}", err.localizedDescription())
                })?;
            stream
        };

        let (started, started_rx) = mpsc::channel();
        let on_start = RcBlock::new(move |error: *mut NSError| {
            let _ = started.send(error_text(error));
        });
        // SAFETY: plain call on a stream that is fully set up.
        unsafe { stream.startCaptureWithCompletionHandler(Some(&*on_start)) };
        match started_rx.recv_timeout(REPLY_TIMEOUT) {
            Ok(None) => {}
            Ok(Some(err)) => return Err(format!("failed to start window capture: {err}")),
            Err(_) => return Err("macOS didn't start the capture".to_string()),
        }

        Ok(Self {
            stream,
            config,
            handler,
            size,
            _queue: queue,
        })
    }

    /// Follows the window's size. The stream keeps the output size it started with, so after
    /// a resize the window would be scaled or cut to fit it.
    fn fit_to(&mut self, window: &WindowSpot) {
        let size = pixel_size(window.bounds, self.handler.scale());
        if size == self.size {
            return;
        }
        self.size = size;
        // SAFETY: plain setters on our own configuration, then an update of our own stream.
        unsafe {
            self.config.setWidth(size.0);
            self.config.setHeight(size.1);
            self.stream
                .updateConfiguration_completionHandler(&self.config, None);
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let (stopped, stopped_rx) = mpsc::channel();
        let on_stop = RcBlock::new(move |_: *mut NSError| {
            let _ = stopped.send(());
        });
        // SAFETY: plain call on our own stream. Stopping one that macOS already stopped only
        // reports an error, which doesn't matter here.
        unsafe {
            self.stream
                .stopCaptureWithCompletionHandler(Some(&*on_stop))
        };
        // Frames already on their way still reach the handler until the stop completes.
        let _ = stopped_rx.recv_timeout(REPLY_TIMEOUT);
    }
}

/// Hands an object from the queue that produced it over to the capture thread.
struct Handover<T>(T);

// SAFETY: the completion handler gives the object away and never touches it again, so only
// one thread uses it at a time. Objective-C reference counting is thread-safe.
unsafe impl<T> Send for Handover<T> {}

/// Finds the window in ScreenCaptureKit's list. Window ids get reused, so the owner has to
/// match too.
fn find_window(window_id: u32, pid: u32) -> Result<Retained<SCWindow>, String> {
    let (reply, reply_rx) = mpsc::channel();
    let on_content = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY: the pointer is either null or a valid object for the length of the call.
            let result = match unsafe { Retained::retain(content) } {
                Some(content) => Ok(Handover(content)),
                None => Err(error_text(error).unwrap_or_else(|| "no windows".to_string())),
            };
            let _ = reply.send(result);
        },
    );
    // SAFETY: plain call. Windows that are minimized or on another desktop count too.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true,
            false,
            &on_content,
        );
    }
    let content = match reply_rx.recv_timeout(REPLY_TIMEOUT) {
        Ok(Ok(Handover(content))) => content,
        Ok(Err(err)) => return Err(format!("failed to find the window: {err}")),
        Err(_) => return Err("macOS didn't send the window list".to_string()),
    };

    // SAFETY: plain getters on a snapshot of the shareable content.
    let windows = unsafe { content.windows() };
    windows
        .iter()
        .find(|window| {
            // SAFETY: plain getters on a window from the snapshot.
            unsafe {
                window.windowID() == window_id
                    && window
                        .owningApplication()
                        .is_some_and(|app| u32::try_from(app.processID()) == Ok(pid))
            }
        })
        .ok_or_else(|| "that window is gone, refresh the list".to_string())
}

/// The error a completion handler got, if any.
fn error_text(error: *mut NSError) -> Option<String> {
    // SAFETY: completion handlers get either null or a valid error.
    unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string())
}

/// ScreenCaptureKit wants the output size in pixels, while window sizes come in points.
fn pixel_size(bounds: CGRect, scale: f64) -> (usize, usize) {
    let pixels = |points: f64| (points * scale).round().max(1.0) as usize;
    (pixels(bounds.size.width), pixels(bounds.size.height))
}

struct HandlerIvars {
    feed: Arc<CaptureFeed>,
    signals: Sender<Signal>,
    /// The buffer of the frame `publish` handed back, reused for the next one.
    spare: Mutex<Vec<u8>>,
    /// Pixels per point on the window's display, from the newest frame. It changes when the
    /// window moves to another display.
    scale: Mutex<f64>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and StreamHandler doesn't implement
    // Drop.
    #[unsafe(super(NSObject))]
    #[name = "GlintStreamHandler"]
    #[ivars = HandlerIvars]
    struct StreamHandler;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for StreamHandler {}

    // SAFETY: the signature matches the protocol method.
    unsafe impl SCStreamOutput for StreamHandler {
        // Runs on the frame queue for every frame.
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output(
            &self,
            _stream: &SCStream,
            sample: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind == SCStreamOutputType::Screen {
                autoreleasepool(|_| self.take_frame(sample));
            }
        }
    }

    // SAFETY: the signature matches the protocol method. The error is optional because
    // macOS has been seen to pass none.
    unsafe impl SCStreamDelegate for StreamHandler {
        #[unsafe(method(stream:didStopWithError:))]
        fn stream_did_stop(&self, _stream: &SCStream, error: Option<&NSError>) {
            let _ = self.ivars().signals.send(stop_signal(error));
        }
    }
);

impl StreamHandler {
    fn new(feed: Arc<CaptureFeed>, signals: Sender<Signal>, scale: f64) -> Retained<Self> {
        let this = Self::alloc().set_ivars(HandlerIvars {
            feed,
            signals,
            spare: Mutex::new(Vec::new()),
            scale: Mutex::new(scale),
        });
        // SAFETY: NSObject's init takes no arguments.
        unsafe { msg_send![super(this), init] }
    }

    fn scale(&self) -> f64 {
        *lock(&self.ivars().scale)
    }

    fn take_frame(&self, sample: &CMSampleBuffer) {
        let Some((status, scale)) = frame_info(sample) else {
            return;
        };
        // Idle frames repeat the last one, and blank or suspended ones don't show the window.
        if status != SCFrameStatus::Complete {
            return;
        }
        if let Some(scale) = scale {
            *lock(&self.ivars().scale) = scale;
        }
        // SAFETY: the sample buffer stays valid for the whole callback.
        let Some(pixels) = (unsafe { sample.image_buffer() }) else {
            return;
        };

        let mut buffer = std::mem::take(&mut *lock(&self.ivars().spare));
        let Some(frame) = copy_pixels(&pixels, &mut buffer) else {
            *lock(&self.ivars().spare) = buffer;
            return;
        };
        if let Some(previous) = self.ivars().feed.frames.publish(next_frame_id(), frame) {
            *lock(&self.ivars().spare) = previous.into_pixels();
        }
    }
}

fn stop_signal(error: Option<&NSError>) -> Signal {
    let Some(error) = error else {
        return Signal::Failed("the capture stopped".to_string());
    };
    let code = SCStreamErrorCode(error.code());
    if code == SCStreamErrorCode::UserStopped {
        Signal::StoppedByUser
    } else if code == SCStreamErrorCode::SystemStoppedStream {
        Signal::Interrupted
    } else {
        Signal::Failed(error.localizedDescription().to_string())
    }
}

/// The frame's status, and the display scale it was captured at when the frame says.
fn frame_info(sample: &CMSampleBuffer) -> Option<(SCFrameStatus, Option<f64>)> {
    // SAFETY: the sample buffer stays valid for the whole callback.
    let attachments = unsafe { sample.sample_attachments_array(false) }?;
    // SAFETY: ScreenCaptureKit gives every video frame one attachments dictionary with string
    // keys.
    let attachments: CFRetained<CFArray<CFDictionary<CFString, CFType>>> =
        unsafe { CFRetained::cast_unchecked(attachments) };
    let info = attachments.get(0)?;
    // SAFETY: the SCStreamFrameInfo keys are constant strings that live for the whole program.
    let (status_key, scale_key): (&CFString, &CFString) = unsafe {
        (
            SCStreamFrameInfoStatus.as_ref(),
            SCStreamFrameInfoScaleFactor.as_ref(),
        )
    };
    let status = info.get(status_key)?.downcast_ref::<CFNumber>()?.as_i64()?;
    let scale = info
        .get(scale_key)
        .and_then(|scale| scale.downcast_ref::<CFNumber>()?.as_f64());
    Some((SCFrameStatus(status as isize), scale))
}

/// Copies a BGRA pixel buffer into `buffer`, dropping the padding at the end of each row.
fn copy_pixels(pixels: &CVPixelBuffer, buffer: &mut Vec<u8>) -> Option<BgraFrame> {
    if CVPixelBufferGetPixelFormatType(pixels) != kCVPixelFormatType_32BGRA {
        return None;
    }
    let flags = CVPixelBufferLockFlags::ReadOnly;
    // SAFETY: a read-only lock on a live pixel buffer, released below with the same flags.
    if unsafe { CVPixelBufferLockBaseAddress(pixels, flags) } != kCVReturnSuccess {
        return None;
    }
    let (width, height) = (
        CVPixelBufferGetWidth(pixels),
        CVPixelBufferGetHeight(pixels),
    );
    let stride = CVPixelBufferGetBytesPerRow(pixels);
    let base = CVPixelBufferGetBaseAddress(pixels).cast::<u8>();
    let row_len = width * 4;
    let readable = !base.is_null() && stride >= row_len;
    if readable {
        buffer.clear();
        buffer.reserve(row_len * height);
        for y in 0..height {
            // SAFETY: while locked, the buffer holds `height` rows that are `stride` bytes
            // apart, and each row starts with `width` BGRA pixels.
            let row = unsafe { std::slice::from_raw_parts(base.add(y * stride), row_len) };
            buffer.extend_from_slice(row);
        }
    }
    // SAFETY: locked above with the same flags.
    unsafe { CVPixelBufferUnlockBaseAddress(pixels, flags) };

    if !readable {
        return None;
    }
    let (width, height) = (u32::try_from(width).ok()?, u32::try_from(height).ok()?);
    BgraFrame::new(width, height, std::mem::take(buffer))
}

/// Where the window server has the window.
struct WindowSpot {
    bounds: CGRect,
    on_screen: bool,
}

/// `None` once the window is gone.
fn window_spot(window_id: u32) -> Option<WindowSpot> {
    let info = CGWindowListCopyWindowInfo(CGWindowListOption::OptionIncludingWindow, window_id)?;
    // SAFETY: CGWindowListCopyWindowInfo returns an array of dictionaries with string keys.
    let info: CFRetained<CFArray<WindowDict>> = unsafe { CFRetained::cast_unchecked(info) };
    let window = info.get(0)?;
    // SAFETY: the kCGWindow* keys are constant strings that live for the whole program.
    let (bounds, on_screen) = unsafe {
        (
            rect(&window, kCGWindowBounds)?,
            flag(&window, kCGWindowIsOnscreen),
        )
    };
    Some(WindowSpot {
        bounds,
        on_screen: on_screen.unwrap_or(false),
    })
}

fn window_state(window_id: u32, window: &WindowSpot) -> CaptureState {
    if !window.on_screen {
        // Minimized, hidden with its app or on another desktop. The window list doesn't
        // say which.
        CaptureState::Hidden
    } else if is_covered(window_id, window.bounds) {
        CaptureState::Covered
    } else {
        CaptureState::Capturing
    }
}

/// Whether other windows hide all of this one. Browsers can stop drawing a window like
/// that, and then its frames stop too.
fn is_covered(window_id: u32, bounds: CGRect) -> bool {
    let Some(above) =
        CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenAboveWindow, window_id)
    else {
        return false;
    };
    // SAFETY: CGWindowListCopyWindowInfo returns an array of dictionaries with string keys.
    let above: CFRetained<CFArray<WindowDict>> = unsafe { CFRetained::cast_unchecked(above) };
    let covers = (0..above.len())
        .filter_map(|index| above.get(index))
        .filter_map(|window| {
            // SAFETY: the kCGWindow* keys are constant strings that live for the whole
            // program.
            let (layer, alpha, bounds) = unsafe {
                (
                    number(&window, kCGWindowLayer),
                    float(&window, kCGWindowAlpha),
                    rect(&window, kCGWindowBounds),
                )
            };
            // Menus, the Dock and overlays sit on other layers, and see-through windows
            // don't hide anything.
            if layer != Some(0) || alpha.is_some_and(|alpha| alpha < 1.0) {
                return None;
            }
            bounds.map(to_rect)
        });
    to_rect(bounds).is_covered_by(covers)
}

/// Reads a rectangle the window server stores as a dictionary, like the window bounds.
fn rect(window: &WindowDict, key: &CFString) -> Option<CGRect> {
    let value = window.get(key)?;
    let dict = value.downcast_ref::<CFDictionary>()?;
    let mut rect = CGRect::default();
    // SAFETY: `dict` describes a rectangle, and `rect` outlives the call.
    unsafe { CGRectMakeWithDictionaryRepresentation(Some(dict), &mut rect) }.then_some(rect)
}

fn flag(window: &WindowDict, key: &CFString) -> Option<bool> {
    Some(window.get(key)?.downcast_ref::<CFBoolean>()?.as_bool())
}

fn to_rect(bounds: CGRect) -> Rect {
    Rect {
        left: bounds.origin.x.round() as i32,
        top: bounds.origin.y.round() as i32,
        right: (bounds.origin.x + bounds.size.width).round() as i32,
        bottom: (bounds.origin.y + bounds.size.height).round() as i32,
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::Instant;

    use glint_core::SlotWait;

    use super::*;
    use crate::window_list::{self, WindowInfo};

    fn wait_for_window(app_name: &str) -> Option<WindowInfo> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let found = window_list::list()
                .ok()
                .and_then(|windows| windows.into_iter().find(|w| w.app_name == app_name));
            if found.is_some() {
                return found;
            }
            thread::sleep(Duration::from_millis(200));
        }
        None
    }

    #[test]
    #[ignore = "needs a desktop session and the Screen Recording permission, run with --ignored"]
    fn captures_a_real_window() {
        // Calculator comes with every Mac and opens a window right away.
        Command::new("open")
            .args(["-a", "Calculator"])
            .status()
            .unwrap();
        let window = wait_for_window("Calculator").expect("Calculator didn't open a window");

        let feed = Arc::new(CaptureFeed::new());
        let result = start(window.id, window.pid, Arc::clone(&feed)).map(|capture| {
            let wait = feed.frames.wait_newer(0, Duration::from_secs(10));
            drop(capture);
            wait
        });
        let _ = Command::new("pkill").args(["-x", "Calculator"]).status();

        let frame = match result {
            Ok(SlotWait::Frame { frame, .. }) => frame,
            Ok(other) => panic!("no frame captured: {other:?}"),
            Err(err) => panic!("capture didn't start: {err}"),
        };
        let (width, height) = (frame.width(), frame.height());
        assert!(
            width >= 100 && height >= 100,
            "unexpected size {width}x{height}"
        );
        // A real window has more than one color in it, a blank frame doesn't.
        let (pixels, _) = frame.pixels().as_chunks::<4>();
        assert!(pixels.iter().any(|pixel| pixel != &pixels[0]));
    }
}
