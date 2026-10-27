//! Now playing, and how loud it is.
//!
//! The session comes from the same Windows API the script build polled
//! (`GlobalSystemMediaTransportControlsSessionManager`); the level comes from
//! the default endpoint's peak meter, read through its COM interface.
//!
//! Every WinRT and COM call here blocks, so none of them run on the render
//! thread: a worker thread owns the session manager, the session and the meter,
//! polls them on its own clock and publishes a plain-data snapshot. The
//! interface copies the newest snapshot without ever waiting, exactly the way
//! the lyric fetch does.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::core::{Interface, GUID};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};

/// What the island knows about the track on screen.
#[derive(Clone, Default)]
pub struct Now {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub source: String,
    /// The raw application id the source was named from, kept so the icon
    /// lookup can match it against a live window.
    pub source_aumid: String,
    pub playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub can_seek: bool,
}

impl Now {
    pub fn has_track(&self) -> bool {
        !self.title.is_empty()
    }
}

/// The label the music page prints under the title: the app's friendly name,
/// derived the way the script's alias table did.
pub fn source_label(aumid: &str) -> String {
    let lower = aumid.to_lowercase();
    let known = [
        ("spotify", "Spotify"),
        ("zunemusic", "Media Player"),
        ("mediaplayer", "Media Player"),
        ("applemusic", "Apple Music"),
        ("itunes", "Apple Music"),
        ("youtube", "YouTube"),
        ("chrome", "Chrome"),
        ("msedge", "Edge"),
        ("firefox", "Firefox"),
        ("brave", "Brave"),
        ("vlc", "VLC"),
        ("foobar", "foobar2000"),
        ("tidal", "TIDAL"),
        ("deezer", "Deezer"),
        ("soundcloud", "SoundCloud"),
        ("bandcamp", "Bandcamp"),
    ];
    for (needle, label) in known {
        if lower.contains(needle) {
            return label.to_string();
        }
    }
    // A packaged id is "Publisher.App_hash!Entry": the app is the middle part.
    let tail = aumid.rsplit('!').next().unwrap_or(aumid);
    let stem = if tail.contains('.') {
        aumid.split('!').next().unwrap_or(aumid)
    } else {
        tail
    };
    let stem = stem.rsplit('.').next().unwrap_or(stem);
    let stem = stem.split('_').next().unwrap_or(stem);
    if stem.is_empty() {
        "Media".to_string()
    } else {
        let mut chars = stem.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => "Media".to_string(),
        }
    }
}

/// Milliseconds between two whole seconds, the way the scripts did it.
pub fn duration_ms(value: windows::Foundation::TimeSpan) -> u64 {
    (value.Duration.max(0) / 10_000) as u64
}

/// A transport request handed to the polling thread so the click never blocks.
enum Cmd {
    Toggle,
    Next,
    Prev,
    Seek(i64),
}

/// One reading the worker publishes for the interface to draw. Plain data
/// only, so it can cross the channel; the COM objects stay on the worker.
struct Snapshot {
    now: Now,
    sound: bool,
    peak: f32,
    art: Option<Vec<u8>>,
    art_key: String,
    /// The source app's icon, premultiplied BGRA, with the id it belongs to.
    icon: Option<(u32, u32, Vec<u8>)>,
    icon_key: String,
    /// When the timeline position was sampled, so the scrubber can creep
    /// forward between readings without another round-trip.
    sampled: Instant,
}

pub struct Media {
    pub now: Now,
    /// Whether the equaliser should be moving: a playing session, or sound the
    /// peak meter can hear.
    pub sound: bool,
    pub peak: f32,
    pub art: Option<Vec<u8>>,
    art_key: String,
    /// The logo of the app making sound, decoded from the worker's bytes.
    pub icon: Option<(u32, u32, Vec<u8>)>,
    icon_key: String,
    sampled: Instant,
    cmd_tx: mpsc::Sender<Cmd>,
    snap_rx: mpsc::Receiver<Snapshot>,
    // The worker runs for the life of the process; the handle is held only to
    // name that ownership (dropping it would simply detach the thread).
    _worker: std::thread::JoinHandle<()>,
}

