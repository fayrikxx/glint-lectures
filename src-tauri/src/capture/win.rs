use std::ffi::c_void;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use glint_core::BgraFrame;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{E_FAIL, HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
};
use windows::core::{Error, Interface, Ref, Result as WinResult, factory};

use super::{CaptureFeed, CaptureState, lock, next_frame_id};
use crate::window_list::is_cloaked;

/// How often the capture thread copies the newest frame to the CPU. Four times a second is
/// enough for slides and keeps the cost of the copy low.
const TICK: Duration = Duration::from_millis(250);
const PIXEL_FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;

enum Signal {
    Stop,
    WindowClosed,
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
    if !window_belongs_to(hwnd_from_id(window_id), pid) {
        return Err("that window is gone, refresh the list".to_string());
    }
    if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
        return Err("window capture needs Windows 10 version 1903 or newer".to_string());
    }

    let (signals, signal_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let closed_signal = signals.clone();
    // The capture objects are created on the capture thread and never leave it.
    let thread = thread::Builder::new()
        .name("glint-capture".to_string())
        .spawn(move || run(window_id, feed, signal_rx, closed_signal, ready_tx))
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

fn run(
    window_id: u64,
    feed: Arc<CaptureFeed>,
    signals: Receiver<Signal>,
    closed_signal: Sender<Signal>,
    ready: Sender<Result<(), String>>,
) {
    let hwnd = hwnd_from_id(window_id);
    let mut wgc = match Wgc::start(hwnd, &closed_signal) {
        Ok(wgc) => wgc,
        Err(err) => {
            let _ = ready.send(Err(format!("failed to start window capture: {err}")));
            return;
        }
    };
    let _ = ready.send(Ok(()));

    let mut restarted = false;
    let mut buffer = Vec::new();
    loop {
        match signals.recv_timeout(TICK) {
            Ok(Signal::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Signal::WindowClosed) => {
                feed.set_state(CaptureState::WindowClosed);
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }

        let state = window_state(hwnd);
        if state == CaptureState::WindowClosed {
            feed.set_state(state);
            break;
        }

        // A driver reset, sleep or an RDP switch can take the device away. WGC then just
        // stops sending frames, which would look exactly like a static slide.
        if wgc.device_lost() {
            if restarted {
                feed.set_state(failed("the graphics device was lost twice"));
                break;
            }
            restarted = true;
            match Wgc::start(hwnd, &closed_signal) {
                Ok(new) => wgc = new,
                Err(err) => {
                    feed.set_state(failed(&format!("failed to restart capture: {err}")));
                    break;
                }
            }
            continue;
        }

        let capturing = state == CaptureState::Capturing;
        feed.set_state(state);
        if !capturing {
            continue;
        }
        // A failed read usually means the device just went away, which the next tick handles.
        if let Ok(Some(frame)) = wgc.read_latest(&mut buffer)
            && let Some(previous) = feed.frames.publish(next_frame_id(), frame)
        {
            buffer = previous.into_pixels();
        }
    }
    feed.frames.close();
}

fn failed(message: &str) -> CaptureState {
    CaptureState::Failed {
        message: message.to_string(),
    }
}

/// One Windows Graphics Capture session with the D3D objects behind it.
struct Wgc {
    gpu: Arc<Mutex<GpuState>>,
    item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    frame_arrived: i64,
    closed: i64,
    staging: Option<ID3D11Texture2D>,
}

/// Shared by the frame pool's thread, which copies each frame into `latest`, and the
/// capture thread, which reads `latest` back on its tick.
struct GpuState {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    // Frames go back to the pool right away. If we only copied a frame on our tick, the
    // last change before a window goes static could be dropped and never sent again.
    latest: Option<ID3D11Texture2D>,
    /// The part of `latest` that holds the newest frame.
    latest_size: (u32, u32),
    pool_size: SizeInt32,
    dirty: bool,
}

impl Wgc {
    fn start(hwnd: HWND, closed_signal: &Sender<Signal>) -> WinResult<Self> {
        let (device, context) = create_device()?;
        // The pool's thread and the capture thread share the immediate context. The return
        // value is only the previous setting.
        let multithread: ID3D11Multithread = context.cast()?;
        // SAFETY: plain call on a live context.
        let _ = unsafe { multithread.SetMultithreadProtected(true) };

        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        // SAFETY: `hwnd` was checked to be a live window just before.
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd)? };
        let size = item.Size()?;
        // A minimized window can report 0x0, and the pool needs at least one pixel.
        let size = SizeInt32 {
            Width: size.Width.max(1),
            Height: size.Height.max(1),
        };
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device(&device)?,
            PIXEL_FORMAT,
            2,
            size,
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        // Mouse movement isn't a slide change. Windows before 10 2004 can't turn the cursor
        // off; that's fine, it just stays in the frames.
        let _ = session.SetIsCursorCaptureEnabled(false);

        let gpu = Arc::new(Mutex::new(GpuState {
            device,
            context,
            latest: None,
            latest_size: (0, 0),
            pool_size: size,
            dirty: false,
        }));
        let frame_arrived = pool.FrameArrived(&TypedEventHandler::new({
            let gpu = Arc::clone(&gpu);
            move |pool: Ref<Direct3D11CaptureFramePool>, _| match pool.as_ref() {
                Some(pool) => copy_new_frame(pool, &gpu),
                None => Ok(()),
            }
        }))?;
        // Only pass the news on: tearing the session down from inside one of its own
        // events races with the pool.
        let signal = closed_signal.clone();
        let closed = item.Closed(&TypedEventHandler::new(move |_, _| {
            let _ = signal.send(Signal::WindowClosed);
            Ok(())
        }))?;
        session.StartCapture()?;

        Ok(Self {
            gpu,
            item,
            pool,
            session,
            frame_arrived,
            closed,
            staging: None,
        })
    }

    fn device_lost(&self) -> bool {
        let device = lock(&self.gpu).device.clone();
        // SAFETY: plain query on a live device.
        unsafe { device.GetDeviceRemovedReason() }.is_err()
    }

    /// Copies `latest` to the CPU if a frame arrived since the last read.
    fn read_latest(&mut self, buffer: &mut Vec<u8>) -> WinResult<Option<BgraFrame>> {
        let (context, staging, width, height) = {
            let mut gpu = lock(&self.gpu);
            let (width, height) = gpu.latest_size;
            let Some(latest) = gpu.latest.clone().filter(|_| gpu.dirty) else {
                return Ok(None);
            };
            gpu.dirty = false;

            let staging = match self.staging.take() {
                Some(staging) if texture_size(&staging) == (width, height) => staging,
                _ => create_texture(&gpu.device, width, height, true)?,
            };
            // SAFETY: both textures are live and at least `width` x `height`.
            unsafe {
                gpu.context.CopySubresourceRegion(
                    &staging,
                    0,
                    0,
                    0,
                    0,
                    &latest,
                    0,
                    Some(&region(width, height)),
                );
            }
            self.staging = Some(staging.clone());
            (gpu.context.clone(), staging, width, height)
        };

        // Map waits for the copy above to finish, so it runs outside the lock: the pool's
        // thread must never wait on us, or the pool drops new frames.
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: `staging` is a CPU-readable texture that nothing else maps.
        unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))? };
        let row_len = width as usize * 4;
        buffer.clear();
        buffer.reserve(row_len * height as usize);
        for y in 0..height as usize {
            // SAFETY: the mapped texture has `height` rows that are `RowPitch` bytes apart,
            // and each row starts with `width` BGRA pixels.
            let row = unsafe {
                std::slice::from_raw_parts(
                    mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                    row_len,
                )
            };
            buffer.extend_from_slice(row);
        }
        // SAFETY: mapped just above.
        unsafe { context.Unmap(&staging, 0) };

        Ok(BgraFrame::new(width, height, std::mem::take(buffer)))
    }
}

