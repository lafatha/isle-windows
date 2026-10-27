//! The island's state: which page is up, how far the springs have moved, what
//! is focusable, and what a tick does every quarter of a second.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;

use crate::chat;
use crate::keys;
use crate::lyrics::Lyrics;
use crate::media::{self, Media};
use crate::pages;
use crate::render::Renderer;
use crate::settings::Settings;
use crate::spring::Animator;
use crate::store::{CalEvent, Reminder, Store};
use crate::sys::{self, Clock};
use crate::ui::{Action, Page, Rect};
use crate::usage::Usage;

pub const HOST_W: f32 = 800.0;
pub const HOST_H: f32 = 290.0;

/// Where the panic hook writes, so an abort on click leaves a trace instead of
/// only a frozen window.
pub fn crash_log_path() -> std::path::PathBuf {
    crate::settings::config_dir().join("crash.log")
}

/// The sizes the script's animator was started with.
pub const COMPACT_W: f32 = 232.0;
pub const EXPANDED_W: f32 = 600.0;
pub const LYRICS_W: f32 = 620.0;
/// Expanded height by page; the conversation makes room for its input row and,
/// when the tab row is open, for the tab row as well.
pub const PAGE_H: f32 = 204.0;
pub const VOICE_H: f32 = 240.0;
pub const VOICE_MENU_H: f32 = 275.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertKind {
    /// A 232 px row: a key lock changed, or a focus phase ended.
    Mini,
    /// The 320 px card: the charger went in.
    Charging,
    /// The 380 px card: an event is about to start, with its Join button.
    Meeting,
    /// The 380 px card: a notification, with its Reply button.
    Message,
    /// The 440 by 96 px card: a coding agent asking to run something.
    Permission,
}

pub struct Alert {
    pub kind: AlertKind,
    pub until: Instant,
}

/// Which text box has the keyboard, and what has been typed into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Reminder,
    Calendar,
    Chat,
    /// The clipboard's search box, which narrows the shelf as it is typed.
    Search,
}

pub struct Input {
    pub field: Option<Field>,
    pub text: String,
}

pub struct Focus {
    pub running: bool,
    pub work: bool,
    pub ends: Option<Instant>,
    pub left_ms: i64,
}

impl Focus {
    pub fn new(work_min: u32) -> Self {
        Self {
            running: false,
            work: true,
            ends: None,
            left_ms: work_min as i64 * 60_000,
        }
    }

    pub fn remaining_ms(&self) -> i64 {
        match (self.running, self.ends) {
            (true, Some(ends)) => {
                (ends.saturating_duration_since(Instant::now())).as_millis() as i64
            }
            _ => self.left_ms,
        }
    }

    pub fn clock_text(&self) -> String {
        let ms = self.remaining_ms().max(0);
        let total = (ms + 999) / 1000;
        format!("{:02}:{:02}", total / 60, total % 60)
    }

    pub fn toggle(&mut self, work_min: u32, break_min: u32) {
        if self.running {
            self.left_ms = self.remaining_ms();
            self.running = false;
            self.ends = None;
            return;
        }
        if self.left_ms <= 0 {
            self.work = !self.work;
            self.left_ms = if self.work {
                work_min as i64 * 60_000
            } else {
                break_min as i64 * 60_000
            };
        }
        self.running = true;
        self.ends = Some(Instant::now() + Duration::from_millis(self.left_ms as u64));
    }

    pub fn reset(&mut self, work_min: u32) {
        self.running = false;
        self.ends = None;
        self.work = true;
        self.left_ms = work_min as i64 * 60_000;
    }

    /// Returns true when the phase just ended, so the caller can chime.
    pub fn tick(&mut self, work_min: u32, break_min: u32) -> bool {
        if !self.running {
            return false;
        }
        if self.remaining_ms() > 0 {
            return false;
        }
        self.work = !self.work;
        self.left_ms = if self.work {
            work_min as i64 * 60_000
        } else {
            break_min as i64 * 60_000
        };
        self.ends = Some(Instant::now() + Duration::from_millis(self.left_ms as u64));
        true
    }
}

#[derive(Clone)]
pub struct Message {
    pub from_user: bool,
    pub text: String,
    /// A command the user has to approve before it runs.
    pub card: Option<Card>,
}

/// A command card: the line, and how far it has got.
#[derive(Clone)]
pub struct Card {
    pub line: String,
    pub state: CardState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CardState {
    /// Waiting for Run or Deny.
    Pending,
    /// Approved; running since this instant.
    Running(Instant),
    /// The user chose to leave it alone.
    Denied,
    /// Finished, either way, with the code it exited on.
    Done { ok: bool, code: i32 },
}

pub struct Voice {
    pub messages: Vec<Message>,
    pub rx: Option<Receiver<String>>,
    /// The card that is running: which message it belongs to, and the answer
    /// when it finishes.
    pub run: Option<(usize, Receiver<crate::chat::Outcome>)>,
    pub label: String,
    pub thinking_at: Option<Instant>,
    pub dots: u8,
}

/// The mascot's eyes: a random glance every 2.4 s, with a blink on some of them.
pub struct Blink {
    next: Instant,
    pub glance: u8,
    blink_until: Option<Instant>,
}

impl Blink {
    pub fn new() -> Self {
        Self {
            next: Instant::now() + Duration::from_millis(2400),
            glance: 0,
            blink_until: None,
        }
    }

    /// Returns true when something changed and the island needs redrawing.
    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
        let mut changed = false;
        if let Some(until) = self.blink_until {
            if now >= until {
                self.blink_until = None;
                changed = true;
            }
        }
        if now >= self.next {
            self.next = now + Duration::from_millis(2400);
            // The script's eye timer rolled 0..4: centre, right, left,
            // attentive centre, curious up.
            self.glance = (now.elapsed().subsec_nanos() % 5) as u8;
            // A blink most of the time, so the face stays alive. The script's
            // animation closed for 75 ms and opened for 75 ms.
            self.blink_until = Some(now + Duration::from_millis(150));
            changed = true;
        }
        changed
    }