impl Media {
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (snap_tx, snap_rx) = mpsc::channel();
        let _worker = std::thread::spawn(move || worker(cmd_rx, snap_tx));
        Self {
            now: Now::default(),
            sound: false,
            peak: 0.0,
            art: None,
            art_key: String::new(),
            icon: None,
            icon_key: String::new(),
            sampled: Instant::now(),
            cmd_tx,
            snap_rx,
            _worker,
        }
    }

    /// Takes the newest reading the worker published. Never blocks: the WinRT
    /// round-trips happen on the worker thread. `force` is kept so the call
    /// sites that wanted an immediate refresh read the same as before.
    pub fn poll(&mut self, _force: bool) {
        while let Ok(snap) = self.snap_rx.try_recv() {
            self.now = snap.now;
            self.sound = snap.sound;
            self.peak = snap.peak;
            if snap.art_key != self.art_key {
                self.art_key = snap.art_key;
                self.art = snap.art;
            }
            if snap.icon_key != self.icon_key {
                self.icon_key = snap.icon_key;
                self.icon = snap.icon;
            }
            self.sampled = snap.sampled;
        }
    }

    /// Advances the displayed position between readings, so the scrubber creeps
    /// forward instead of jumping once a second.
    pub fn position_ms(&self) -> u64 {
        if !self.now.playing || !self.now.can_seek {
            return self.now.position_ms;
        }
        let drift = self.sampled.elapsed().as_millis() as u64;
        (self.now.position_ms + drift).min(self.now.duration_ms)
    }

    pub fn toggle(&self) {
        let _ = self.cmd_tx.send(Cmd::Toggle);
    }

    pub fn next(&self) {
        let _ = self.cmd_tx.send(Cmd::Next);
    }

    pub fn previous(&self) {
        let _ = self.cmd_tx.send(Cmd::Prev);
    }

    pub fn seek(&mut self, ratio: f32) {
        let ratio = ratio.clamp(0.0, 1.0);
        if !self.now.can_seek {
            return;
        }
        let target = (self.now.duration_ms as f32 * ratio) as i64 * 10_000;
        let _ = self.cmd_tx.send(Cmd::Seek(target));
        // Show the jump at once; the worker confirms it on the next reading.
        self.now.position_ms = (self.now.duration_ms as f32 * ratio) as u64;
        self.sampled = Instant::now();
    }

    /// Sound is decided on the worker; this only refreshes the cached copy so
    /// the existing `poll` then `poll_sound` call sites keep working.
    pub fn poll_sound(&mut self) {
        self.poll(false);
    }
}

