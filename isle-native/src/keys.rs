//! The two things the island does to Windows at arm's length: press a media
//! key, and make the sound that says a focus phase has ended.

use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYBD_EVENT_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK;

const KEYEVENTF_KEYUP: u32 = 0x0002;

/// A media key pressed and released, the way `IsleMediaHook` did it.
pub fn media_key(vk: u8) {
    unsafe {
        keybd_event(vk, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(vk, 0, KEYBD_EVENT_FLAGS(KEYEVENTF_KEYUP), 0);
    }
}

pub fn beep() {
    unsafe {
        let _ = MessageBeep(MB_ICONASTERISK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK,
    };

    #[test]
    fn the_media_key_codes_are_the_ones_the_script_sent() {
        // Play/pause, next, previous: VK_MEDIA_* from the Win32 headers.
        assert_eq!(VK_MEDIA_PLAY_PAUSE.0, 0xB3);
        assert_eq!(VK_MEDIA_NEXT_TRACK.0, 0xB0);
        assert_eq!(VK_MEDIA_PREV_TRACK.0, 0xB1);
        assert_eq!(KEYEVENTF_KEYUP, 0x0002);
    }
}
