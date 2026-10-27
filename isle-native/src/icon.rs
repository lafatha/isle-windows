//! The logo of whichever app is making sound.
//!
//! The closed island shows the assistant, but the script swapped in the source
//! application's own icon while something was playing. A media session only
//! reports an application id, so the icon is found the way the taskbar finds
//! it: match that id to a live window, read the process image behind it, and
//! ask the shell for that file's icon - then paint the icon into a 32-bit DIB
//! so Direct2D gets premultiplied BGRA it can use directly.
//!
//! Everything here runs on the media worker thread, which already has COM
//! initialised. Nothing is allowed to panic, because the binary aborts.

use std::ffi::c_void;

use windows::core::{GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::{BOOL, CloseHandle, HWND, LPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DeleteDC,
    DeleteObject, DIB_RGB_COLORS, SelectObject,
};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Shell::{
    SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON,
};
use windows::Win32::UI::Shell::PropertiesSystem::{
    IPropertyStore, PROPERTYKEY, SHGetPropertyStoreForWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, DrawIconEx, EnumWindows, GetWindowTextLengthW, GetWindowThreadProcessId,
    IsWindowVisible, DI_FLAGS, HICON,
};

/// The icon edge, in pixels: the large shell icon, drawn into a square buffer.
const SIZE: i32 = 32;

/// `PKEY_AppUserModel_ID`: the window's own application id, the same string the
/// media session reports as its source. The struct is `const`-constructible, so
/// the well-known guid is written out rather than pulled from a heavier module.
fn app_user_model_id() -> PROPERTYKEY {
    PROPERTYKEY {
        fmtid: GUID::from_u128(0x9F4C_2855_9F79_4B39_8D43_6B72D6C9A6F5),
        pid: 5,
    }
}

/// The windows the enumeration is hunting for, written back through the
/// callback's `LPARAM`. `found` is a window carrying the very id the session
/// reported; `named` is a looser second choice - a window whose executable the
/// id points at, for browsers that stamp a per-window id unlike the session's.
/// Both stay null until a match turns up.
struct Hunt {
    target: String,
    found: HWND,
    named: HWND,
}

/// The leading dotted segment of an id: `Chrome` from `Chrome.LNK-...`, the
/// part that survives whatever suffix an installer or browser hangs off it.
fn base(s: &str) -> String {
    s.split('.').next().unwrap_or(s).to_string()
}

/// Does a window's application id name the same app as the session's, allowing
/// for the suffix a browser or installer adds to the base id.
fn id_matches(value: &str, target: &str) -> bool {
    let value = value.trim().to_lowercase();
    if value.is_empty() {
        return false;
    }
    if value == target {
        return true;
    }
    let tb = base(target);
    !tb.is_empty() && base(&value) == tb
}

/// Does the executable behind a window correspond to the id - its file stem
/// appears in the id, or the id's base appears in the stem?
fn exe_matches(path: &[u16], target: &str) -> bool {
    let name = String::from_utf16_lossy(path);
    let file = name.rsplit(['\\', '/']).next().unwrap_or("");
    let stem: String = file.chars().take_while(|c| *c != '.').collect();
    let stem = stem.to_lowercase();
    if stem.is_empty() {
        return false;
    }
    let tb = base(target);
    target.contains(&stem) || (!tb.is_empty() && stem.contains(&tb))
}

/// `EnumWindows` callback: compare each titled, visible window's application id
/// with the session's, and stop on the first match.
unsafe extern "system" fn examine(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let hunt = &mut *(lparam.0 as *mut Hunt);
    // Already found: stop the walk.
    if !hunt.found.0.is_null() {
        return BOOL(0);
    }
    // A tool window or a window with no caption is not the app's main frame.
    if IsWindowVisible(hwnd).0 == 0 || GetWindowTextLengthW(hwnd) <= 0 {
        return BOOL(1);
    }
    if let Ok(store) = SHGetPropertyStoreForWindow::<HWND, IPropertyStore>(hwnd) {
        let key = app_user_model_id();
        if let Ok(value) = unsafe { store.GetValue(&key) } {
            if id_matches(&value.to_string(), &hunt.target) {
                hunt.found = hwnd;
                return BOOL(0);
            }
        }
    }
    // No id match yet: settle for the first window whose process the id names.
    if hunt.named.0.is_null() {
        if let Some(path) = window_exe(hwnd) {
            if exe_matches(&path, &hunt.target) {
                hunt.named = hwnd;
            }
        }
    }
    BOOL(1)
}

/// The executable path of `hwnd`'s process, as wide characters with no
/// terminator; `None` when the process cannot be opened or named.
fn window_exe(hwnd: HWND) -> Option<Vec<u16>> {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = [0u16; 1024];
    let mut size = buffer.len() as u32;
    let named = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
    };
    unsafe {
        let _ = CloseHandle(handle);
    }
    if named.is_ok() && size > 0 {
        Some(buffer[..size as usize].to_vec())
    } else {
        None
    }
}

