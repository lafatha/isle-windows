//! The Windows facts the island needs but Direct2D does not: the clipboard,
//! the power state, Caps Lock, and a clock that survives sleep.

use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, SYSTEMTIME};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;

/// Where the pointer is, in this window's client pixels, or `None` when the
/// OS will not say. A pointer that leaves the host window sends it no further
/// moves, so the leave check has to ask the OS rather than trust the last
/// position it was handed - which is what the script's leave timer did with
/// `[System.Windows.Forms.Cursor]::Position`.
pub fn cursor_in_window(hwnd: HWND) -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    unsafe {
        let mut point = POINT::default();
        GetCursorPos(&mut point).ok()?;
        // The point becomes client-relative in place; a pointer outside the
        // window still converts, it simply lands off the client rect.
        let _ = ScreenToClient(hwnd, &mut point);
        Some((point.x, point.y))
    }
}

/// `HH:mm:ss`, for the migration marker the script wrote.
pub fn now_string() -> String {
    let st: SYSTEMTIME = unsafe { GetLocalTime() };
    format!("{:02}:{:02}:{:02}", st.wHour, st.wMinute, st.wSecond)
}

pub fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Clock {
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub day: u16,
    pub month: u16,
    pub year: u16,
    pub weekday: u16,
}

impl Clock {
    pub fn read() -> Self {
        let st: SYSTEMTIME = unsafe { GetLocalTime() };
        Self {
            hour: st.wHour,
            minute: st.wMinute,
            second: st.wSecond,
            day: st.wDay,
            month: st.wMonth,
            year: st.wYear,
            weekday: st.wDayOfWeek,
        }
    }

    pub fn compact(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// The header's "Mon 27 Oct", with the two spaces the script padded it with
    /// so the clock beside it does not jump as the weekday name changes width.
    pub fn header_date(&self) -> String {
        const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let day = DAYS.get(self.weekday as usize).copied().unwrap_or("Mon");
        let month = MONTHS
            .get(self.month.saturating_sub(1) as usize)
            .copied()
            .unwrap_or("Jan");
        format!("{day} {} {month}  ", self.day)
    }

    pub fn header_time(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }
}

/// The clipboard: what was copied last, and how to put something back.
pub struct Clipboard;

impl Clipboard {
    pub fn open() -> bool {
        unsafe { OpenClipboard(HWND::default()).is_ok() }
    }

    pub fn close() {
        unsafe {
            let _ = CloseClipboard();
        }
    }

    /// The text on the clipboard, if there is any.
    pub fn text() -> Option<String> {
        unsafe {
            let handle = GetClipboardData(CF_UNICODETEXT).ok()?;
            if handle.0.is_null() {
                return None;
            }
            let ptr = GlobalLock(HGLOBAL(handle.0)) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let value = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(HGLOBAL(handle.0));
            Some(value)
        }
    }

    /// The files on the clipboard, as Explorer puts them there.
    pub fn files() -> Vec<String> {
        unsafe {
            let Ok(handle) = GetClipboardData(CF_HDROP) else {
                return Vec::new();
            };
            if handle.0.is_null() {
                return Vec::new();
            }
            let drop = HDROP(handle.0);
            let count = DragQueryFileW(drop, u32::MAX, None);
            let mut files = Vec::with_capacity(count as usize);
            for index in 0..count {
                let mut buf = vec![0u16; 512];
                let len = DragQueryFileW(drop, index, Some(&mut buf));
                if len > 0 {
                    files.push(String::from_utf16_lossy(&buf[..len as usize]));
                }
            }
            files
        }
    }

    /// Puts text back, so a shelf entry can be copied with one click.
    pub fn set_text(text: &str) {
        unsafe {
            if OpenClipboard(HWND::default()).is_err() {
                return;
            }
            let _ = EmptyClipboard();
            let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = units.len() * 2;
            if let Ok(handle) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
                let ptr = GlobalLock(handle) as *mut u16;
                if !ptr.is_null() {
                    std::ptr::copy_nonoverlapping(units.as_ptr(), ptr, units.len());
                    let _ = GlobalUnlock(handle);
                    let _ = SetClipboardData(CF_UNICODETEXT, HANDLE(handle.0));
                }
            }
            let _ = CloseClipboard();
        }
    }
}

/// AC/Battery and the charge level, for the charging alert.
#[derive(Clone, Copy, Debug, Default)]
pub struct Power {
    pub percent: u8,
    pub plugged: bool,
}

impl Power {
    pub fn read() -> Self {
        let mut status = SYSTEM_POWER_STATUS::default();
        unsafe {
            if GetSystemPowerStatus(&mut status).is_err() {
                return Self::default();
            }
        }
        Self {
            percent: status.BatteryLifePercent,
            plugged: status.ACLineStatus == 1,
        }
    }
}

pub fn caps_lock_on() -> bool {
    unsafe { GetKeyState(VK_CAPITAL.0 as i32) & 1 == 1 }
}

/// Startup folder and home, for the provider scans.
pub fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE").map(std::path::PathBuf::from)
}

/// Days since 1970-01-01 for a civil date, the usual way round.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parses the `2025-10-26T12:34:56.789Z` stamps the Antigravity transcripts use.
/// Local stamps without a zone are read as UTC, which is what the script did.
pub fn parse_iso8601(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let number =
        |from: usize, len: usize| -> Option<i64> { value.get(from..from + len)?.parse().ok() };
    let year = number(0, 4)?;
    let month = number(5, 2)?;
    let day = number(8, 2)?;
    let hour = number(11, 2)?;
    let minute = number(14, 2)?;
    let second = number(17, 2)?;
    let millis = if bytes.len() > 20 && bytes[19] == b'.' {
        number(20, 3).unwrap_or(0)
    } else {
        0
    };
    let days = days_from_civil(year, month, day);
    Some(days * 86_400_000 + hour * 3_600_000 + minute * 60_000 + second * 1000 + millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats_like_the_script() {
        let clock = Clock {
            hour: 9,
            minute: 5,
            second: 0,
            day: 27,
            month: 10,
            year: 2026,
            weekday: 2,
        };
        assert_eq!(clock.compact(), "09:05");
        assert_eq!(clock.header_time(), "09:05");
        assert_eq!(clock.header_date(), "Tue 27 Oct  ");
    }

    #[test]
    fn iso_stamps_from_the_transcripts_parse() {
        let stamp = parse_iso8601("2026-10-27T09:05:00.000Z").unwrap();
        assert_eq!(stamp, 1_793_091_900_000);
        assert_eq!(parse_iso8601("nonsense"), None);
    }

    #[test]
    fn a_missing_zone_is_read_as_utc() {
        let with = parse_iso8601("2026-01-02T03:04:05Z").unwrap();
        let without = parse_iso8601("2026-01-02T03:04:05").unwrap();
        assert_eq!(with, without);
    }
}