impl Drop for Wgc {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.frame_arrived);
        let _ = self.item.RemoveClosed(self.closed);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// Runs on the frame pool's thread for every frame Windows produces.
fn copy_new_frame(pool: &Direct3D11CaptureFramePool, gpu: &Mutex<GpuState>) -> WinResult<()> {
    let frame = pool.TryGetNextFrame()?;
    let content = frame.ContentSize()?;
    let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
    // SAFETY: the surface of a capture frame is a D3D11 texture.
    let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };

    let mut gpu = lock(gpu);
    if content.Width > 0 && content.Height > 0 && content != gpu.pool_size {
        pool.Recreate(&winrt_device(&gpu.device)?, PIXEL_FORMAT, 2, content)?;
        gpu.pool_size = content;
    }

    let target = (content.Width.max(0) as u32, content.Height.max(0) as u32);
    let source = texture_size(&texture);
    // Right after a window grows, frames still come from the old, smaller buffers.
    // Copy only what the frame really has, like WebRTC does.
    let (width, height) = (target.0.min(source.0), target.1.min(source.1));
    if width == 0 || height == 0 {
        return Ok(());
    }

    let latest = match gpu.latest.clone() {
        Some(latest) if texture_size(&latest) == target => latest,
        _ => {
            let latest = create_texture(&gpu.device, target.0, target.1, false)?;
            gpu.latest = Some(latest.clone());
            latest
        }
    };
    // SAFETY: both textures are live and at least `width` x `height`.
    unsafe {
        gpu.context.CopySubresourceRegion(
            &latest,
            0,
            0,
            0,
            0,
            &texture,
            0,
            Some(&region(width, height)),
        );
    }
    gpu.latest_size = (width, height);
    gpu.dirty = true;
    frame.Close()
}