/// Turns an icon handle into premultiplied BGRA pixels: paint it into a
/// top-down 32-bit DIB, then fold the alpha channel into the colours the way
/// Direct2D wants them.
fn icon_pixels(icon: HICON) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIZE,
                // Negative height makes the buffer top-down, so the rows reach
                // Direct2D already in the order it expects.
                biHeight: -SIZE,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(dc);
            return None;
        };
        if bits.is_null() {
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(dc);
            return None;
        }
        let previous = SelectObject(dc, bitmap);
        // `DI_NORMAL` (3) paints both the colour and the mask; the DIB starts
        // zeroed, so whatever the icon leaves transparent stays transparent.
        let _ = DrawIconEx(dc, 0, 0, icon, SIZE, SIZE, 0, windows::Win32::Graphics::Gdi::HBRUSH::default(), DI_FLAGS(3));
        let count = (SIZE * SIZE * 4) as usize;
        let mut pixels = std::slice::from_raw_parts(bits as *const u8, count).to_vec();
        SelectObject(dc, previous);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(dc);
        for px in pixels.chunks_exact_mut(4) {
            let alpha = px[3] as u32;
            if alpha != 255 {
                px[0] = (px[0] as u32 * alpha / 255) as u8;
                px[1] = (px[1] as u32 * alpha / 255) as u8;
                px[2] = (px[2] as u32 * alpha / 255) as u8;
            }
        }
        Some((SIZE as u32, SIZE as u32, pixels))
    }
}

/// The shell's icon for a file path, as pixels. `path` is wide and unterminated.
fn icon_for_path(path: &[u16]) -> Option<(u32, u32, Vec<u8>)> {
    let mut wide: Vec<u16> = path.to_vec();
    wide.push(0);
    let mut info = SHFILEINFOW::default();
    let flags = SHGFI_ICON | SHGFI_LARGEICON;
    let ok = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    if ok == 0 || info.hIcon.0.is_null() {
        return None;
    }
    let pixels = icon_pixels(info.hIcon);
    unsafe {
        let _ = DestroyIcon(info.hIcon);
    }
    pixels
}

/// The icon of the app behind the media session, identified by its application
/// id. Returns premultiplied BGRA pixels, or `None` for an id that leads
/// nowhere - which simply leaves the assistant in place.
pub fn resolve(aumid: &str) -> Option<(u32, u32, Vec<u8>)> {
    let target = aumid.trim().to_lowercase();
    if target.is_empty() {
        return None;
    }
    let mut hunt = Hunt {
        target,
        found: HWND::default(),
        named: HWND::default(),
    };
    unsafe {
        let _ = EnumWindows(
            Some(examine),
            LPARAM(&mut hunt as *mut Hunt as isize),
        );
    }
    let hwnd = if !hunt.found.0.is_null() {
        hunt.found
    } else {
        hunt.named
    };
    if !hwnd.0.is_null() {
        if let Some(path) = window_exe(hwnd) {
            let icon = icon_for_path(&path);
            if icon.is_some() {
                return icon;
            }
        }
    }
    // No window carried the id (a background or windowless player): if the id
    // is itself a file path, ask the shell about that file directly.
    let lower = aumid.to_lowercase();
    if lower.contains(".exe") || lower.contains('\\') || lower.contains('/') {
        let path: Vec<u16> = aumid.encode_utf16().collect();
        return icon_for_path(&path);
    }
    None
}