    /// The eye height in the compact island's pixels and the current glance:
    /// 0/3 centre, 1 right, 2 left, 4 up. While a blink runs, the height rides
    /// the script's close-and-open ramp down to 1.0 and back to 4.8.
    pub fn eyes(&self) -> (f32, u8) {
        let height = match self.blink_until {
            Some(until) => {
                let elapsed = 0.150 - until.duration_since(Instant::now()).as_secs_f32().max(0.0);
                if elapsed < 0.075 {
                    4.8 - 3.8 * elapsed / 0.075
                } else {
                    1.0 + 3.8 * (elapsed - 0.075) / 0.075
                }
            }
            None => 4.8,
        };
        (height, self.glance)
    }
}

/// How far each scrollable list has been scrolled.
#[derive(Default)]
pub struct Scrolls {
    pub notifications: f32,
    pub reminders: f32,
    pub shelf: f32,
    pub calendar: f32,
    pub chat: f32,
    pub lyrics: f32,
}

pub struct App {
    pub hwnd: HWND,
    pub renderer: Renderer,
    pub animator: Animator,
    pub settings: Settings,
    settings_path: PathBuf,
    pub store: Store,

    pub page: Page,
    pub expanded: bool,
    pub tabs_visible: bool,
    pub alert: Option<Alert>,

    pub clock: Clock,
    minute: u16,

    pub media: Media,
    pub lyrics: Lyrics,
    pub usage: Usage,
    pub focus: Focus,
    pub voice: Voice,

    pub shelf_files: bool,
    pub shelf_filter: usize,
    /// Eases the Clipboard/Files highlight from one segment to the other so the
    /// segmented control glides, and the two card rows cross-fade.
    pub shelf_tab: f32,
    /// Decoded clipboard thumbnails, keyed by the image path they came from.
    pub clip_thumbs: HashMap<String, windows::Win32::Graphics::Direct2D::ID2D1Bitmap>,
    thumb_pending: HashSet<String>,
    thumb_failed: HashSet<String>,
    thumb_tx: Sender<(String, Option<(u32, u32, Vec<u8>)>)>,
    thumb_rx: Receiver<(String, Option<(u32, u32, Vec<u8>)>)>,
    pub calendar_month: u32,
    pub mouse: (f32, f32),
    pub hover_body: bool,
    leave_at: Option<Instant>,
    intro_until: Option<Instant>,

    pub input: Input,
    pub scrolls: Scrolls,
    pub blink: Blink,

    pub art: Option<(String, windows::Win32::Graphics::Direct2D::ID2D1Bitmap)>,
    /// The logo of the app making sound, shown in the closed island's left slot
    /// in place of the assistant while something plays.
    pub app_icon: Option<(String, windows::Win32::Graphics::Direct2D::ID2D1Bitmap)>,
    /// The sleeve's dominant colour, used to tint the lyric page the way the
    /// reference washed the card in the artwork's own palette.
    pub art_color: Option<u32>,
    /// Where the scrubber of whichever player is on screen was last drawn, so
    /// a click on the bar can be turned into a position.
    pub seek_bar: std::cell::Cell<Rect>,

    tick: u32,
    next_tick: Instant,
    next_anim: Instant,
    last_frame: Instant,
    started: Instant,
    art_key: String,
    app_icon_key: String,
    /// Eases a freshly switched page in from transparent over a fifth of a
    /// second, so a tab click cross-fades instead of snapping.
    pub page_fade: f32,
    /// The lyric stream's running scroll, eased toward the line being sung.
    pub lyric_offset: f32,
    /// How open each header tab's pill is (indexed by `Page::TABS`), eased
    /// toward 1 for the selected tab and 0 for the rest so the highlight glides
    /// from one to the next instead of snapping.
    pub tab_open: [f32; 8],
    pub dirty: bool,
    hit_list: Vec<crate::ui::Hit>,
    last_clip: String,
    plugged: bool,
    caps: bool,
    quit_at: Option<Instant>,
}

impl App {
    pub fn new(hwnd: HWND, renderer: Renderer, quit_after: Option<u64>) -> Self {
        let settings_path = crate::settings::config_dir().join("settings.json");
        let settings = Settings::load(&settings_path);
        // The render clock starts at the frame rate already on disk, so a
        // chosen 120/144 is honoured from the first frame.
        crate::set_frame_interval(settings.target_fps);
        let store = Store::open();
        let clock = Clock::read();
        let power = sys::Power::read();
        let voice_label = settings.ai_provider.clone();
        let mut media = Media::new();
        media.poll(true);
        let now = Instant::now();
        // The closed island starts at whatever height the preferences set.
        let mut animator = Animator::new();
        let collapsed = settings.island_height.clamp(24, 54) as f32;
        animator.height.value = collapsed;
        animator.height.target = collapsed;
        let (thumb_tx, thumb_rx) = std::sync::mpsc::channel();
        Self {
            hwnd,
            renderer,
            animator,
            focus: Focus::new(settings.focus_work_min),
            settings,
            settings_path,
            store,
            page: Page::Music,
            expanded: false,
            tabs_visible: true,
            alert: None,
            clock,
            minute: clock.minute,
            media,
            lyrics: Lyrics::default(),
            usage: Usage::default(),
            voice: Voice {
                messages: vec![Message {
                    from_user: false,
                    text: format!(
                        "Hi, I'm Isle. I answer with {voice_label} on this PC. Ask about it, or tell me what to open, install or change."
                    ),
                    card: None,
                }],
                rx: None,
                run: None,
                label: crate::chat::label(&voice_label).to_string(),
                thinking_at: None,
                dots: 0,
            },
            shelf_files: false,
            shelf_filter: 0,
            shelf_tab: 0.0,
            clip_thumbs: HashMap::new(),
            thumb_pending: HashSet::new(),
            thumb_failed: HashSet::new(),
            thumb_tx,
            thumb_rx,
            calendar_month: clock.month.saturating_sub(1) as u32,
            hit_list: Vec::new(),
            mouse: (-1.0, -1.0),
            hover_body: false,
            leave_at: None,
            intro_until: Some(now + Duration::from_millis(2000)),
            input: Input { field: None, text: String::new() },
            scrolls: Scrolls::default(),
            blink: Blink::new(),
            art: None,
            app_icon: None,
            seek_bar: std::cell::Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            tick: 0,
            next_tick: now,
            next_anim: now,
            last_frame: now,
            started: now,
            art_key: String::new(),
            app_icon_key: String::new(),
            page_fade: 1.0,
            lyric_offset: 0.0,
            tab_open: [0.0; 8],
            art_color: None,
            dirty: true,
            last_clip: String::new(),
            plugged: power.plugged && power.percent < 255,
            caps: sys::caps_lock_on(),
            quit_at: quit_after.map(|seconds| now + Duration::from_secs(seconds)),
        }
    }