fn create_device() -> WinResult<(ID3D11Device, ID3D11DeviceContext)> {
    // WARP covers machines and VMs without a usable GPU driver.
    create_device_of_type(D3D_DRIVER_TYPE_HARDWARE)
        .or_else(|_| create_device_of_type(D3D_DRIVER_TYPE_WARP))
}

fn create_device_of_type(
    driver: D3D_DRIVER_TYPE,
) -> WinResult<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    // SAFETY: the out pointers live for the whole call.
    unsafe {
        D3D11CreateDevice(
            None,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    device.zip(context).ok_or_else(|| Error::from(E_FAIL))
}

/// The WinRT wrapper the capture APIs want around our D3D11 device.
fn winrt_device(device: &ID3D11Device) -> WinResult<IDirect3DDevice> {
    let dxgi: IDXGIDevice = device.cast()?;
    // SAFETY: `dxgi` is a live DXGI device; the wrapper keeps its own reference.
    unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()
}

fn create_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    cpu_readable: bool,
) -> WinResult<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: if cpu_readable {
            D3D11_USAGE_STAGING
        } else {
            D3D11_USAGE_DEFAULT
        },
        BindFlags: 0,
        CPUAccessFlags: if cpu_readable {
            D3D11_CPU_ACCESS_READ.0 as u32
        } else {
            0
        },
        MiscFlags: 0,
    };
    let mut texture = None;
    // SAFETY: `desc` and the out pointer live for the whole call.
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
    texture.ok_or_else(|| Error::from(E_FAIL))
}

fn texture_size(texture: &ID3D11Texture2D) -> (u32, u32) {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: GetDesc only fills in `desc`.
    unsafe { texture.GetDesc(&mut desc) };
    (desc.Width, desc.Height)
}

fn region(width: u32, height: u32) -> D3D11_BOX {
    D3D11_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: width,
        bottom: height,
        back: 1,
    }
}

fn hwnd_from_id(window_id: u64) -> HWND {
    HWND(window_id as usize as *mut c_void)
}

/// Window handles get reused, so check that `hwnd` is still the window that was listed.
fn window_belongs_to(hwnd: HWND, pid: u32) -> bool {
    let mut owner = 0;
    // SAFETY: plain queries that fail harmlessly for a stale handle.
    unsafe {
        IsWindow(Some(hwnd)).as_bool() && {
            GetWindowThreadProcessId(hwnd, Some(&mut owner));
            owner == pid
        }
    }
}