/// The polling thread. It owns every WinRT and COM object, so the blocking
/// `.get()` waits happen here and never touch the render loop.
fn worker(cmd_rx: mpsc::Receiver<Cmd>, tx: mpsc::Sender<Snapshot>) {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let mut manager: Option<GlobalSystemMediaTransportControlsSessionManager> = None;
    let mut session: Option<GlobalSystemMediaTransportControlsSession> = None;
    let mut meter: Option<Meter> = None;
    let mut now = Now::default();
    let mut art: Option<Vec<u8>> = None;
    let mut art_key = String::new();
    let mut icon: Option<(u32, u32, Vec<u8>)> = None;
    let mut icon_key = String::new();
    let mut sound = false;
    let mut peak = 0.0f32;
    let mut quiet = 0u32;
    let mut last_props = Instant::now() - Duration::from_secs(10);
    let mut last_icon_try = Instant::now() - Duration::from_secs(10);
    // A fallback clock for players that never report a position (browsers above
    // all): the last position we did know, and the instant we knew it.
    let mut pos_base: u64 = 0;
    let mut pos_at = Instant::now();
    // The previous reading, so we only re-anchor when the player's position
    // actually moves rather than freezing on a value it keeps repeating.
    let mut last_provider: u64 = 0;

    loop {
        // Act on transport requests first, so a click feels immediate.
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Cmd::Toggle => match session.as_ref() {
                    Some(s) if s.TryTogglePlayPauseAsync().and_then(|o| o.get()).is_ok() => {}
                    _ => crate::keys::media_key(0xCD),
                },
                Cmd::Next => match session.as_ref() {
                    Some(s) if s.TrySkipNextAsync().and_then(|o| o.get()).is_ok() => {}
                    _ => crate::keys::media_key(0xB0),
                },
                Cmd::Prev => match session.as_ref() {
                    Some(s) if s.TrySkipPreviousAsync().and_then(|o| o.get()).is_ok() => {}
                    _ => crate::keys::media_key(0xB1),
                },
                Cmd::Seek(target) => {
                    if let Some(s) = session.as_ref() {
                        let _ = s
                            .TryChangePlaybackPositionAsync(target)
                            .and_then(|o| o.get());
                    }
                    // Re-anchor the fallback clock to where we just jumped, so
                    // a silent player resumes counting from the new spot.
                    pos_base = (target.max(0) / 10_000) as u64;
                    pos_at = Instant::now();
                }
            }
        }

        if manager.is_none() {
            manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
                .and_then(|o| o.get())
                .ok();
        }

        if let Some(m) = manager.as_ref() {
            session = m.GetCurrentSession().ok();
            if let Some(s) = session.as_ref() {
                if let Ok(info) = s.GetPlaybackInfo() {
                    if let Ok(status) = info.PlaybackStatus() {
                        now.playing = status == Status::Playing;
                    }
                }
                if let Ok(timeline) = s.GetTimelineProperties() {
                    let start = timeline.StartTime().map(duration_ms).unwrap_or(0);
                    let end = timeline.EndTime().map(duration_ms).unwrap_or(0);
                    now.duration_ms = end.saturating_sub(start);
                    now.can_seek = now.duration_ms > 0;
                    // Trust the player's own position whenever it reports a
                    // non-zero one. Some sources leave it at zero, so then we
                    // run our own clock from the last point we did know,
                    // advancing only while it is actually playing.
                    let provider = timeline.Position().map(duration_ms).unwrap_or(0);
                    // Re-anchor only when the reading actually moves. A player
                    // that keeps reporting the same position while playing is
                    // not updating it, so trusting it every poll would freeze
                    // the clock there; letting it run advances the seconds.
                    if provider > 0 && provider != last_provider {
                        pos_base = provider;
                        pos_at = Instant::now();
                    }
                    last_provider = provider;
                    now.position_ms = if now.playing {
                        (pos_base + pos_at.elapsed().as_millis() as u64).min(now.duration_ms)
                    } else {
                        pos_base.min(now.duration_ms)
                    };
                }
                if let Ok(app) = s.SourceAppUserModelId() {
                    let aumid = app.to_string();
                    now.source = source_label(&aumid);
                    now.source_aumid = aumid.clone();
                    // The icon is the expensive find - a window walk and a
                    // shell call - so it runs when the source changes, and
                    // retries (throttled) while a lookup has yet to succeed:
                    // the player's window may not have been up first time.
                    if aumid != icon_key
                        || (icon.is_none()
                            && last_icon_try.elapsed() >= Duration::from_secs(2))
                    {
                        icon_key = aumid.clone();
                        last_icon_try = Instant::now();
                        icon = crate::icon::resolve(&icon_key);
                    }
                }
                // The metadata call is the slow one; it runs here, off the
                // render thread, once a second.
                if last_props.elapsed() >= Duration::from_secs(1) {
                    last_props = Instant::now();
                    if let Ok(props) = s.TryGetMediaPropertiesAsync().and_then(|o| o.get()) {
                        now.title = props.Title().map(|v| v.to_string()).unwrap_or_default();
                        now.artist = props.Artist().map(|v| v.to_string()).unwrap_or_default();
                        now.album = props.AlbumTitle().map(|v| v.to_string()).unwrap_or_default();
                        let key = format!("{}|{}", now.title, now.artist);
                        if key != art_key {
                            art_key = key;
                            // A new track: start the fallback clock from the top,
                            // and clear the last reading so the next poll re-snaps
                            // to wherever the player really is. Without that, a
                            // source that reports a stuck position (so it never
                            // "changes") would leave us counting up from zero when
                            // the app opens mid-song.
                            pos_base = 0;
                            pos_at = Instant::now();
                            last_provider = 0;
                            art = props
                                .Thumbnail()
                                .ok()
                                .and_then(|thumb| read_thumbnail(&thumb));
                        }
                    }
                }
            } else {
                now = Now::default();
                art = None;
                art_key.clear();
                icon = None;
                icon_key.clear();
            }
        }

        if meter.is_none() {
            meter = Meter::create();
        }
        if let Some(m) = meter.as_mut() {
            match m.peak() {
                Some(value) => peak = value,
                None => {
                    meter = None;
                    peak = 0.0;
                }
            }
        }
        // The equaliser's switch: a playing session or an audible peak turns it
        // on at once, and it takes three quiet readings to stop, so a short
        // pause does not flatten the bars.
        if now.playing || peak > 0.005 {
            quiet = 0;
            sound = true;
        } else {
            quiet += 1;
            if quiet >= 3 {
                sound = false;
            }
        }

        let _ = tx.send(Snapshot {
            now: now.clone(),
            sound,
            peak,
            art: art.clone(),
            art_key: art_key.clone(),
            icon: icon.clone(),
            icon_key: icon_key.clone(),
            sampled: Instant::now(),
        });
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Reads the whole thumbnail into memory; the decoder runs later, once.
fn read_thumbnail(
    reference: &windows::Storage::Streams::IRandomAccessStreamReference,
) -> Option<Vec<u8>> {
    use windows::Storage::Streams::{DataReader, IRandomAccessStreamWithContentType};
    let stream: IRandomAccessStreamWithContentType = reference.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()?;
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size as u32).ok()?.get().ok()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(bytes)
}

/// `IAudioMeterInformation` on the default playback device, called through the
/// raw vtable: the windows crate ships the enumerator's class id but not these
/// two interfaces, and they have not changed shape since Vista.
struct Meter {
    information: *mut core::ffi::c_void,
}