    // ---- geometry ---------------------------------------------------------

    /// The notch body in window coordinates, at the current spring positions.
    pub fn body(&self) -> Rect {
        let w = self.animator.width.value;
        let s = self.animator.shoulder.value;
        let total = w + 2.0 * s;
        Rect::new(
            (HOST_W - total) / 2.0,
            0.0,
            total,
            self.animator.height.value,
        )
    }

    /// How far the island has grown towards its expanded size, 0 to 1.
    pub fn progress(&self) -> f32 {
        ((self.animator.width.value - COMPACT_W) / (EXPANDED_W - COMPACT_W)).clamp(0.0, 1.0)
    }

    /// Seconds since the window opened, the clock the equalizer runs on.
    pub fn seconds(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    // ---- lifecycle --------------------------------------------------------

    pub fn expand(&mut self) {
        if self.alert.is_some() {
            self.dismiss_alert();
        }
        self.expanded = true;
        let width = if self.page == Page::Lyrics {
            LYRICS_W
        } else {
            EXPANDED_W
        };
        self.animator
            .start(width, self.expanded_height(), 32.0, 22.0);
        self.dirty = true;
    }

    pub fn expanded_height(&self) -> f32 {
        if self.page == Page::Voice {
            if self.tabs_visible {
                VOICE_MENU_H
            } else {
                VOICE_H
            }
        } else {
            PAGE_H
        }
    }

    /// The height of the closed island, as the preferences' stepper set it.
    pub fn collapsed_height(&self) -> f32 {
        self.settings.island_height.clamp(24, 54) as f32
    }

    /// What the clipboard's search box is narrowing the shelf with, in lower
    /// case, or an empty string while the box is shut.
    pub fn shelf_query(&self) -> String {
        if self.input.field == Some(Field::Search) {
            self.input.text.trim().to_lowercase()
        } else {
            String::new()
        }
    }

    pub fn collapse(&mut self) {
        self.expanded = false;
        self.input.field = None;
        self.animator
            .start(COMPACT_W, self.collapsed_height(), 16.0, 10.0);
        self.dirty = true;
    }

    /// Raise an alert card in place of whatever was on screen, for six seconds.
    pub fn show_alert(&mut self, kind: AlertKind) {
        let (w, h, r) = match kind {
            AlertKind::Mini => (232.0, 32.0, 16.0),
            AlertKind::Charging => (320.0, 52.0, 26.0),
            AlertKind::Meeting | AlertKind::Message => (380.0, 52.0, 26.0),
            AlertKind::Permission => (440.0, 96.0, 28.0),
        };
        self.alert = Some(Alert {
            kind,
            until: Instant::now() + Duration::from_secs(6),
        });
        self.expanded = false;
        self.animator.start(w, h, r, 13.0);
        self.dirty = true;
    }

    pub fn dismiss_alert(&mut self) {
        if self.alert.take().is_some() {
            self.animator
                .start(COMPACT_W, self.collapsed_height(), 16.0, 10.0);
            self.dirty = true;
        }
    }

    /// Re-reads `settings.json`, which the preferences window just wrote.
    pub fn reload_settings(&mut self) {
        self.settings = Settings::load(&self.settings_path);
        // The preferences may have moved the frame rate; re-point the clock.
        crate::set_frame_interval(self.settings.target_fps);
        if !self.focus.running {
            self.focus.reset(self.settings.focus_work_min);
        }
        self.voice.label = crate::chat::label(&self.settings.ai_provider).to_string();
        if !self.expanded && self.alert.is_none() {
            // The closed island takes a new height at once, as the script's
            // stepper did.
            let height = self.collapsed_height();
            self.animator.height.value = height;
            self.animator.height.target = height;
        }
        self.dirty = true;
    }

    /// The preview a row in the preferences asks for, by its position in
    /// [`AlertKind`]'s list.
    pub fn preview_alert(&mut self, code: usize) {
        let kind = match code {
            0 => AlertKind::Mini,
            1 => AlertKind::Charging,
            2 => AlertKind::Meeting,
            3 => AlertKind::Message,
            _ => AlertKind::Permission,
        };
        self.show_alert(kind);
    }

    /// The preferences' Clear button: pinned items stay, as its subtitle says.
    pub fn clear_clips(&mut self) {
        self.store.clips.retain(|clip| clip.pinned);
        self.store.save_clips();
        self.dirty = true;
    }

    /// Switching pages, with the script's sizing rules: the conversation page
    /// keeps its taller box, the lyrics mode is twenty pixels wider.
    pub fn switch_page(&mut self, page: Page) {
        if self.page == page && page != Page::Voice {
            return;
        }
        self.page = page;
        // The new face eases in; the lyric stream starts parked on the line
        // being sung rather than scrolling all the way from the top.
        self.page_fade = 0.0;
        if page == Page::Lyrics {
            let active = crate::lyrics::active_line(&self.lyrics.lines, self.media.position_ms())
                .unwrap_or(0) as f32;
            self.lyric_offset = active * crate::lyrics::LINE_PITCH;
        }
        if page == Page::Voice {
            self.tabs_visible = false;
        } else {
            self.tabs_visible = true;
        }
        self.scrolls = Scrolls::default();
        if page == Page::Usage {
            self.usage.poll();
        }
        if self.expanded {
            let width = if page == Page::Lyrics {
                LYRICS_W
            } else {
                EXPANDED_W
            };
            self.animator
                .start(width, self.expanded_height(), 32.0, 22.0);
        }
        self.dirty = true;
    }

    // ---- input ------------------------------------------------------------

    /// Mouse moved, in logical coordinates.
    pub fn mouse_move(&mut self, x: f32, y: f32) {
        self.mouse = (x, y);
        let inside = self.body().contains(x, y);
        if inside && !self.hover_body {
            self.hover_body = true;
            self.leave_at = None;
            if self.alert.is_some() {
                if let Some(alert) = self.alert.as_mut() {
                    alert.until = Instant::now() + Duration::from_secs(6);
                }
            } else if !self.expanded && self.settings.opens_on_hover() {
                self.expand();
            }
        } else if !inside && self.hover_body {
            self.hover_body = false;
            if self.alert.is_none() && self.expanded {
                self.leave_at = Some(Instant::now());
            }
        }
    }

    /// The pointer left the host window, which sends no further moves. `x`/`y`
    /// are where it actually got to, read from the OS: the leave tick below
    /// re-tests the recorded position, and a stale one that still read
    /// "inside" would leave the island open for good.
    pub fn mouse_leave(&mut self, x: f32, y: f32) {
        self.mouse = (x, y);
        if self.hover_body {
            self.hover_body = false;
            if self.alert.is_none() && self.expanded {
                self.leave_at = Some(Instant::now());
            }
        }
    }

    pub fn click(&mut self, x: f32, y: f32) {
        if let Some(action) = self.pick(x, y).cloned() {
            match action {
                // A scrubber has no single place to jump to: the position is
                // worked out from where inside the bar the pointer landed.
                Action::SeekMusic | Action::SeekLyrics => self.seek_at(x),
                other => self.dispatch(other),
            }
            return;
        }
        if !self.expanded && self.alert.is_none() && self.body().contains(x, y) {
            self.expand();
        }
    }

    fn seek_at(&mut self, x: f32) {
        let bar = self.seek_bar.get();
        if bar.w <= 0.0 || self.media.now.duration_ms == 0 {
            return;
        }
        let ratio = ((x - bar.x) / bar.w).clamp(0.0, 1.0);
        self.media.seek(ratio);
        self.media.poll(true);
        self.dirty = true;
    }

    fn pick(&self, x: f32, y: f32) -> Option<&Action> {
        self.hit_list
            .iter()
            .rev()
            .find(|hit| hit.rect.contains(x, y))
            .map(|hit| &hit.action)
    }

    pub fn wheel(&mut self, delta: i32) {
        let step = if delta > 0 { -26.0 } else { 26.0 };
        let max = self.scrolls_max();
        match self.page {
            Page::Notifications => {
                self.scrolls.notifications = (self.scrolls.notifications + step).clamp(0.0, max)
            }
            Page::Reminders => {
                self.scrolls.reminders = (self.scrolls.reminders + step).clamp(0.0, max)
            }
            Page::Shelf => self.scrolls.shelf = (self.scrolls.shelf + step).clamp(0.0, max),
            Page::Calendar => {
                self.scrolls.calendar = (self.scrolls.calendar + step).clamp(0.0, max)
            }
            Page::Voice => self.scrolls.chat = (self.scrolls.chat + step).clamp(0.0, max),
            Page::Lyrics => self.scrolls.lyrics = (self.scrolls.lyrics + step).clamp(0.0, max),
            _ => {}
        }
        self.dirty = true;
    }

    /// Files dropped on the island go to the shelf, in the same six slots the
    /// clipboard monitor fills.
    pub fn drop_files(&mut self, paths: Vec<PathBuf>) {
        let mut changed = false;
        for path in paths {
            changed |= self.store.add_shelf(&path);
        }
        if changed {
            self.store.save_shelf();
            self.shelf_files = true;
            self.dirty = true;
        }
    }

    fn scrolls_max(&self) -> f32 {
        match self.page {
            Page::Voice => (self.voice.messages.len() as f32 * 34.0 - 120.0).max(0.0),
            Page::Reminders => (self.store.reminders.len() as f32 * 40.0 - 110.0).max(0.0),
            Page::Calendar => (self.store.events.len() as f32 * 34.0 - 100.0).max(0.0),
            Page::Lyrics => (self.lyrics.lines.len() as f32 * 34.0 - 120.0).max(0.0),
            _ => 400.0,
        }
    }

    pub fn key(&mut self, vk: u32) {
        const VK_ESCAPE: u32 = 0x1B;
        const VK_RETURN: u32 = 0x0D;
        const VK_BACK: u32 = 0x08;
        match vk {
            VK_ESCAPE => {
                if self.input.field == Some(Field::Search) {
                    // The script's search box swallowed Escape: it emptied the
                    // box and put the magnifier back, and the island stayed.
                    self.input.text.clear();
                    self.input.field = None;
                    self.dirty = true;
                } else if self.expanded {
                    self.collapse();
                } else {
                    unsafe {
                        let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
                    }
                }
            }
            VK_RETURN => self.submit_field(),
            VK_BACK => {
                if self.input.field.is_some() {
                    self.input.text.pop();
                    self.dirty = true;
                }
            }
            _ => {}
        }
    }

    pub fn typed(&mut self, ch: char) {
        if self.input.field.is_none() {
            return;
        }
        if ch.is_control() {
            return;
        }
        if self.input.text.chars().count() < 200 {
            self.input.text.push(ch);
            self.dirty = true;
        }
    }

    fn focus_field(&mut self, field: Field) {
        self.input.field = Some(field);
        self.input.text.clear();
        self.dirty = true;
    }

    fn submit_field(&mut self) {
        let text = self.input.text.trim().to_string();
        let Some(field) = self.input.field else {
            return;
        };
        if text.is_empty() {
            return;
        }
        match field {
            Field::Reminder => {
                self.store.reminders.push(Reminder { text, done: false });
                self.store.save_reminders();
            }
            Field::Calendar => {
                let clock = Clock::read();
                self.store.events.push(CalEvent {
                    title: text,
                    time: clock.header_time(),
                    color: "#3584E4".into(),
                });
                self.store.save_events();
            }
            Field::Chat => {
                self.input.text.clear();
                self.send_chat(text);
                return;
            }
            // The shelf narrows on every keystroke; Enter has nothing to add.
            Field::Search => return,
        }
        self.input.text.clear();
        self.dirty = true;
    }

    // ---- actions ----------------------------------------------------------

    fn dispatch(&mut self, action: Action) {
        match action {
            Action::Expand => self.expand(),
            Action::Tab(page) => self.switch_page(page),
            Action::ToggleMenu => {
                self.tabs_visible = !self.tabs_visible;
                if self.expanded {
                    let width = if self.page == Page::Lyrics {
                        LYRICS_W
                    } else {
                        EXPANDED_W
                    };
                    let height = if self.page == Page::Voice {
                        if self.tabs_visible {
                            VOICE_MENU_H
                        } else {
                            VOICE_H
                        }
                    } else if self.tabs_visible {
                        VOICE_MENU_H
                    } else {
                        VOICE_H
                    };
                    self.animator.start(width, height, 32.0, 22.0);
                }
                self.dirty = true;
            }
            Action::VoiceButton => {
                self.switch_page(Page::Voice);
                self.tabs_visible = false;
                if self.expanded {
                    self.animator.start(EXPANDED_W, VOICE_H, 32.0, 22.0);
                }
            }
            Action::Gear => crate::prefs::show(),
            Action::PlayPause => {
                self.media.toggle();
                self.media.poll(true);
            }
            Action::NextTrack => {
                self.media.next();
                self.media.poll(true);
            }
            Action::PrevTrack => {
                self.media.previous();
                self.media.poll(true);
            }
            Action::OutputDevice => {
                // The script's `BtnOutput` ran `Start-Process "ms-settings:sound"`;
                // the same URI goes through the shell here.
                let _ = crate::chat::shell_open(std::ffi::OsStr::new("ms-settings:sound"));
            }
            Action::SeekMusic | Action::SeekLyrics => {}
            Action::LyricsMode => self.switch_page(Page::Lyrics),
            Action::BackToMusic => self.switch_page(Page::Music),
            Action::LyricLine(at_ms) => {
                if self.media.now.can_seek && self.media.now.duration_ms > 0 {
                    let ratio = at_ms as f32 / self.media.now.duration_ms as f32;
                    self.media.seek(ratio);
                }
            }
            Action::FocusStart => {
                self.focus
                    .toggle(self.settings.focus_work_min, self.settings.focus_break_min);
            }
            Action::FocusReset => self.focus.reset(self.settings.focus_work_min),
            Action::FocusWork(kind) => {
                self.focus.work = kind == 0;
                self.focus.running = false;
                self.focus.ends = None;
                self.focus.left_ms = if self.focus.work {
                    self.settings.focus_work_min as i64 * 60_000
                } else {
                    self.settings.focus_break_min as i64 * 60_000
                };
            }
            Action::WorkInc => {
                self.settings.focus_work_min = (self.settings.focus_work_min + 1).min(90);
                self.settings.save(&self.settings_path);
                self.focus.reset(self.settings.focus_work_min);
            }
            Action::WorkDec => {
                self.settings.focus_work_min =
                    self.settings.focus_work_min.saturating_sub(1).max(1);
                self.settings.save(&self.settings_path);
                self.focus.reset(self.settings.focus_work_min);
            }
            Action::BreakInc => {
                self.settings.focus_break_min = (self.settings.focus_break_min + 1).min(30);
                self.settings.save(&self.settings_path);
            }
            Action::BreakDec => {
                self.settings.focus_break_min =
                    self.settings.focus_break_min.saturating_sub(1).max(1);
                self.settings.save(&self.settings_path);
            }
            Action::ReminderToggle(index) => {
                if let Some(item) = self.store.reminders.get_mut(index) {
                    item.done = !item.done;
                    self.store.save_reminders();
                }
            }
            Action::ReminderAdd => {
                if self.input.field == Some(Field::Reminder) && !self.input.text.trim().is_empty() {
                    self.submit_field();
                } else {
                    self.focus_field(Field::Reminder);
                }
            }
            Action::ReminderField => self.focus_field(Field::Reminder),
            Action::ShelfClip => self.shelf_files = false,
            Action::ShelfFiles => {
                // The script hid the search with the rest of the clipboard
                // controls on this tab, so nothing is left typing unseen.
                self.shelf_files = true;
                if self.input.field == Some(Field::Search) {
                    self.input.text.clear();
                    self.input.field = None;
                }
            }
            Action::ShelfFilter(index) => self.shelf_filter = index as usize,
            Action::ShelfSearch => {
                if self.input.field != Some(Field::Search) {
                    self.input.text.clear();
                    self.input.field = Some(Field::Search);
                }
            }
            Action::ShelfSearchClose => {
                self.input.text.clear();
                self.input.field = None;
            }
            Action::ShelfClear => {
                self.store.clips.clear();
                self.store.save_clips();
            }
            Action::ShelfCopy(index) => {
                if let Some(clip) = self.store.clips.get(index) {
                    sys::Clipboard::set_text(&clip.text);
                }
            }
            Action::ShelfCopyFile(index) => {
                if let Some(file) = self.store.shelf.get(index) {
                    if let Ok(bytes) = std::fs::read(&file.path) {
                        let _ = bytes;
                    }
                    sys::Clipboard::set_text(&file.path);
                }
            }
            Action::CalPrev => self.calendar_month = (self.calendar_month + 11) % 12,
            Action::CalNext => self.calendar_month = (self.calendar_month + 1) % 12,
            Action::CalAdd => {
                if self.input.field == Some(Field::Calendar) && !self.input.text.trim().is_empty() {
                    self.submit_field();
                } else {
                    self.focus_field(Field::Calendar);
                }
            }
            Action::CalField => self.focus_field(Field::Calendar),
            Action::NotifDnd => {
                self.settings.dnd = !self.settings.dnd;
                self.settings.save(&self.settings_path);
            }
            Action::NotifClear => {
                self.store.notifications.clear();
                self.store.save_notifications();
            }
            Action::UsageDot(index) => self.usage.index = index,
            Action::ChatField => self.focus_field(Field::Chat),
            Action::ChatSend => {
                if !self.input.text.trim().is_empty() {
                    self.submit_field();
                }
            }
            Action::ChatNew => {
                self.voice.messages.clear();
                self.voice.messages.push(Message {
                    from_user: false,
                    text: "New conversation. What do you need?".into(),
                    card: None,
                });
                self.scroll_to_end();
            }
            Action::CardRun(index) => self.run_card(index),
            Action::CardDeny(index) => self.deny_card(index),
            Action::CallMute => {}
            Action::CallJoin | Action::CallEnd => {}
            Action::AlertDismiss => self.dismiss_alert(),
            Action::AlertDeny | Action::AlertAllow | Action::AlertAlways => self.dismiss_alert(),
            Action::AlertReply | Action::AlertJoin => self.dismiss_alert(),
        }
        self.dirty = true;
    }

    // ---- conversation -----------------------------------------------------

    fn send_chat(&mut self, prompt: String) {
        let text = prompt.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.voice.messages.push(Message {
            from_user: true,
            text: text.clone(),
            card: None,
        });
        self.scroll_to_end();

        // The script's chain: a volume, a program, a package, a shell line, and
        // then the assistant itself.
        match crate::chat::plan(&text) {
            chat::Plan::Volume(share) => self.set_volume(share),
            chat::Plan::Open(target) => {
                let say = if chat::launch(&target) {
                    format!("Opened {target}.")
                } else {
                    format!("I could not find {target} on this PC.")
                };
                self.say(say);
            }
            chat::Plan::Card { say, line } => {
                self.say(say);
                self.voice.messages.push(Message {
                    from_user: false,
                    text: String::new(),
                    card: Some(Card {
                        line,
                        state: CardState::Pending,
                    }),
                });
                self.scroll_to_end();
            }
            chat::Plan::Assistant => self.ask_assistant(text),
        }
        self.dirty = true;
    }

    fn say(&mut self, text: String) {
        self.voice.messages.push(Message {
            from_user: false,
            text,
            card: None,
        });
        self.scroll_to_end();
    }

    fn set_volume(&mut self, share: u32) {
        if share > 100 {
            self.say("Volume goes from 0 to 100.".into());
            return;
        }
        if media::set_volume(share as f32 / 100.0) {
            self.say(format!("Master volume is at {share}%."));
        } else {
            self.say("Windows would not let me change the volume.".into());
        }
    }

    fn ask_assistant(&mut self, prompt: String) {
        self.voice.messages.push(Message {
            from_user: false,
            text: "Thinking".into(),
            card: None,
        });
        self.voice.thinking_at = Some(Instant::now());
        self.voice.dots = 0;
        self.scroll_to_end();
        self.voice.label = chat::label(&self.settings.ai_provider).to_string();
        match chat::ask(&self.settings.ai_provider, &self.settings.ai_model, &prompt) {
            chat::Spawn::Started(rx) => self.voice.rx = Some(rx),
            chat::Spawn::NotInstalled => self.finish_reply(
                "The assistant chosen in Preferences is not installed on this PC. Install it, or choose another one."
                    .into(),
            ),
            chat::Spawn::Failed(error) => {
                self.finish_reply(format!("I could not start the assistant: {error}"))
            }
        }
    }

    /// Runs the card on message `index`, once the user has approved it.
    fn run_card(&mut self, index: usize) {
        let Some(message) = self.voice.messages.get_mut(index) else {
            return;
        };
        let Some(card) = message.card.as_mut() else {
            return;
        };
        if card.state != CardState::Pending {
            return;
        }
        match chat::run(&card.line) {
            Some(rx) => {
                card.state = CardState::Running(Instant::now());
                self.voice.run = Some((index, rx));
            }
            None => {
                card.state = CardState::Done {
                    ok: false,
                    code: -1,
                };
                self.voice.messages.push(Message {
                    from_user: false,
                    text: "I could not start that.".into(),
                    card: None,
                });
            }
        }
        self.dirty = true;
    }

    fn deny_card(&mut self, index: usize) {
        if let Some(card) = self
            .voice
            .messages
            .get_mut(index)
            .and_then(|m| m.card.as_mut())
        {
            if card.state == CardState::Pending {
                card.state = CardState::Denied;
            }
        }
        self.say("Left it alone.".into());
        self.dirty = true;
    }

    fn finish_reply(&mut self, text: String) {
        if let Some(last) = self.voice.messages.last_mut() {
            last.text = text;
        }
        self.voice.rx = None;
        self.voice.thinking_at = None;
        self.dirty = true;
    }

    fn scroll_to_end(&mut self) {
        self.scrolls.chat = self.scrolls_max();
    }

    // ---- album art --------------------------------------------------------

    /// Decodes the sleeve of the current track, once per track. The decode
    /// happens off the WinRT stream and into a Direct2D bitmap the target owns.
    fn refresh_art(&mut self) {
        let key = format!("{}|{}", self.media.now.title, self.media.now.artist);
        if key == self.art_key {
            return;
        }
        self.art_key = key.clone();
        let decoded = self
            .media
            .art
            .as_ref()
            .and_then(|bytes| crate::art::decode(bytes));
        self.art_color = decoded
            .as_ref()
            .and_then(|(width, height, pixels)| dominant_colour(*width, *height, pixels));
        self.art = decoded
            .and_then(|(width, height, pixels)| self.renderer.bitmap(width, height, &pixels))
            .map(|bitmap| (key, bitmap));
        self.dirty = true;
    }

    /// Turns the worker's icon bytes into a Direct2D bitmap, once per source
    /// app. The bytes are already premultiplied BGRA, so they become a bitmap
    /// without a decode step.
    fn refresh_icon(&mut self) {
        let aumid = self.media.now.source_aumid.clone();
        if aumid.is_empty() {
            if self.app_icon.is_some() {
                self.app_icon = None;
                self.dirty = true;
            }
            self.app_icon_key.clear();
            return;
        }
        // Rebuild when the source changes, or when a lookup that had come back
        // empty has since produced bytes: the worker retries the find in the
        // background, and this is where the late icon becomes a bitmap.
        let stale = aumid != self.app_icon_key;
        let late = self.app_icon.is_none() && self.media.icon.is_some();
        if !stale && !late {
            return;
        }
        self.app_icon_key = aumid;
        self.app_icon = self
            .media
            .icon
            .as_ref()
            .and_then(|(width, height, pixels)| self.renderer.bitmap(*width, *height, pixels))
            .map(|bitmap| (self.app_icon_key.clone(), bitmap));
        self.dirty = true;
    }

    // ---- the quarter-second tick -----------------------------------------

    fn tick(&mut self, now: Instant) {
        self.tick = self.tick.wrapping_add(1);

        let clock = Clock::read();
        if clock.minute != self.minute {
            self.minute = clock.minute;
            self.clock = clock;
            self.dirty = true;
        } else {
            self.clock.second = clock.second;
        }

        self.media.poll(false);
        self.media.poll_sound();
        self.refresh_art();
        self.refresh_icon();
        self.lyrics.ensure(
            &self.media.now.title,
            &self.media.now.artist,
            self.media.now.duration_ms,
        );
        self.lyrics.poll();
        if self.page == Page::Usage {
            self.usage.poll();
            if clock.second != self.clock.second {
                self.dirty = true;
            }
        }
        if self.blink.tick() {
            self.dirty = true;
        }

        // Clipboard every other tick, as the script did.
        if self.tick % 2 == 0 && sys::Clipboard::open() {
            let files = sys::Clipboard::files();
            let text = sys::Clipboard::text().unwrap_or_default();
            sys::Clipboard::close();
            if !files.is_empty() {
                let mut changed = false;
                for file in files {
                    changed |= self.store.add_shelf(std::path::Path::new(&file));
                }
                if changed {
                    self.store.save_shelf();
                    self.dirty = true;
                }
            } else if !text.trim().is_empty() && text != self.last_clip {
                self.last_clip = text.clone();
                if self.store.push_clip(text) {
                    self.store.save_clips();
                    self.dirty = true;
                }
            }
        }

        let power = sys::Power::read();
        let plugged = power.plugged && power.percent < 255;
        if plugged != self.plugged {
            self.plugged = plugged;
            if plugged && self.settings.alert_charging {
                self.show_alert(AlertKind::Charging);
            }
        }
        let caps = sys::caps_lock_on();
        if caps != self.caps {
            self.caps = caps;
            // The script raised the mini row for either direction of the lock.
            if self.settings.alert_caps_lock {
                self.show_alert(AlertKind::Mini);
            }
        }

        if self
            .focus
            .tick(self.settings.focus_work_min, self.settings.focus_break_min)
        {
            keys::beep();
            self.show_alert(AlertKind::Mini);
            self.dirty = true;
        }

        if let Some(rx) = self.voice.rx.as_ref() {
            match rx.try_recv() {
                Ok(text) => self.finish_reply(text),
                Err(TryRecvError::Empty) => {
                    self.voice.dots = (self.voice.dots + 1) % 4;
                    if let Some(last) = self.voice.messages.last_mut() {
                        last.text = format!("Thinking{}", ".".repeat(self.voice.dots as usize));
                    }
                    self.dirty = true;
                }
                Err(TryRecvError::Disconnected) => {
                    let who = self.voice.label.clone();
                    self.finish_reply(format!("No answer came back. Is {who} signed in?"))
                }
            }
        }

        if let Some((index, rx)) = self.voice.run.as_ref() {
            match rx.try_recv() {
                Ok(outcome) => {
                    let index = *index;
                    let ok = outcome.code == 0;
                    let tail = outcome.tail;
                    if let Some(card) = self
                        .voice
                        .messages
                        .get_mut(index)
                        .and_then(|message| message.card.as_mut())
                    {
                        card.state = CardState::Done {
                            ok,
                            code: outcome.code,
                        };
                    }
                    self.voice.run = None;
                    let text = if ok {
                        if tail.is_empty() {
                            "Done.".to_string()
                        } else {
                            format!("Done.\n{tail}")
                        }
                    } else if tail.is_empty() {
                        format!("It stopped with exit code {}.", outcome.code)
                    } else {
                        format!("It stopped with exit code {}.\n{tail}", outcome.code)
                    };
                    self.say(text);
                }
                // The card prints the elapsed time, so it needs a redraw.
                Err(TryRecvError::Empty) => self.dirty = true,
                Err(TryRecvError::Disconnected) => {
                    self.voice.run = None;
                    self.dirty = true;
                }
            }
        }

        if let Some(alert) = self.alert.as_ref() {
            if now >= alert.until {
                self.dismiss_alert();
            }
        }

        if let Some(leave) = self.leave_at {
            if now.duration_since(leave) >= Duration::from_millis(300) {
                self.leave_at = None;
                let (x, y) = self.mouse;
                if !self.body().contains(x, y) && self.expanded {
                    self.collapse();
                }
            }
        }

        if let Some(intro) = self.intro_until {
            if now >= intro {
                self.intro_until = None;
                if !self.hover_body && self.expanded {
                    self.collapse();
                }
            } else if !self.expanded && self.alert.is_none() && self.mouse == (-1.0, -1.0) {
                // The "hello" wave, and only that: it re-expands on every tick
                // until the pointer has first come near. Once the user has moved
                // over the island, its own hover and leave rules take over, so a
                // collapse is never fought by a still-running intro.
                self.expand();
            }
        }
    }

    // ---- frame ------------------------------------------------------------

    pub fn frame(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();

        if now >= self.next_tick {
            self.next_tick = now + Duration::from_millis(250);
            self.tick(now);
        }

        // The render clock already paces frames to the chosen rate, so every
        // wake steps the springs; `dt` is the time since the last one.
        self.last_frame = now;
        if self.animator.step(dt) {
            self.dirty = true;
        } else if self.media.sound && now >= self.next_anim {
            // The equalizer is the only thing that moves on its own while
            // there is sound; sixteen frames a second is smooth enough for
            // four 2.5 px bars and costs a fraction of a real animation
            // clock.
            self.next_anim = now + Duration::from_millis(62);
            self.dirty = true;
        }
        // The scrubber and the running time are extrapolated from the last
        // reading, so they change every frame while a track plays. Left on the
        // equalizer's 62 ms beat they would jump twice a second - the "the
        // seconds are not responsive" the reference never showed - so on the
        // player and lyric pages we repaint each frame the clock hands us.
        if self.media.now.playing && matches!(self.page, Page::Music | Page::Lyrics) {
            self.dirty = true;
        }
        // A running blink changes the eye height every frame.
        if self.blink.blink_until.is_some() {
            self.dirty = true;
        }
        // A switched page rides a short cross-fade so the new face eases in
        // instead of snapping into place.
        if self.page_fade < 1.0 {
            self.page_fade = (self.page_fade + dt / 0.18).min(1.0);
            self.dirty = true;
        }
        // The header highlight glides: a tab's pill eases open only while it is
        // the selected one, so switching closes the old pill and opens the new
        // (and the row reflows) smoothly rather than snapping.
        {
            let want = self.page.tab();
            let mut moving = false;
            for (index, tab) in Page::TABS.iter().enumerate() {
                let target = if *tab == want { 1.0 } else { 0.0 };
                let cur = self.tab_open[index];
                if (cur - target).abs() > 0.002 {
                    let k = 1.0 - (-16.0 * dt).exp();
                    self.tab_open[index] = cur + (target - cur) * k;
                    moving = true;
                } else if cur != target {
                    self.tab_open[index] = target;
                    moving = true;
                }
            }
            if moving {
                self.dirty = true;
            }
        }
        // The shelf's Clipboard/Files highlight glides between the two segments,
        // easing toward whichever tab is now selected.
        {
            let target = if self.shelf_files { 1.0 } else { 0.0 };
            if (self.shelf_tab - target).abs() > 0.002 {
                let k = 1.0 - (-16.0 * dt).exp();
                self.shelf_tab += (target - self.shelf_tab) * k;
                self.dirty = true;
            } else if self.shelf_tab != target {
                self.shelf_tab = target;
                self.dirty = true;
            }
        }
        // Adopt any clipboard thumbnails the loader thread has finished decoding.
        while let Ok((path, decoded)) = self.thumb_rx.try_recv() {
            self.thumb_pending.remove(&path);
            match decoded {
                Some((width, height, pixels)) => {
                    if let Some(bitmap) = self.renderer.bitmap(width, height, &pixels) {
                        self.clip_thumbs.insert(path, bitmap);
                    }
                }
                None => {
                    self.thumb_failed.insert(path);
                }
            }
            self.dirty = true;
        }
        // Ask the loader for image paths shown on the shelf that are neither
        // cached nor already in flight, so pictures preview themselves.
        if self.page == Page::Shelf {
            let mut wanted: Vec<String> = Vec::new();
            if self.shelf_files {
                for file in self.store.shelf.iter() {
                    wanted.push(file.path.clone());
                }
            } else {
                for clip in self.store.clips.iter() {
                    wanted.push(clip.text.trim().to_string());
                }
            }
            for path in wanted {
                if !crate::pages::tools::is_image_path(&path) {
                    continue;
                }
                if self.clip_thumbs.contains_key(&path)
                    || self.thumb_pending.contains(&path)
                    || self.thumb_failed.contains(&path)
                {
                    continue;
                }
                self.thumb_pending.insert(path.clone());
                let tx = self.thumb_tx.clone();
                std::thread::spawn(move || {
                    let bytes = std::fs::read(&path).ok();
                    let decoded = bytes.as_deref().and_then(crate::art::decode);
                    let _ = tx.send((path, decoded));
                });
            }
        }
        // The lyric stream glides to the line being sung, the way a karaoke
        // pane drifts upward as the song advances.
        if self.page == Page::Lyrics && !self.lyrics.plain && !self.lyrics.lines.is_empty() {
            let active =
                crate::lyrics::active_line(&self.lyrics.lines, self.media.position_ms()).unwrap_or(0)
                    as f32;
            let target = active * crate::lyrics::LINE_PITCH;
            let k = 1.0 - (-10.0 * dt).exp();
            let next = self.lyric_offset + (target - self.lyric_offset) * k;
            if (next - self.lyric_offset).abs() > 0.05 {
                self.lyric_offset = next;
                self.dirty = true;
            } else if self.lyric_offset != target {
                self.lyric_offset = target;
                self.dirty = true;
            }
        }

        if self.dirty {
            self.draw();
        }

        if let Some(deadline) = self.quit_at {
            if now >= deadline {
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
                }
            }
        }
    }

