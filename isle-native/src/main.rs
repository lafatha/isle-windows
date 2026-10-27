//! The native island.
//!
//! One layered, always-on-top, taskbar-less window; one Direct2D software
//! target painting into an off-screen DIB; one 16 ms timer driving the springs.
//! Nothing here starts a PowerShell process or opens a console.
//!
//! `--seconds N` quits after N seconds, which is how the footprint is measured.

#![windows_subsystem = "windows"]

mod app;
mod art;
mod bot;
mod chat;
mod icon;
mod icons;
mod keys;
mod lyrics;
mod media;
mod pages;
mod prefs;
mod render;
mod settings;
mod spring;
mod store;
mod sys;
mod ui;
mod usage;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;

use app::{App, HOST_H, HOST_W};
use render::Renderer;

/// `WM_MOUSELEAVE`. The crate's metadata omits it, so it is spelled here.
const WM_MOUSE_LEAVE: u32 = 0x02A3;
/// A private message the render clock posts to drive `frame` at a steady
/// cadence. `WM_TIMER` aliases against the ~15.6 ms system tick and judders.
/// `WM_APP + 10`, clear of the `prefs` messages that take `WM_APP + 1..=3`.
const WM_APP_FRAME: u32 = 0x800A;
/// 16 ms: fast enough for the springs to look continuous, slow enough that an
/// idle island wakes the CPU about sixty times a second doing nothing.
const FRAME_MS: u32 = 16;
/// Keeps the render-clock thread alive; cleared when the window is destroyed.
static CLOCK_ON: AtomicBool = AtomicBool::new(true);
/// How long the render clock sleeps between frames. The island writes it from
/// the `TargetFps` preference, so a chosen 120/144/240 really does run faster
/// instead of being capped at the clock's default 60.
pub(crate) static FRAME_INTERVAL_MS: AtomicU32 = AtomicU32::new(FRAME_MS);
/// Whether a frame message is already sitting in the queue waiting to be
/// drawn. The clock only posts a new one when this is clear, so a frame that
/// costs more than the interval cannot pile up stale messages ahead of the
/// click and move messages - which is what made the island freeze under load.
static FRAME_PENDING: AtomicBool = AtomicBool::new(false);

/// Points the render clock at the frame rate the preferences chose. `0` (the
/// "V-Sync (Auto)" entry) keeps the default 60 Hz; any other value sets the
/// clock's sleep to that frame's width, so the picker's options take effect.
pub(crate) fn set_frame_interval(fps: f64) {
    let ms = if fps <= 0.0 {
        FRAME_MS
    } else {
        (1000.0 / fps).round().max(1.0) as u32
    };
    FRAME_INTERVAL_MS.store(ms, Ordering::Relaxed);
}

// Raise the system timer resolution so the render clock's 16 ms sleep actually
// wakes every ~16 ms. Without it Windows rounds the tick to its default
// ~15.6 ms and, under load, coalesces it toward 30 ms - which is what made the
// springs judder. Declared directly so it does not depend on which feature the
// `windows` crate happens to group the winmm time functions under.
#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
}

struct Window {
    app: App,
    /// Logical-to-pixel factor of the primary display at launch.
    scale: f32,
}