const IID_IAUDIO_METER_INFORMATION: GUID = GUID::from_u128(0xc02216f6_8c67_4b5b_9d00_d008e73e0064);
const IID_IAUDIO_ENDPOINT_VOLUME: GUID = GUID::from_u128(0x5cdf2c82_841e_4546_9722_0cf74078229a);

type Release = unsafe extern "system" fn(*mut core::ffi::c_void) -> u32;
type GetDefaultAudioEndpoint =
    unsafe extern "system" fn(*mut core::ffi::c_void, i32, i32, *mut *mut core::ffi::c_void) -> i32;
type Activate = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    *const GUID,
    u32,
    *const core::ffi::c_void,
    *mut *mut core::ffi::c_void,
) -> i32;
type GetPeakValue = unsafe extern "system" fn(*mut core::ffi::c_void, *mut f32) -> i32;
type SetMasterVolumeLevelScalar =
    unsafe extern "system" fn(*mut core::ffi::c_void, f32, *const GUID) -> i32;

unsafe fn vtable(object: *mut core::ffi::c_void) -> *const *const core::ffi::c_void {
    *(object as *const *const *const core::ffi::c_void)
}

/// Activates one interface on the default playback device.
unsafe fn open_interface(iid: &GUID) -> Option<*mut core::ffi::c_void> {
    use windows::Win32::Media::Audio::MMDeviceEnumerator;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    let enumerator: windows::core::IUnknown =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let enumerator = enumerator.as_raw();

    // IMMDeviceEnumerator::GetDefaultAudioEndpoint: slot 4, eRender/eConsole.
    let get_endpoint: GetDefaultAudioEndpoint = std::mem::transmute(*vtable(enumerator).add(4));
    let mut device: *mut core::ffi::c_void = std::ptr::null_mut();
    if get_endpoint(enumerator, 0, 0, &mut device) < 0 || device.is_null() {
        release(enumerator);
        return None;
    }

    // IMMDevice::Activate: slot 3.
    let activate: Activate = std::mem::transmute(*vtable(device).add(3));
    let mut interface: *mut core::ffi::c_void = std::ptr::null_mut();
    let hr = activate(device, iid, CLSCTX_ALL.0, std::ptr::null(), &mut interface);
    release(device);
    release(enumerator);
    if hr < 0 || interface.is_null() {
        return None;
    }
    Some(interface)
}

/// Sets the default output's master volume to a share of full; false when
/// Windows refuses, which is what the script said back to the user.
pub fn set_volume(scalar: f32) -> bool {
    unsafe {
        let Some(volume) = open_interface(&IID_IAUDIO_ENDPOINT_VOLUME) else {
            return false;
        };
        // IAudioEndpointVolume::SetMasterVolumeLevelScalar: slot 7.
        let set: SetMasterVolumeLevelScalar = std::mem::transmute(*vtable(volume).add(7));
        let hr = set(volume, scalar.clamp(0.0, 1.0), std::ptr::null());
        release(volume);
        hr >= 0
    }
}

impl Meter {
    fn create() -> Option<Self> {
        unsafe {
            open_interface(&IID_IAUDIO_METER_INFORMATION).map(|information| Self { information })
        }
    }

    fn peak(&mut self) -> Option<f32> {
        unsafe {
            // IAudioMeterInformation::GetPeakValue: slot 3.
            let get_peak: GetPeakValue = std::mem::transmute(*vtable(self.information).add(3));
            let mut peak = 0.0f32;
            if get_peak(self.information, &mut peak) < 0 {
                None
            } else {
                Some(peak.clamp(0.0, 1.0))
            }
        }
    }
}

unsafe fn release(object: *mut core::ffi::c_void) {
    if object.is_null() {
        return;
    }
    let release_fn: Release = std::mem::transmute(*vtable(object).add(2));
    release_fn(object);
}

impl Drop for Meter {
    fn drop(&mut self) {
        unsafe { release(self.information) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_labels_follow_the_alias_table() {
        assert_eq!(
            source_label("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"),
            "Spotify"
        );
        assert_eq!(
            source_label("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"),
            "Media Player"
        );
        assert_eq!(source_label("chrome.exe"), "Chrome");
        assert_eq!(source_label("VLC media player"), "VLC");
        assert_eq!(source_label(""), "Media");
    }

    #[test]
    fn source_label_falls_back_to_the_app_part() {
        assert_eq!(
            source_label("Comet.JVPSDCZDP5BUKVQI65TBATIXMI"),
            "JVPSDCZDP5BUKVQI65TBATIXMI"
        );
    }

    #[test]
    fn time_spans_convert_to_milliseconds() {
        let span = windows::Foundation::TimeSpan {
            Duration: 12_345_000,
        };
        assert_eq!(duration_ms(span), 1234);
        let negative = windows::Foundation::TimeSpan { Duration: -5 };
        assert_eq!(duration_ms(negative), 0);
    }
}