fn window_state(hwnd: HWND) -> CaptureState {
    // SAFETY: plain queries that fail harmlessly if the window is gone.
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            CaptureState::WindowClosed
        } else if IsIconic(hwnd).as_bool() {
            CaptureState::Minimized
        } else if !IsWindowVisible(hwnd).as_bool() || is_cloaked(hwnd) {
            CaptureState::Hidden
        } else {
            CaptureState::Capturing
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use glint_core::SlotWait;
    use windows::Win32::Foundation::{COLORREF, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, PAINTSTRUCT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, MSG,
        PM_REMOVE, PeekMessageW, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WM_PAINT,
        WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };
    use windows::core::w;

    use super::*;

    const RED: [u8; 4] = [0, 0, 255, 255];
    const BLUE: [u8; 4] = [255, 0, 0, 255];

    unsafe extern "system" fn paint_halves(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message != WM_PAINT {
            // SAFETY: forwards the message we were given.
            return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
        }
        // SAFETY: standard WM_PAINT handling on our own window.
        unsafe {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let middle = client.right / 2;
            // COLORREF is 0x00BBGGRR.
            for (left, right, color) in [(0, middle, 0x0000ff), (middle, client.right, 0xff0000)] {
                let brush = CreateSolidBrush(COLORREF(color));
                let half = RECT {
                    left,
                    top: 0,
                    right,
                    bottom: client.bottom,
                };
                FillRect(dc, &half, brush);
                let _ = DeleteObject(brush.into());
            }
            let _ = EndPaint(hwnd, &paint);
        }
        LRESULT(0)
    }

    /// Opens a window painted red on the left and blue on the right, and keeps its message
    /// loop going on another thread until `stop` gets a message.
    fn open_test_window() -> (u64, Sender<()>, JoinHandle<()>) {
        let (window_tx, window_rx) = mpsc::channel();
        let (stop, stop_rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            // SAFETY: plain window setup and a message loop on this thread.
            unsafe {
                let instance = GetModuleHandleW(None).unwrap();
                let class = WNDCLASSW {
                    lpfnWndProc: Some(paint_halves),
                    hInstance: instance.into(),
                    lpszClassName: w!("GlintCaptureTest"),
                    ..Default::default()
                };
                RegisterClassW(&class);
                let hwnd = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("GlintCaptureTest"),
                    w!("Glint capture test"),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    100,
                    100,
                    480,
                    320,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .unwrap();
                window_tx.send(hwnd.0 as usize as u64).unwrap();

                let mut message = MSG::default();
                while stop_rx.try_recv().is_err() {
                    while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                let _ = DestroyWindow(hwnd);
            }
        });
        (window_rx.recv().unwrap(), stop, thread)
    }

    fn pixel(frame: &BgraFrame, x: u32, y: u32) -> [u8; 4] {
        let start = (y * frame.width() + x) as usize * 4;
        frame.pixels()[start..start + 4].try_into().unwrap()
    }

    #[test]
    #[ignore = "needs a desktop session, run with --ignored"]
    fn captures_a_real_window() {
        let (window_id, stop_window, window_thread) = open_test_window();
        let feed = Arc::new(CaptureFeed::new());
        let capture = start(window_id, std::process::id(), Arc::clone(&feed)).unwrap();

        // The first frames can come before the window has painted, so wait for the colors.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut after = 0;
        let mut last = None;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            let SlotWait::Frame { id, frame } = feed.frames.wait_newer(after, left) else {
                break;
            };
            after = id;
            let (width, height) = (frame.width(), frame.height());
            // The capture includes the title bar and borders, so sample the middle of each half.
            let done = pixel(&frame, width / 4, height / 2) == RED
                && pixel(&frame, width * 3 / 4, height / 2) == BLUE;
            last = Some(frame);
            if done {
                break;
            }
        }

        drop(capture);
        stop_window.send(()).unwrap();
        window_thread.join().unwrap();

        let frame = last.expect("no frame captured");
        let (width, height) = (frame.width(), frame.height());
        assert!(
            width >= 400 && height >= 250,
            "unexpected size {width}x{height}"
        );
        assert_eq!(pixel(&frame, width / 4, height / 2), RED);
        assert_eq!(pixel(&frame, width * 3 / 4, height / 2), BLUE);
    }
}