    fn draw(&mut self) {
        self.dirty = false;
        if self.renderer.begin().is_err() {
            return;
        }
        // Both borrows are immutable: the canvas only reads the target, and the
        // pages only read the state. The one thing paint writes is the rect of
        // the scrubber, which is a Cell for exactly that reason.
        let hits = pages::paint(self, &self.renderer);
        let _ = self.renderer.end();
        let _ = self.renderer.present(self.hwnd);
        self.hit_list = hits;
    }
}

/// The sleeve's dominant colour, as `0xRRGGBB`. The most saturated pixels -
/// the artwork's real palette - are averaged and pulled back toward the whole
/// image's mean so a small bright accent cannot swamp a dark cover. Premultiplied
/// BGRA in, one RGB triple out; `None` for an empty or all-transparent sleeve.
fn dominant_colour(width: u32, height: u32, pixels: &[u8]) -> Option<u32> {
    let (width, height) = (width as usize, height as usize);
    if width == 0 || height == 0 || pixels.len() < width * height * 4 {
        return None;
    }
    let (mut vr, mut vg, mut vb, mut vn) = (0u64, 0u64, 0u64, 0u64);
    let (mut ar, mut ag, mut ab) = (0u64, 0u64, 0u64);
    let mut count = 0u64;
    for pixel in pixels.chunks_exact(4) {
        let (b, g, r, a) = (pixel[0] as u64, pixel[1] as u64, pixel[2] as u64, pixel[3] as u64);
        if a < 40 {
            continue;
        }
        ar += r;
        ag += g;
        ab += b;
        count += 1;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        if max >= 24 && max - min >= 48 {
            vr += r;
            vg += g;
            vb += b;
            vn += 1;
        }
    }
    if count == 0 {
        return None;
    }
    let (mr, mg, mb) = (ar / count, ag / count, ab / count);
    let (r, g, b) = if vn > 0 {
        let (hr, hg, hb) = (vr / vn, vg / vn, vb / vn);
        (
            (hr * 72 + mr * 28) / 100,
            (hg * 72 + mg * 28) / 100,
            (hb * 72 + mb * 28) / 100,
        )
    } else {
        (mr, mg, mb)
    };
    Some(((r >> 8) << 16 | (g >> 8) << 8 | (b >> 8)) as u32 & 0xFFFFFF)
}