/// The window's state, parked on the window so the wndproc can find it.
unsafe fn state(hwnd: HWND) -> Option<&'static mut Window> {
    let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Window;
    raw.as_mut()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_APP_FRAME => {
            // Take the frame off the books before drawing, so the clock may
            // post the next one while this frame is still being painted.
            FRAME_PENDING.store(false, Ordering::Release);
            if let Some(window) = state(hwnd) {
                window.app.frame();
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if let Some(window) = state(hwnd) {
                let scale = window.scale.max(0.1);
                let (x, y) = (loword(lp) as f32 / scale, hiword(lp) as f32 / scale);
                window.app.mouse_move(x, y);
                // Re-armed on every move so the leave message always arrives
                // once the pointer has genuinely gone.
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    // Only read when TME_HOVER is set, so the "default" value
                    // stands in for HOVER_DEFAULT.
                    dwHoverTime: u32::MAX,
                };
                let _ = TrackMouseEvent(&mut track);
            }
            LRESULT(0)
        }
        WM_MOUSE_LEAVE => {
            if let Some(window) = state(hwnd) {
                // Nothing else will tell the island where the pointer went, so
                // read its live position the way the script's leave timer did.
                let scale = window.scale.max(0.1);
                let (x, y) = sys::cursor_in_window(hwnd)
                    .map(|(px, py)| (px as f32 / scale, py as f32 / scale))
                    .unwrap_or((-1.0, -1.0));
                window.app.mouse_leave(x, y);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Some(window) = state(hwnd) {
                let scale = window.scale.max(0.1);
                let (x, y) = (loword(lp) as f32 / scale, hiword(lp) as f32 / scale);
                window.app.click(x, y);
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            if let Some(window) = state(hwnd) {
                window.app.collapse();
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            if let Some(window) = state(hwnd) {
                let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
                window.app.wheel(delta);
            }
            LRESULT(0)
        }
        WM_CHAR => {
            if let Some(window) = state(hwnd) {
                if let Some(ch) = char::from_u32(wp.0 as u32) {
                    window.app.typed(ch);
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if let Some(window) = state(hwnd) {
                window.app.key(wp.0 as u32);
            }
            LRESULT(0)
        }
        WM_DROPFILES => {
            let drop = HDROP(wp.0 as *mut core::ffi::c_void);
            if let Some(window) = state(hwnd) {
                window.app.drop_files(dropped(drop));
            }
            DragFinish(drop);
            LRESULT(0)
        }
        prefs::WM_ISLE_SETTINGS => {
            if let Some(window) = state(hwnd) {
                window.app.reload_settings();
            }
            LRESULT(0)
        }
        prefs::WM_ISLE_ALERT => {
            if let Some(window) = state(hwnd) {
                window.app.preview_alert(wp.0);
            }
            LRESULT(0)
        }
        prefs::WM_ISLE_CLEAR => {
            if let Some(window) = state(hwnd) {
                window.app.clear_clips();
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let over = state(hwnd)
                .map(|window| {
                    let (x, y) = window.app.mouse;
                    window.app.body().contains(x, y)
                })
                .unwrap_or(false);
            let cursor = if over { IDC_HAND } else { IDC_ARROW };
            SetCursor(LoadCursorW(None, cursor).unwrap_or_default());
            LRESULT(1)
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut paint);
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        WM_DESTROY => {
            CLOCK_ON.store(false, Ordering::Relaxed);
            timeEndPeriod(1);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn loword(value: LPARAM) -> i32 {
    (value.0 & 0xFFFF) as i16 as i32
}

fn hiword(value: LPARAM) -> i32 {
    ((value.0 >> 16) & 0xFFFF) as i16 as i32
}

/// The paths of a `WM_DROPFILES` message, in the order Explorer handed them.
fn dropped(drop: HDROP) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    unsafe {
        let count = DragQueryFileW(drop, 0xFFFF_FFFF, None);
        for index in 0..count {
            let length = DragQueryFileW(drop, index, None);
            if length == 0 {
                continue;
            }
            let mut buffer = vec![0u16; length as usize + 1];
            let written = DragQueryFileW(drop, index, Some(&mut buffer));
            if written > 0 {
                paths.push(std::path::PathBuf::from(String::from_utf16_lossy(
                    &buffer[..written as usize],
                )));
            }
        }
    }
    paths
}

/// Native faults bypass the panic hook. An access violation or a stack
/// overflow in the unsafe Direct2D/COM code kills the process outright - the
/// window freezes on its last frame and nothing is written, which is exactly
/// what made the click-hang impossible to diagnose. A vectored exception
/// handler sees those faults before the OS tears the process down, so the
/// exception code and the faulting address land in `crash.log` next to any
/// Rust panic. The record is read by raw offset so it does not depend on which
/// shape the `windows` crate happens to publish these fields under.
unsafe extern "system" fn native_crash_filter(pointers: *mut core::ffi::c_void) -> i32 {
    use std::io::Write;
    if !pointers.is_null() {
        // EXCEPTION_POINTERS { ExceptionRecord, ContextRecord }: the record is
        // the first field. EXCEPTION_RECORD lays out as ExceptionCode (i32 @
        // 0), ExceptionFlags (u32 @ 4), ExceptionRecord (ptr @ 8) and
        // ExceptionAddress (ptr @ 16).
        let record = *(pointers as *const *const u8);
        if !record.is_null() {
            let code = *(record as *const i32);
            let address = *(record.add(16) as *const *const core::ffi::c_void);
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(app::crash_log_path())
            {
                let _ = writeln!(
                    file,
                    "native fault: code 0x{:08X} at {:p}",
                    code as u32, address
                );
            }
        }
    }
    // 0 = EXCEPTION_CONTINUE_SEARCH: let the normal termination proceed.
    0
}

fn main() {
    unsafe {
        type Handler = unsafe extern "system" fn(*mut core::ffi::c_void) -> i32;
        windows::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler(
            1,
            std::mem::transmute::<Option<Handler>, _>(Some(
                native_crash_filter as Handler,
            )),
        );
    }
    // The binary aborts on panic, which otherwise vanishes without a trace -
    // the window simply freezes on its last painted frame. Record what panicked
    // and from where so a hang on click can be diagnosed.
    std::panic::set_hook(Box::new(|info|
        {
            use std::io::Write;
            let path = app::crash_log_path();
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = writeln!(file, "{}", info);
            }
        }
    ));
    let mut quit_after: Option<u64> = None;
    let mut open_prefs = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--seconds" {
            quit_after = args.next().and_then(|value| value.parse().ok());
        } else if arg == "--prefs" {
            // A testing aid: the preferences with no gear to click first.
            open_prefs = true;
        }
    }

    unsafe {
        // The WinRT calls (media sessions, notices, the art decoder) need an
        // apartment. MTA, not STA: the media poll blocks on `IAsyncOperation::get`,
        // whose completion is dispatched to the starting context. On an STA that
        // has no message pump running yet at startup, the wait never wakes and the
        // island never shows whenever a media session is live; on the MTA the
        // completion runs on a worker thread and the blocking get returns.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let scale = GetDpiForSystem() as f32 / 96.0;
        let width_px = (HOST_W * scale).round() as i32;
        let height_px = (HOST_H * scale).round() as i32;
        let screen = GetSystemMetrics(SM_CXSCREEN);
        let x = (screen - width_px) / 2;

        let instance = match GetModuleHandleW(None) {
            Ok(module) => HINSTANCE(module.0),
            Err(_) => return,
        };
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: w!("IsleNative"),
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return;
        }

        // No WS_EX_TRANSPARENT: the alpha layer already lets clicks through
        // where nothing is painted, and the island itself still wants them.
        // No WS_EX_NOACTIVATE either, so typing into a field works.
        let ex_style = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
        let hwnd = match CreateWindowExW(
            ex_style,
            w!("IsleNative"),
            w!("Isle"),
            WS_POPUP,
            x,
            0,
            width_px,
            height_px,
            None,
            None,
            instance,
            None,
        ) {
            Ok(hwnd) => hwnd,
            Err(_) => return,
        };

        let renderer = match Renderer::new(width_px, height_px, scale) {
            Ok(renderer) => renderer,
            Err(_) => {
                let _ = DestroyWindow(hwnd);
                return;
            }
        };

        let app = App::new(hwnd, renderer, quit_after);
        let window = Box::new(Window { app, scale });
        let pointer = Box::into_raw(window);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer as isize);

        // Dropping files on the island fills the shelf.
        DragAcceptFiles(hwnd, true);

        timeBeginPeriod(1);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let hwnd_raw = hwnd.0 as isize;
        // A dedicated clock drives the springs: it sleeps at the frame interval
        // (accurate because `timeBeginPeriod` raised the system tick) and posts
        // a private message the loop turns into one `frame` step. This is
        // steadier than `WM_TIMER`, which aliases against the 15.6 ms tick and
        // makes the expand/collapse judder like a low-frame flip.
        std::thread::spawn(move || {
            let hwnd = HWND(hwnd_raw as *mut core::ffi::c_void);
            while CLOCK_ON.load(Ordering::Relaxed) {
                // Post only when the previous frame has been consumed. If a
                // frame costs more than the interval, the skipped posts keep
                // the queue short and input responsive instead of flooding it.
                if !FRAME_PENDING.swap(true, Ordering::AcqRel)
                    && PostMessageW(hwnd, WM_APP_FRAME, WPARAM(0), LPARAM(0)).is_err()
                {
                    FRAME_PENDING.store(false, Ordering::Release);
                }
                let ms = FRAME_INTERVAL_MS.load(Ordering::Relaxed).max(1) as u64;
                std::thread::sleep(Duration::from_millis(ms));
            }
        });

        // The preferences window is made now, hidden, so the gear opens it at
        // once and its first frame is already painted.
        prefs::create(hwnd);
        if open_prefs {
            prefs::show();
        }

        if let Some(window) = pointer.as_mut() {
            window.app.frame();
        }

        // No window filter: WM_QUIT arrives after the window is already gone,
        // and filtering on a dead handle makes GetMessage fail instead of quit.
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(Box::from_raw(pointer));
    }
}
