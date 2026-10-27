//! The preferences window.
//!
//! The script built this out of a second XAML window: a 600 by 720 rounded
//! panel with a header, six scrollable pages and a six-button bar along the
//! bottom. Here it is a second layered window of its own class, painted by the
//! same Direct2D software target as the island, and it only repaints when
//! something has changed.
//!
//! The two windows talk through `WM_APP` messages: the island learns that the
//! settings file changed, that a preview alert should be raised, or that the
//! clipboard history should be emptied.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::Duration;

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::AlertKind;
use crate::bot;
use crate::icons;
use crate::render::Renderer;
use crate::settings::{config_dir, Settings};
use crate::ui::{Align, Canvas, Icon, Rect, BOLD, NORMAL, SEMI};

/// The island learns the settings file changed.
pub const WM_ISLE_SETTINGS: u32 = WM_APP + 1;
/// The island should show one of its cards as a preview; the kind is in `wp`.
pub const WM_ISLE_ALERT: u32 = WM_APP + 2;
/// The island should empty its clipboard history.
pub const WM_ISLE_CLEAR: u32 = WM_APP + 3;

/// `WM_MOUSELEAVE`. The crate's metadata omits it, so it is spelled here, the
/// same way the island does.
const WM_LEAVE: u32 = 0x02A3;

/// The window, in logical pixels: a macOS-style settings panel with a category
/// sidebar down the left and a detail pane on the right.
const W: f32 = 680.0;
const H: f32 = 640.0;
/// The Border around everything, `Margin="20"`.
const MARGIN: f32 = 20.0;
/// The left column that lists the categories.
const SIDEBAR_W: f32 = 200.0;
/// The gap above the detail pane's first heading, chosen so it lines up with
/// the sidebar's first row (both text tops land at shell.y + 40).
const HEADER_H: f32 = 28.0;
/// A sidebar row and the gap under it.
const SIDE_ROW_H: f32 = 38.0;
const SIDE_ROW_GAP: f32 = 4.0;
/// The scrollable detail area, to the right of the sidebar and below the title.
const CONTENT_X: f32 = MARGIN + SIDEBAR_W + 20.0;
const CONTENT_Y: f32 = MARGIN + HEADER_H;
const CONTENT_W: f32 = W - CONTENT_X - MARGIN - 12.0;
const CONTENT_H: f32 = H - MARGIN - CONTENT_Y;
/// A row with a 13 px title and an 11 px line under it, `Padding="14,11"`.
const ROW_H: f32 = 52.0;

const BG: u32 = 0x242424;
const CARD: u32 = 0x2E2E2E;
const EDGE: u32 = 0x383838;
const WHITE: u32 = 0xFFFFFF;
const DIM: u32 = 0x9A9996;
const FAINT: u32 = 0xE0E0E0;
const PILL: u32 = 0x383838;
const TRACK: u32 = 0x424242;
const BLUE: u32 = 0x3584E4;
const RED: u32 = 0xFF6F61;
const BLACK: u32 = 0x000000;

/// The sidebar's categories: a name, the glyph it wears, and the colour of the
/// little tile behind that glyph, in the manner of System Settings.
const CATEGORIES: [(&str, Icon, u32); 6] = [
    ("Island", Icon::Sparkle, 0x5E5CE6),
    ("Clipboard", Icon::File, 0xFF9F0A),
    ("Focus Timer", Icon::Timer, 0xFF453A),
    ("Assistant", Icon::Mic, 0xBF5AF2),
    ("AI Usage", Icon::Gauge, 0x30D158),
    ("About", Icon::Gear, 0x8E8E93),
];

/// Everything the window can be asked to do. Small and copyable, so a click
/// can be resolved before anything is borrowed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Act {
    Page(usize),
    Close,
    Toggle(&'static str),
    Step(&'static str, i32),
    Menu(&'static str),
    Pick(usize),
    Demo(AlertKind),
    ClearClips,
    ResetAll,
    Shape(usize),
    Colour(usize),
}

struct Spot {
    rect: Rect,
    act: Act,
}

/// An open dropdown: what it sets, what it offers, which item is current, and
/// where its button sits so the list can hang below it.
struct MenuState {
    key: &'static str,
    options: Vec<(String, String)>,
    current: usize,
    anchor: Rect,
}

/// A row inside a card. Every control the script had is one of these.
enum Cell {
    Toggle {
        title: &'static str,
        sub: &'static str,
        key: &'static str,
        on: bool,
    },
    Stepper {
        title: &'static str,
        sub: &'static str,
        key: &'static str,
        value: String,
        live: bool,
    },
    Picker {
        title: &'static str,
        sub: &'static str,
        key: &'static str,
        label: String,
    },
    Demo {
        title: &'static str,
        sub: &'static str,
        kind: AlertKind,
        switch: Switch,
    },
    Button {
        title: &'static str,
        sub: &'static str,
        label: &'static str,
        act: Act,
        fill: u32,
        ink: u32,
    },
    Plain {
        title: &'static str,
        sub: &'static str,
        value: String,
    },
    /// A switch the script drew but never bound to anything.
    Inert {
        title: &'static str,
        sub: &'static str,
        on: bool,
    },
    /// A pill the script drew without a handler.
    InertButton {
        title: &'static str,
        sub: &'static str,
        label: &'static str,
    },
}

enum Switch {
    Bound(&'static str),
    Fixed(bool),
}

pub struct Prefs {
    hwnd: HWND,
    island: HWND,
    renderer: Renderer,
    settings: Settings,
    path: PathBuf,
    page: usize,
    scroll: [f32; 6],
    content_h: [f32; 6],
    menu: Option<MenuState>,
    /// Dropdown open progress (0..1) and whether it is animating shut.
    menu_anim: f32,
    menu_closing: bool,
    /// Eased on/off value (0..1) per bound switch, for the toggle animation.
    switch_anim: Vec<(&'static str, f32)>,
    /// Chosen index for pickers not yet wired to a persisted setting.
    extra: Vec<(&'static str, usize)>,
    spots: Vec<Spot>,
    mouse: (f32, f32),
    /// Which spot the pointer is over, plus one; zero for none. Kept so a
    /// mouse move only costs a repaint when the highlight actually changes.
    hover: usize,
    close: Rect,
    body: Rect,
    dirty: bool,
}

/// The one window, so the island's gear can open it without threading a
/// handle through everything.
static HANDLE: AtomicIsize = AtomicIsize::new(0);
/// A private message the preferences clock posts to drive the dropdown and
/// switch animations. `WM_APP + 20`, clear of the island's `WM_APP + 10` frame
/// tick and the `WM_ISLE_*` notices above.
const WM_APP_PREFS_TICK: u32 = WM_APP + 20;
/// Set while an animation is running; the clock only posts while it is, so an
/// idle or hidden window costs nothing.
static ANIMATING: AtomicBool = AtomicBool::new(false);

/// Opens the window, or brings it back to the front if it is already up.
pub fn show() {
    let raw = HANDLE.load(Ordering::Relaxed);
    if raw == 0 {
        return;
    }
    let hwnd = HWND(raw as *mut core::ffi::c_void);
    unsafe {
        // Re-read the file: the island or another Isle window may have saved
        // something since this one was last looked at.
        if let Some(prefs) = state(hwnd) {
            prefs.settings = Settings::load(&prefs.path);
            prefs.dirty = true;
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            prefs.flush();
            return;
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

unsafe fn state(hwnd: HWND) -> Option<&'static mut Prefs> {
    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Prefs).as_mut()
}

/// Creates the window, hidden, and parks its state on it.
pub fn create(island: HWND) {
    unsafe {
        let scale = GetDpiForSystem() as f32 / 96.0;
        let width_px = (W * scale).round() as i32;
        let height_px = (H * scale).round() as i32;
        let instance = match GetModuleHandleW(None) {
            Ok(module) => HINSTANCE(module.0),
            Err(_) => return,
        };
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: w!("IslePrefs"),
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return;
        }
        let x = (GetSystemMetrics(SM_CXSCREEN) - width_px) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - height_px) / 2;
        let hwnd = match CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("IslePrefs"),
            w!("Isle"),
            WS_POPUP,
            x,
            y,
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
        let path = config_dir().join("settings.json");
        let prefs = Box::new(Prefs {
            hwnd,
            island,
            renderer,
            settings: Settings::load(&path),
            path,
            page: 0,
            scroll: [0.0; 6],
            content_h: [0.0; 6],
            menu: None,
            menu_anim: 0.0,
            menu_closing: false,
            switch_anim: Vec::new(),
            extra: Vec::new(),
            spots: Vec::new(),
            mouse: (-1.0, -1.0),
            hover: 0,
            close: Rect::new(0.0, 0.0, 0.0, 0.0),
            body: Rect::new(0.0, 0.0, 0.0, 0.0),
            dirty: true,
        });
        let pointer = Box::into_raw(prefs);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer as isize);
        HANDLE.store(hwnd.0 as isize, Ordering::Relaxed);
        // Painted once while hidden so the first show has no empty frame.
        if let Some(prefs) = state(hwnd) {
            prefs.paint();
        }
        // A light clock drives the dropdown and switch animations. It sleeps at
        // the frame interval and posts a private message the loop turns into one
        // `tick`, but only while `ANIMATING` is set, so an idle window is free.
        let raw = hwnd.0 as isize;
        std::thread::spawn(move || {
            let hwnd = HWND(raw as *mut core::ffi::c_void);
            loop {
                if ANIMATING.load(Ordering::Relaxed)
                    && PostMessageW(hwnd, WM_APP_PREFS_TICK, WPARAM(0), LPARAM(0)).is_err()
                {
                    ANIMATING.store(false, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(16));
            }
        });
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_LBUTTONDOWN => {
            if let Some(prefs) = state(hwnd) {
                let (x, y) = prefs.point(lp);
                // The script's DragMove ran everywhere except on a control
                // with a hand cursor, so a press on one has to stay a click.
                let control = prefs.spots.iter().any(|spot| spot.rect.contains(x, y));
                if prefs.body.contains(x, y) && !control {
                    let _ = ReleaseCapture();
                    SendMessageW(
                        hwnd,
                        WM_NCLBUTTONDOWN,
                        WPARAM(HTCAPTION as usize),
                        LPARAM(0),
                    );
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Some(prefs) = state(hwnd) {
                let (x, y) = prefs.point(lp);
                prefs.click(x, y);
                prefs.flush();
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if let Some(prefs) = state(hwnd) {
                let (x, y) = prefs.point(lp);
                prefs.mouse = (x, y);
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: u32::MAX,
                };
                let _ = TrackMouseEvent(&mut track);
                // The highlight is a whole row or pill, so nothing on screen
                // changes until the pointer crosses into another one.
                let hover = prefs.hover_at(x, y);
                if hover != prefs.hover {
                    prefs.hover = hover;
                    prefs.dirty = true;
                }
                prefs.mouse = (x, y);
                prefs.flush();
            }
            LRESULT(0)
        }
        WM_LEAVE => {
            if let Some(prefs) = state(hwnd) {
                prefs.mouse = (-1.0, -1.0);
                prefs.hover = 0;
                prefs.dirty = true;
                prefs.flush();
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            if let Some(prefs) = state(hwnd) {
                let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
                prefs.wheel(delta);
                prefs.flush();
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let over = state(hwnd)
                .map(|prefs| prefs.over_control())
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
        WM_CLOSE => {
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_DESTROY => {
            HANDLE.store(0, Ordering::Relaxed);
            LRESULT(0)
        }
        m if m == WM_APP_PREFS_TICK => {
            if let Some(prefs) = state(hwnd) {
                let moving = prefs.tick();
                ANIMATING.store(moving, Ordering::Relaxed);
                prefs.flush();
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

impl Prefs {
    fn point(&self, lp: LPARAM) -> (f32, f32) {
        // The renderer scales logical units by the system DPI, so the raw
        // pixel coordinates have to come back down the same way.
        let scale = self.scale();
        (
            (lp.0 & 0xFFFF) as i16 as f32 / scale,
            ((lp.0 >> 16) & 0xFFFF) as i16 as f32 / scale,
        )
    }

    fn scale(&self) -> f32 {
        unsafe { GetDpiForSystem() as f32 / 96.0 }
    }

    fn over_control(&self) -> bool {
        let (x, y) = self.mouse;
        self.spots.iter().any(|spot| spot.rect.contains(x, y))
    }

    /// The spot under a point, plus one. Mirrors the reverse scan `click`
    /// uses, so what looks hovered is what a press would hit.
    fn hover_at(&self, x: f32, y: f32) -> usize {
        self.spots
            .iter()
            .rposition(|spot| spot.rect.contains(x, y))
            .map_or(0, |index| index + 1)
    }

    fn click(&mut self, x: f32, y: f32) {
        let hit = self
            .spots
            .iter()
            .rev()
            .find(|spot| spot.rect.contains(x, y))
            .map(|spot| spot.act);
        // With a dropdown open, the next press decides its fate: picking an
        // item applies and closes, pressing the same control again closes it
        // (a toggle), another control switches to that menu, and a press
        // anywhere else dismisses it. Every close animates out.
        if self.menu.is_some() && !self.menu_closing {
            match hit {
                Some(Act::Pick(index)) => self.pick(index),
                Some(Act::Menu(key)) => {
                    if self.menu.as_ref().map_or(false, |menu| menu.key == key) {
                        self.close_menu();
                    } else {
                        self.open_menu(key);
                    }
                }
                _ => self.close_menu(),
            }
            self.dirty = true;
            return;
        }
        if let Some(act) = hit {
            self.apply(act);
        }
    }

    fn wheel(&mut self, delta: i32) {
        let page = self.page;
        let max = (self.content_h[page] - CONTENT_H).max(0.0);
        self.scroll[page] = (self.scroll[page] - delta as f32 / 120.0 * 60.0).clamp(0.0, max);
        self.dirty = true;
    }

    fn save(&self) {
        self.settings.save(&self.path);
        unsafe {
            let _ = PostMessageW(self.island, WM_ISLE_SETTINGS, WPARAM(0), LPARAM(0));
        }
    }

    fn toggle(&mut self, key: &'static str) {
        // Seed the animator at the value on screen before the flip, so the
        // switch slides from old to new instead of snapping.
        let before = self.switch_value(key);
        let s = &mut self.settings;
        match key {
            "compact" => s.compact_top_bar = !s.compact_top_bar,
            "inbar" => s.island_in_top_bar = !s.island_in_top_bar,
            "volume" => s.alert_volume = !s.alert_volume,
            "charging" => s.alert_charging = !s.alert_charging,
            "network" => s.alert_network = !s.alert_network,
            "bluetooth" => s.alert_bluetooth = !s.alert_bluetooth,
            "caps" => s.alert_caps_lock = !s.alert_caps_lock,
            "dnd" => s.dnd = !s.dnd,
            _ => return,
        }
        self.set_switch_value(key, before);
        self.save();
        ANIMATING.store(true, Ordering::Relaxed);
    }

    fn step(&mut self, key: &str, direction: i32) {
        let s = &mut self.settings;
        match key {
            // The script's clamps, exactly: 24 to 54 by two, a rest of at
            // least 40 ms by twenty, and never fewer than one minute.
            "height" => s.island_height = (s.island_height + 2 * direction).clamp(24, 54),
            "rest" => s.rest_delay = ((s.rest_delay as i64 + 20 * direction as i64).max(40)) as u64,
            "focus" => {
                s.focus_work_min = ((s.focus_work_min as i64 + direction as i64).max(1)) as u32
            }
            "break" => {
                s.focus_break_min = ((s.focus_break_min as i64 + direction as i64).max(1)) as u32
            }
            _ => return,
        }
        self.save();
    }

    fn apply(&mut self, act: Act) {
        match act {
            Act::Page(index) => self.page = index,
            Act::Close => unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            },
            Act::Toggle(key) => self.toggle(key),
            Act::Step(key, direction) => self.step(key, direction),
            Act::Menu(key) => self.open_menu(key),
            Act::Pick(index) => self.pick(index),
            Act::Demo(kind) => unsafe {
                let _ = PostMessageW(self.island, WM_ISLE_ALERT, WPARAM(kind as usize), LPARAM(0));
            },
            Act::ClearClips => unsafe {
                let _ = PostMessageW(self.island, WM_ISLE_CLEAR, WPARAM(0), LPARAM(0));
            },
            Act::ResetAll => {
                // The script's second handler threw the whole hashtable away
                // and started again from the defaults.
                self.settings = Settings::default();
                self.save();
            }
            Act::Shape(index) => {
                if let Some(name) = bot::SHAPES.get(index) {
                    self.settings.assistant_shape = format!("Shape{}", capital(name));
                    self.save();
                }
            }
            Act::Colour(index) => {
                if let Some(name) = bot::COLOURS.get(index) {
                    self.settings.assistant_color = format!("Col{}", capital(name));
                    self.save();
                }
            }
        }
        self.dirty = true;
    }

    fn open_menu(&mut self, key: &'static str) {
        let (options, current) = self.menu_for(key);
        let anchor = self
            .spots
            .iter()
            .rev()
            .find(|spot| spot.act == Act::Menu(key))
            .map(|spot| spot.rect)
            .unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
        self.menu = Some(MenuState {
            key,
            options,
            current,
            anchor,
        });
        self.menu_anim = 0.0;
        self.menu_closing = false;
        ANIMATING.store(true, Ordering::Relaxed);
    }

    /// Starts the close animation; the menu is dropped once it finishes.
    fn close_menu(&mut self) {
        if self.menu.is_some() {
            self.menu_closing = true;
            ANIMATING.store(true, Ordering::Relaxed);
        }
    }

    /// Options and current index for a menu, covering both persisted settings
    /// and the session-only cosmetic pickers.
    fn menu_for(&self, key: &'static str) -> (Vec<(String, String)>, usize) {
        if let Some(list) = cosmetic_options(key) {
            let current = self.extra_index(key).min(list.len().saturating_sub(1));
            let options = list
                .iter()
                .map(|s| ((*s).to_string(), (*s).to_string()))
                .collect();
            return (options, current);
        }
        options_for(&self.settings, key)
    }

    fn extra_index(&self, key: &str) -> usize {
        self.extra
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, index)| *index)
            .unwrap_or(0)
    }

    fn set_extra(&mut self, key: &'static str, index: usize) {
        match self.extra.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = index,
            None => self.extra.push((key, index)),
        }
    }

    /// Advances the dropdown and switch animations one step, returning whether
    /// anything is still moving (so the clock keeps or stops ticking).
    fn tick(&mut self) -> bool {
        let mut moving = false;
        if self.menu.is_some() {
            let target = if self.menu_closing { 0.0 } else { 1.0 };
            let next = self.menu_anim + (target - self.menu_anim) * 0.30;
            if (next - target).abs() < 0.01 {
                self.menu_anim = target;
            } else {
                self.menu_anim = next;
                moving = true;
            }
            if self.menu_closing && self.menu_anim <= 0.01 {
                self.menu = None;
                self.menu_closing = false;
            }
        }
        for (key, value) in self.switch_anim.iter_mut() {
            let target = if bound_bool(&self.settings, key) { 1.0 } else { 0.0 };
            let next = *value + (target - *value) * 0.32;
            if (next - target).abs() < 0.01 {
                *value = target;
            } else {
                *value = next;
                moving = true;
            }
        }
        self.dirty = true;
        moving
    }

    /// The eased value a bound switch shows now, or its hard state if it has
    /// never animated (so opening the window shows no motion).
    fn switch_value(&self, key: &str) -> f32 {
        self.switch_anim
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, value)| *value)
            .unwrap_or(if bound_bool(&self.settings, key) { 1.0 } else { 0.0 })
    }

    fn set_switch_value(&mut self, key: &'static str, value: f32) {
        match self.switch_anim.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = value,
            None => self.switch_anim.push((key, value)),
        }
    }

    fn pick(&mut self, index: usize) {
        let (key, value) = match self
            .menu
            .as_ref()
            .and_then(|menu| menu.options.get(index).cloned().map(|(_, value)| (menu.key, value)))
        {
            Some(found) => found,
            None => return,
        };
        if cosmetic_options(key).is_some() {
            // Not wired to a persisted setting yet: remember it for the session.
            self.set_extra(key, index);
        } else {
            match key {
                "topbar" => self.settings.top_bar_style = value,
                "display" => self.settings.display_mode = value,
                "openmode" => self.settings.open_mode = value,
                "glow" => self.settings.glow_mode = value,
                "fps" => self.settings.target_fps = value.parse().unwrap_or(0.0),
                "ai" => {
                    self.settings.ai_provider = value;
                    // A model from the old provider would not exist in the new
                    // one, so the first is taken, as the script did.
                    let models = models_for(&self.settings.ai_provider);
                    self.settings.ai_model = models[0].to_string();
                }
                "model" => self.settings.ai_model = value,
                _ => {}
            }
            self.save();
        }
        self.close_menu();
    }
}

/// `"sky"` to `"Sky"`, for the `ColSky` / `ShapeSky` names in the file.
fn capital(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn models_for(provider: &str) -> &'static [&'static str] {
    match provider {
        "Claude Code" => &["Sonnet", "Opus", "Haiku"],
        "Codex" => &["GPT-4o", "o3-mini", "Codex default"],
        _ => &["Gemini Flash", "Gemini Pro", "Claude Sonnet"],
    }
}

fn provider_sub(provider: &str) -> &'static str {
    match provider {
        "Claude Code" => "What Claude answers with",
        "Codex" => "What Codex answers with",
        _ => "What Antigravity answers with",
    }
}

/// The items a dropdown offers, and which one is current.
fn options_for(settings: &Settings, key: &str) -> (Vec<(String, String)>, usize) {
    let (pairs, current): (Vec<(&str, &str)>, &str) = match key {
        "topbar" => (
            vec![
                ("Glass", "glass"),
                ("Solid", "solid"),
                ("Transparent", "transparent"),
            ],
            &settings.top_bar_style,
        ),
        "display" => (
            vec![
                ("Main display", "Main display"),
                ("Follow cursor", "Follow cursor"),
                ("All displays", "All displays"),
            ],
            &settings.display_mode,
        ),
        "openmode" => (
            vec![("On hover", "hover"), ("On click", "click")],
            &settings.open_mode,
        ),
        "glow" => (
            vec![
                ("Match", "Match"),
                ("Rainbow", "Rainbow"),
                ("Solid", "Solid"),
                ("Off", "Off"),
            ],
            &settings.glow_mode,
        ),
        "fps" => (
            vec![
                ("V-Sync (Auto)", "0"),
                ("60 FPS", "60"),
                ("120 FPS", "120"),
                ("144 FPS", "144"),
                ("240 FPS", "240"),
            ],
            "",
        ),
        "ai" => (
            vec![
                ("Antigravity", "Antigravity"),
                ("Claude Code", "Claude Code"),
                ("Codex", "Codex"),
            ],
            &settings.ai_provider,
        ),
        _ => {
            let models = models_for(&settings.ai_provider);
            let pairs = models.iter().map(|name| (*name, *name)).collect();
            (pairs, &settings.ai_model)
        }
    };
    let options: Vec<(String, String)> = pairs
        .into_iter()
        .map(|(label, value)| (label.to_string(), value.to_string()))
        .collect();
    // The frame rate is stored as a number, the rest as the string the menus
    // themselves wrote.
    let current_index = if key == "fps" {
        let wanted = format!("{}", settings.target_fps as i64);
        options
            .iter()
            .position(|(_, value)| *value == wanted)
            .unwrap_or(0)
    } else {
        options
            .iter()
            .position(|(_, value)| value == current)
            .unwrap_or(0)
    };
    (options, current_index)
}

/// The label a picker shows for the current setting.
fn picker_label(settings: &Settings, key: &str) -> String {
    let (options, index) = options_for(settings, key);
    options
        .get(index)
        .map(|(label, _)| label.clone())
        .unwrap_or_default()
}

/// The boolean a bound switch reflects, shared by the animator and the drawing.
fn bound_bool(settings: &Settings, key: &str) -> bool {
    match key {
        "compact" => settings.compact_top_bar,
        "inbar" => settings.island_in_top_bar,
        "volume" => settings.alert_volume,
        "charging" => settings.alert_charging,
        "network" => settings.alert_network,
        "bluetooth" => settings.alert_bluetooth,
        "caps" => settings.alert_caps_lock,
        "dnd" => settings.dnd,
        _ => false,
    }
}

/// Option lists for pickers not yet wired to a persisted setting. The first
/// entry is the value the row used to show, so the default matches.
fn cosmetic_options(key: &str) -> Option<Vec<&'static str>> {
    Some(match key {
        "speech" => vec!["Small (265 MB)", "Medium (1.1 GB)", "Large (2.9 GB)"],
        "language" => vec!["Any (detected)", "Indonesian", "English", "Japanese", "Mandarin"],
        "keepconv" => vec!["Until midnight", "For 1 hour", "For 8 hours", "Do not keep"],
        "warnat" => vec!["80%", "50%", "90%"],
        "warnagain" => vec!["95%", "90%", "99%"],
        _ => return None,
    })
}

/// The state the drawing functions need, kept apart from the window so the
/// canvas can borrow the renderer while the pages read the settings.
struct Ctx<'a> {
    settings: &'a Settings,
    page: usize,
    scroll: f32,
    menu: Option<&'a MenuState>,
    menu_anim: f32,
    switch_anim: &'a [(&'static str, f32)],
    extra: &'a [(&'static str, usize)],
    mouse: (f32, f32),
    spots: &'a mut Vec<Spot>,
    close: &'a mut Rect,
    body: &'a mut Rect,
}

impl<'a> Ctx<'a> {
    fn spot(&mut self, rect: Rect, act: Act) {
        if rect.w > 0.0 && rect.h > 0.0 {
            self.spots.push(Spot { rect, act });
        }
    }

    fn hot(&self, rect: Rect) -> bool {
        rect.contains(self.mouse.0, self.mouse.1)
    }

    fn draw(&mut self, c: &mut Canvas) -> f32 {
        let shell = Rect::new(MARGIN, MARGIN, W - 2.0 * MARGIN, H - 2.0 * MARGIN);
        *self.body = shell;
        // The drop shadow of the script is left out: a layered window has no
        // room for one, and painting it would double the window's area.
        c.rect(shell, 18.0, BG);
        c.stroke_rect(shell, 18.0, EDGE, 1.0);

        // The traffic lights, top-left, as System Settings has them. Only the
        // red one is wired: it hides the window, the way the old close did.
        let light_y = shell.y + 18.0;
        c.circle(shell.x + 20.0, light_y, 6.0, 0xFF5F57);
        c.circle(shell.x + 40.0, light_y, 6.0, 0xFEBC2E);
        c.circle(shell.x + 60.0, light_y, 6.0, 0x28C840);
        let close = Rect::new(shell.x + 12.0, light_y - 8.0, 16.0, 16.0);
        self.spot(close, Act::Close);
        *self.close = close;

        // The category sidebar down the left, with a hairline beside it.
        self.sidebar(c, shell);
        c.rect(
            Rect::new(shell.x + SIDEBAR_W, shell.y + 12.0, 1.0, shell.h - 24.0),
            0.0,
            EDGE,
        );

        // The detail pane, clipped to its own area.
        let view = Rect::new(CONTENT_X, CONTENT_Y, CONTENT_W, CONTENT_H);
        c.clip(view);
        let height = self.page_content(c, view);
        c.unclip();
        self.scrollbar(c, view, height);
        self.menu_popover(c);
        height
    }

    fn page_content(&mut self, c: &mut Canvas, view: Rect) -> f32 {
        let mut y = view.y + 6.0 - self.scroll;
        match self.page {
            0 => self.island_page(c, view, &mut y),
            1 => self.clipboard_page(c, view, &mut y),
            2 => self.focus_page(c, view, &mut y),
            3 => self.assistant_page(c, view, &mut y),
            4 => self.usage_page(c, view, &mut y),
            _ => self.about_page(c, view, &mut y),
        }
        (y + self.scroll - view.y + 8.0).max(CONTENT_H)
    }

    fn scrollbar(&self, c: &mut Canvas, view: Rect, height: f32) {
        if height <= view.h + 0.5 {
            return;
        }
        let track = Rect::new(view.right() + 4.0, view.y, 4.0, view.h);
        c.rect(track, 2.0, 0x2E2E2E);
        let travel = (view.h - view.h * (view.h / height)).max(0.0);
        let offset = if height > view.h {
            (self.scroll / (height - view.h)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let thumb = Rect::new(
            track.x,
            track.y + offset * travel,
            4.0,
            (view.h * (view.h / height)).max(24.0),
        );
        c.rect(thumb, 2.0, 0x48484A);
    }

    /// The vertical category list down the left: a coloured glyph tile and a
    /// label, the selected row wearing the accent, as System Settings does.
    fn sidebar(&mut self, c: &mut Canvas, shell: Rect) {
        let row_x = shell.x + 10.0;
        let row_w = SIDEBAR_W - 20.0;
        let mut y = shell.y + 40.0;
        for (index, (label, icon, tint)) in CATEGORIES.iter().enumerate() {
            let row = Rect::new(row_x, y, row_w, SIDE_ROW_H);
            let active = index == self.page;
            let over = self.hot(row);
            if active {
                c.rect(row, 8.0, BLUE);
            } else if over {
                c.rect_a(row, 8.0, WHITE, 0.08);
            }
            let tile = Rect::new(row.x + 7.0, row.y + (row.h - 24.0) / 2.0, 24.0, 24.0);
            c.rect(tile, 6.0, *tint);
            c.icon(*icon, tile.x + 12.0, tile.y + 12.0, 14.0, WHITE, 1.6);
            c.text(
                label,
                Rect::new(
                    tile.right() + 9.0,
                    row.y,
                    row.right() - tile.right() - 13.0,
                    row.h,
                ),
                13.0,
                if active { SEMI } else { NORMAL },
                WHITE,
                Align::Left,
            );
            self.spot(row, Act::Page(index));
            y += SIDE_ROW_H + SIDE_ROW_GAP;
        }
    }

    // ---- cards ------------------------------------------------------------

    fn card(&mut self, c: &mut Canvas, view: Rect, y: &mut f32, cells: &[Cell]) {
        let card = Rect::new(view.x, *y + 4.0, view.w - 8.0, cells.len() as f32 * ROW_H);
        c.rect(card, 12.0, CARD);
        let mut row_y = card.y;
        for (index, cell) in cells.iter().enumerate() {
            let rect = Rect::new(card.x, row_y, card.w, ROW_H);
            if index + 1 < cells.len() {
                c.rect(
                    Rect::new(rect.x + 14.0, rect.bottom() - 1.0, rect.w - 28.0, 1.0),
                    0.0,
                    EDGE,
                );
            }
            self.cell(c, rect, cell);
            row_y += ROW_H;
        }
        *y = card.bottom() + 16.0;
    }

    fn cell(&mut self, c: &mut Canvas, rect: Rect, cell: &Cell) {
        let (title, sub) = match cell {
            Cell::Toggle { title, sub, .. }
            | Cell::Stepper { title, sub, .. }
            | Cell::Picker { title, sub, .. }
            | Cell::Demo { title, sub, .. }
            | Cell::Button { title, sub, .. }
            | Cell::Plain { title, sub, .. }
            | Cell::Inert { title, sub, .. }
            | Cell::InertButton { title, sub, .. } => (*title, *sub),
        };
        let pad = 14.0;
        c.text(
            title,
            Rect::new(
                rect.x + pad,
                rect.y + 11.0,
                rect.w - 2.0 * pad - 120.0,
                16.0,
            ),
            13.0,
            NORMAL,
            WHITE,
            Align::Left,
        );
        if !sub.is_empty() {
            c.text(
                sub,
                Rect::new(
                    rect.x + pad,
                    rect.y + 27.0,
                    rect.w - 2.0 * pad - 160.0,
                    14.0,
                ),
                11.0,
                NORMAL,
                DIM,
                Align::Left,
            );
        }

        match cell {
            Cell::Toggle { key, on, .. } => {
                self.switch(c, rect, *on, Some(Act::Toggle(key)));
            }
            Cell::Stepper {
                key, value, live, ..
            } => {
                self.stepper(c, rect, key, value, *live);
            }
            Cell::Picker { key, label, .. } => {
                self.picker(c, rect, key, label);
            }
            Cell::Demo { kind, switch, .. } => {
                self.spot(rect, Act::Demo(*kind));
                match switch {
                    Switch::Bound(key) => {
                        let on = self.settings_bool(key);
                        self.switch(c, rect, on, Some(Act::Toggle(key)));
                    }
                    Switch::Fixed(on) => self.switch(c, rect, *on, None),
                }
            }
            Cell::Button {
                label,
                act,
                fill,
                ink,
                ..
            } => {
                self.button(c, rect, label, *act, *fill, *ink);
            }
            Cell::InertButton { label, .. } => {
                self.button(c, rect, label, Act::Page(self.page), PILL, WHITE);
                // Drawn, but the script never gave it a handler; drop the
                // spot so a click falls through to the row behind it.
                self.spots.pop();
            }
            Cell::Plain { value, .. } => {
                c.text(
                    value,
                    Rect::new(rect.right() - 114.0, rect.y, 100.0, rect.h),
                    12.0,
                    NORMAL,
                    WHITE,
                    Align::Right,
                );
            }
            Cell::Inert { on, .. } => self.switch(c, rect, *on, None),
        }
    }

    fn settings_bool(&self, key: &str) -> bool {
        bound_bool(self.settings, key)
    }

    fn extra_index(&self, key: &str) -> usize {
        self.extra
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, index)| *index)
            .unwrap_or(0)
    }

    /// The label a picker row shows, from the setting or a session-only choice.
    fn picker_text(&self, key: &str) -> String {
        if let Some(list) = cosmetic_options(key) {
            return list
                .get(self.extra_index(key))
                .map(|s| (*s).to_string())
                .unwrap_or_default();
        }
        picker_label(self.settings, key)
    }

    fn switch(&mut self, c: &mut Canvas, rect: Rect, on: bool, act: Option<Act>) {
        let track = Rect::new(
            rect.right() - 14.0 - 44.0,
            rect.y + (ROW_H - 24.0) / 2.0,
            44.0,
            24.0,
        );
        // Bound switches slide to their eased value; the track fades grey to
        // blue and the thumb travels with it.
        let v = match act {
            Some(Act::Toggle(key)) => self
                .switch_anim
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, value)| *value)
                .unwrap_or(if on { 1.0 } else { 0.0 }),
            _ => {
                if on {
                    1.0
                } else {
                    0.0
                }
            }
        };
        c.rect(track, 12.0, TRACK);
        c.rect_a(track, 12.0, BLUE, v);
        let left = track.x + 3.0;
        let right = track.right() - 3.0 - 18.0;
        let thumb = left + (right - left) * v;
        c.circle(thumb + 9.0, track.y + 12.0, 9.0, WHITE);
        if let Some(act) = act {
            self.spot(
                Rect::new(track.x - 4.0, track.y - 4.0, track.w + 8.0, track.h + 8.0),
                act,
            );
        }
    }

    fn stepper(&mut self, c: &mut Canvas, rect: Rect, key: &'static str, value: &str, live: bool) {
        let pill = Rect::new(
            rect.right() - 14.0 - 86.0,
            rect.y + (ROW_H - 26.0) / 2.0,
            86.0,
            26.0,
        );
        c.text(
            value,
            Rect::new(pill.x - 56.0, rect.y, 46.0, rect.h),
            12.0,
            NORMAL,
            WHITE,
            Align::Right,
        );
        c.rect(pill, 8.0, PILL);
        c.rect(
            Rect::new(pill.x + 43.0, pill.y + 3.0, 1.0, 20.0),
            0.0,
            0x282828,
        );
        c.text(
            "−",
            Rect::new(pill.x, pill.y, 43.0, pill.h),
            13.0,
            NORMAL,
            WHITE,
            Align::Center,
        );
        c.text(
            "+",
            Rect::new(pill.x + 44.0, pill.y, 42.0, pill.h),
            13.0,
            NORMAL,
            WHITE,
            Align::Center,
        );
        if live {
            self.spot(Rect::new(pill.x, pill.y, 43.0, pill.h), Act::Step(key, -1));
            self.spot(
                Rect::new(pill.x + 44.0, pill.y, 42.0, pill.h),
                Act::Step(key, 1),
            );
        }
    }

    fn picker(&mut self, c: &mut Canvas, rect: Rect, key: &'static str, label: &str) {
        let wide = label.chars().count() as f32 * 6.2 + 24.0 + 13.0;
        let pill = Rect::new(
            rect.right() - 14.0 - wide,
            rect.y + (ROW_H - 24.0) / 2.0,
            wide,
            24.0,
        );
        c.rect(pill, 8.0, PILL);
        c.text(
            label,
            Rect::new(pill.x + 12.0, pill.y, pill.w - 25.0, pill.h),
            12.0,
            NORMAL,
            WHITE,
            Align::Left,
        );
        // The script's picker chevron: `M0 1.5 L3.5 5 L7 1.5` in #9A9996,
        // round-capped, at its natural 7 x 3.5 size.
        c.r.path_icon(
            pill.right() - 15.5,
            pill.y + 12.0,
            7.0,
            icons::CHEVRON_CHOICE,
            0x9A9996,
            false,
            1.5,
            true,
            0.0,
        );
        self.spot(pill, Act::Menu(key));
    }

    fn button(&mut self, c: &mut Canvas, rect: Rect, label: &str, act: Act, fill: u32, ink: u32) {
        let wide = label.chars().count() as f32 * 6.4 + 28.0;
        let pill = Rect::new(
            rect.right() - 14.0 - wide,
            rect.y + (ROW_H - 26.0) / 2.0,
            wide,
            26.0,
        );
        c.rect(pill, 8.0, fill);
        c.text(label, pill, 12.0, NORMAL, ink, Align::Center);
        self.spot(pill, act);
    }

    fn menu_popover(&mut self, c: &mut Canvas) {
        let Some(menu) = self.menu.as_ref() else {
            return;
        };
        let options = menu.options.clone();
        let current = menu.current;
        let anchor = menu.anchor;
        let item_h = 30.0;
        let widest = options
            .iter()
            .map(|(label, _)| label.chars().count())
            .max()
            .unwrap_or(0) as f32
            * 6.4
            + 46.0;
        let wide = widest.max(anchor.w).min(300.0);
        let high = options.len() as f32 * item_h + 8.0;
        let x = (anchor.right() - wide).max(CONTENT_X);
        let below = anchor.bottom() + 4.0;
        let y = if below + high > CONTENT_Y + CONTENT_H {
            (anchor.y - high - 4.0).max(MARGIN + HEADER_H)
        } else {
            below
        };
        let panel = Rect::new(x, y, wide, high);
        // Reveal the list from its anchor: clip to a growing rect and fade it
        // in, easing out so it settles fast. `menu_anim` is 1 when fully open,
        // so an idle dropdown is drawn whole.
        let eased = 1.0 - (1.0 - self.menu_anim) * (1.0 - self.menu_anim);
        let grown = Rect::new(x, y, wide, (high * eased).max(0.0));
        c.clip(grown);
        let layer = c.fade;
        c.fade = layer * eased.max(0.05);
        c.rect(panel, 10.0, 0x2B2B2B);
        c.stroke_rect(panel, 10.0, 0x3D3D3D, 1.0);
        for (index, (label, _)) in options.iter().enumerate() {
            let item = Rect::new(
                panel.x + 4.0,
                panel.y + 4.0 + index as f32 * item_h,
                panel.w - 8.0,
                item_h,
            );
            if self.hot(item) {
                c.rect(item, 6.0, PILL);
            }
            if index == current {
                c.text(
                    "✓",
                    Rect::new(item.x + 8.0, item.y, 14.0, item.h),
                    11.0,
                    BOLD,
                    BLUE,
                    Align::Left,
                );
            }
            c.text(
                label,
                Rect::new(item.x + 26.0, item.y, item.w - 32.0, item.h),
                11.5,
                NORMAL,
                WHITE,
                Align::Left,
            );
            self.spot(item, Act::Pick(index));
        }
        c.fade = layer;
        c.unclip();
    }
}

fn group_title(c: &mut Canvas, view: Rect, y: &mut f32, text: &str) {
    c.text(
        text,
        Rect::new(view.x + 6.0, *y + 6.0, view.w - 12.0, 18.0),
        13.0,
        BOLD,
        WHITE,
        Align::Left,
    );
    *y += 26.0;
}

fn group_sub(c: &mut Canvas, view: Rect, y: &mut f32, text: &str, lines: u32) {
    let high = lines as f32 * 15.0;
    c.text_wrap(
        text,
        Rect::new(view.x + 6.0, *y - 2.0, view.w - 20.0, high),
        11.0,
        NORMAL,
        DIM,
        15.0,
    );
    *y += high + 6.0;
}

// ---- pages ----------------------------------------------------------------

impl<'a> Ctx<'a> {
    fn island_page(&mut self, c: &mut Canvas, view: Rect, y: &mut f32) {
        group_title(c, view, y, "Appearance");
        let cells = vec![
            Cell::Picker {
                title: "Top bar",
                sub: "Glass shows the island as a notch, like on a Mac",
                key: "topbar",
                label: picker_label(self.settings, "topbar"),
            },
            Cell::Toggle {
                title: "Compact top bar",
                sub: "A slimmer top bar that frames the island like a bezel",
                key: "compact",
                on: self.settings.compact_top_bar,
            },
            Cell::Toggle {
                title: "Island inside the top bar",
                sub: "Like a notch: the closed island never covers windows",
                key: "inbar",
                on: self.settings.island_in_top_bar,
            },
            Cell::Stepper {
                title: "Island height",
                sub: "How far the closed island hangs below the top bar, in pixels",
                key: "height",
                value: self.settings.island_height.to_string(),
                live: true,
            },
            Cell::Picker {
                title: "Display",
                sub: "With more than one display, where the island is",
                key: "display",
                label: picker_label(self.settings, "display"),
            },
            Cell::Picker {
                title: "Open",
                sub: "How the closed island opens",
                key: "openmode",
                label: picker_label(self.settings, "openmode"),
            },
            Cell::Stepper {
                title: "Rest before opening",
                sub: "In milliseconds",
                key: "rest",
                value: self.settings.rest_delay.to_string(),
                live: true,
            },
            Cell::Picker {
                title: "Animation frame rate (FPS)",
                sub: "Hardware spring animation refresh rate",
                key: "fps",
                label: picker_label(self.settings, "fps"),
            },
        ];
        self.card(c, view, y, &cells);

        group_title(c, view, y, "Alerts");
        let cells = vec![
            Cell::Demo {
                title: "Volume and brightness",
                sub: "Show them in the island instead of the centre of the screen",
                kind: AlertKind::Mini,
                switch: Switch::Bound("volume"),
            },
            Cell::Demo {
                title: "Charging and battery",
                sub: "Plugged in, unplugged, full and low",
                kind: AlertKind::Charging,
                switch: Switch::Bound("charging"),
            },
            Cell::Toggle {
                title: "Network",
                sub: "When the connection drops",
                key: "network",
                on: self.settings.alert_network,
            },
            Cell::Toggle {
                title: "Bluetooth",
                sub: "Devices connecting, with their battery",
                key: "bluetooth",
                on: self.settings.alert_bluetooth,
            },
            Cell::Toggle {
                title: "Caps Lock",
                sub: "When it turns on or off",
                key: "caps",
                on: self.settings.alert_caps_lock,
            },
            Cell::Demo {
                title: "Calendar events",
                sub: "Before each event with a time, with Join for a Meet, Zoom or Teams link",
                kind: AlertKind::Meeting,
                switch: Switch::Fixed(true),
            },
            Cell::Stepper {
                title: "Minutes before an event",
                sub: "",
                key: "meeting",
                value: "5".into(),
                live: false,
            },
            Cell::Demo {
                title: "Windows' notification banners",
                sub: "Show notifications as Windows does instead of in the island",
                kind: AlertKind::Message,
                switch: Switch::Fixed(false),
            },
            Cell::InertButton {
                title: "Win+V",
                sub: "Windows' own clipboard history, separate from Isle's",
                label: "Isle's clipboard history",
            },
            Cell::Demo {
                title: "Privacy indicators",
                sub: "Dots when the camera, microphone or screen sharing is in use",
                kind: AlertKind::Permission,
                switch: Switch::Fixed(true),
            },
            Cell::Inert {
                title: "New song",
                sub: "Briefly show the song when the track changes",
                on: true,
            },
            Cell::Inert {
                title: "Lyrics",
                sub: "Look up the playing song's lyrics on LRCLIB",
                on: true,
            },
        ];
        self.card(c, view, y, &cells);
    }

    fn clipboard_page(&mut self, c: &mut Canvas, view: Rect, y: &mut f32) {
        group_title(c, view, y, "Clipboard History");
        group_sub(
            c,
            view,
            y,
            "Everything you copy is kept here, on this computer.",
            1,
        );
        let cells = vec![
            Cell::Inert {
                title: "Keep clipboard history",
                sub: "Text, links, images and files you copy. Passwords from password managers are never kept.",
                on: true,
            },
            Cell::Button {
                title: "Clear history",
                sub: "Pinned items are kept",
                label: "Clear",
                act: Act::ClearClips,
                fill: PILL,
                ink: WHITE,
            },
        ];
        self.card(c, view, y, &cells);

        group_title(c, view, y, "Never Keep Copies From");
        group_sub(
            c,
            view,
            y,
            "Known password managers are on the list from the start.",
            1,
        );
        let cells = vec![
            Cell::Plain {
                title: "Passwords and Keys",
                sub: "",
                value: "—".into(),
            },
            Cell::InertButton {
                title: "Add an app",
                sub: "",
                label: "Choose",
            },
        ];
        self.card(c, view, y, &cells);
    }

    fn focus_page(&mut self, c: &mut Canvas, view: Rect, y: &mut f32) {
        group_title(c, view, y, "While Focusing");
        let cells = vec![Cell::Toggle {
            title: "Do Not Disturb",
            sub: "Silence notifications during focus sessions, and restore them afterwards",
            key: "dnd",
            on: self.settings.dnd,
        }];
        self.card(c, view, y, &cells);

        group_title(c, view, y, "Pomodoro");
        group_sub(
            c,
            view,
            y,
            "Focus sessions with short breaks, and a long break after a few sessions.",
            1,
        );
        let cells = vec![
            Cell::Stepper {
                title: "Focus (minutes)",
                sub: "",
                key: "focus",
                value: self.settings.focus_work_min.to_string(),
                live: true,
            },
            Cell::Stepper {
                title: "Short break (minutes)",
                sub: "",
                key: "break",
                value: self.settings.focus_break_min.to_string(),
                live: true,
            },
            Cell::Stepper {
                title: "Long break (minutes)",
                sub: "",
                key: "long",
                value: "15".into(),
                live: false,
            },
            Cell::Stepper {
                title: "Sessions before a long break",
                sub: "",
                key: "sessions",
                value: "4".into(),
                live: false,
            },
            Cell::Inert {
                title: "Start breaks automatically",
                sub: "",
                on: true,
            },
            Cell::Inert {
                title: "Start focus automatically",
                sub: "After a break ends",
                on: false,
            },
        ];
        self.card(c, view, y, &cells);
    }
}

// ---- assistant ------------------------------------------------------------

impl<'a> Ctx<'a> {
    fn assistant_page(&mut self, c: &mut Canvas, view: Rect, y: &mut f32) {
        group_title(c, view, y, "Look");
        group_sub(
            c,
            view,
            y,
            "How the assistant looks, and the light it brings to the island.",
            1,
        );
        self.preview(c, view, y);
        self.look_card(c, view, y);

        group_title(c, view, y, "Voice Assistant");
        group_sub(
            c,
            view,
            y,
            "Press the shortcut or the mic and speak. Words are recognised on this PC; the question goes to the AI you choose, using your own sign-in.",
            2,
        );
        let provider = self.settings.ai_provider.clone();
        let cells = vec![
            Cell::Plain {
                title: "Voice assistant",
                sub: "The shortcut and the mic in the open island",
                value: String::new(),
            },
            Cell::Plain {
                title: "Shortcut",
                sub: "Opens the assistant and listens",
                value: "Super + I".into(),
            },
            Cell::Picker {
                title: "AI",
                sub: "Answers with your own sign-in; Isle runs its actions",
                key: "ai",
                label: provider.clone(),
            },
            Cell::Picker {
                title: "Model",
                sub: provider_sub(&provider),
                key: "model",
                label: self.settings.ai_model.clone(),
            },
            Cell::Picker {
                title: "Speech recognition",
                sub: "Bigger is more accurate, and slower; downloaded once",
                key: "speech",
                label: self.picker_text("speech"),
            },
            Cell::Picker {
                title: "Language",
                sub: "The language you speak",
                key: "language",
                label: self.picker_text("language"),
            },
            Cell::Picker {
                title: "Keep the conversation",
                sub: "Never more than a day; then it is deleted, with the AI's copy",
                key: "keepconv",
                label: self.picker_text("keepconv"),
            },
            Cell::InertButton {
                title: "Clear the conversation now",
                sub: "Deletes it here and the AI's copy of it",
                label: "🗑",
            },
            Cell::Inert {
                title: "Use selected text",
                sub: "Send the text you have selected, so the AI can work on it; never from terminals or password managers",
                on: true,
            },
            Cell::Inert {
                title: "Read notifications",
                sub: "Let the AI read your recent notifications when you ask about them",
                on: true,
            },
        ];
        self.card(c, view, y, &cells);

        group_title(c, view, y, "About the AIs");
        group_sub(
            c,
            view,
            y,
            "Claude Code and Codex answer within your plan's limits; Antigravity is at your own risk. Commands and file edits always go through Isle, which asks first.",
            2,
        );
    }

    /// The black notch the script showed above the shape tiles.
    fn preview(&mut self, c: &mut Canvas, view: Rect, y: &mut f32) {
        let card = Rect::new(view.x, *y + 4.0, view.w - 8.0, 100.0);
        c.rect(card, 12.0, CARD);
        // The notch hangs from the top edge of the card: a black pill with a
        // squared top where it meets that edge and rounded bottom corners.
        let notch = Rect::new(card.x + (card.w - 340.0) / 2.0, card.y, 340.0, 78.0);
        c.rect(notch, 22.0, BLACK);
        // Squaring the top in the notch's own black, not the card's grey, so no
        // empty band is left above the content.
        c.rect(Rect::new(notch.x, notch.y, notch.w, 22.0), 0.0, BLACK);
        // The rim of light along the bottom, in two soft bands.
        c.rect(
            Rect::new(notch.x + 24.0, notch.bottom() - 3.0, notch.w - 48.0, 1.0),
            0.5,
            0x2F4A6E,
        );
        c.rect(
            Rect::new(notch.x + 60.0, notch.bottom() - 2.0, notch.w - 120.0, 1.0),
            0.5,
            0x3A3260,
        );
        // The bot and its caption ride in the middle of the notch.
        let cy = notch.y + notch.h / 2.0;
        let bot_x = notch.x + 22.0 + 19.0;
        bot::draw(
            c,
            bot_x,
            cy,
            bot::Spot::Preview,
            &self.settings.assistant_shape,
            &self.settings.assistant_color,
            1.0,
            0,
        );
        c.text(
            "Listening...",
            Rect::new(bot_x + 33.0, cy - 9.0, 200.0, 18.0),
            13.0,
            SEMI,
            FAINT,
            Align::Left,
        );
        *y = card.bottom() + 14.0;
    }

    /// Shape tiles, colour swatches and the glow row, in one card.
    fn look_card(&mut self, c: &mut Canvas, view: Rect, y: &mut f32) {
        let top = *y + 4.0;
        let card = Rect::new(view.x, top, view.w - 8.0, 320.0);
        c.rect(card, 12.0, CARD);
        let inner = Rect::new(card.x + 10.0, card.y, card.w - 20.0, card.h);

        let shape = self.settings.assistant_shape.to_ascii_lowercase();
        let colour = self.settings.assistant_color.to_ascii_lowercase();

        c.text(
            "Shape",
            Rect::new(card.x + 14.0, card.y + 12.0, 100.0, 16.0),
            13.0,
            BOLD,
            WHITE,
            Align::Left,
        );
        let tile_w = (inner.w - 32.0) / 4.0;
        for index in 0..8 {
            let (column, row) = (index % 4, index / 4);
            let tile = Rect::new(
                inner.x + 4.0 + column as f32 * (tile_w + 8.0),
                card.y + 34.0 + row as f32 * 46.0,
                tile_w,
                42.0,
            );
            let name = bot::SHAPES[index];
            if shape.trim_start_matches("shape") == name {
                c.rect(tile, 8.0, PILL);
            }
            bot::draw(
                c,
                tile.x + tile.w / 2.0,
                tile.y + tile.h / 2.0,
                bot::Spot::Tile,
                name,
                &self.settings.assistant_color,
                1.0,
                0,
            );
            self.spot(tile, Act::Shape(index));
        }

        c.rect(
            Rect::new(card.x + 14.0, card.y + 132.0, card.w - 28.0, 1.0),
            0.0,
            EDGE,
        );
        c.text(
            "Colour",
            Rect::new(card.x + 14.0, card.y + 140.0, 100.0, 16.0),
            13.0,
            BOLD,
            WHITE,
            Align::Left,
        );
        let swatch_w = (inner.w - 48.0) / 6.0;
        for index in 0..12 {
            let (column, row) = (index % 6, index / 6);
            let tile = Rect::new(
                inner.x + 4.0 + column as f32 * (swatch_w + 8.0),
                card.y + 162.0 + row as f32 * 44.0,
                swatch_w,
                40.0,
            );
            let name = bot::COLOURS[index];
            let key = if name == "eyedropper" { "sky" } else { name };
            if colour.trim_start_matches("col") == key {
                c.rect(tile, 8.0, PILL);
            }
            let swatch = bot::palette(name);
            c.circle(
                tile.x + tile.w / 2.0,
                tile.y + tile.h / 2.0,
                11.0,
                swatch.base,
            );
            if name == "eyedropper" {
                // The one swatch that wears a second colour, as its name says.
                c.circle(
                    tile.x + tile.w / 2.0 + 4.0,
                    tile.y + tile.h / 2.0 - 4.0,
                    5.0,
                    0xAB8EFF,
                );
            }
            self.spot(tile, Act::Colour(index));
        }

        c.rect(
            Rect::new(card.x + 14.0, card.y + 256.0, card.w - 28.0, 1.0),
            0.0,
            EDGE,
        );
        let glow = Rect::new(card.x, card.y + 264.0, card.w, ROW_H);
        self.cell(
            c,
            glow,
            &Cell::Picker {
                title: "Glow",
                sub: "The light along the island's edge",
                key: "glow",
                label: picker_label(self.settings, "glow"),
            },
        );
        *y = card.bottom() + 16.0;
    }

    fn usage_page(&mut self, c: &mut Canvas, view: Rect, y: &mut f32) {
        group_title(c, view, y, "AI Usage");
        group_sub(
            c,
            view,
            y,
            "Shows how much of each plan's limit you have used, with a warning as one runs low. Read from the apps you are signed in to; nothing to set up.",
            2,
        );
        let cells = vec![
            Cell::Inert {
                title: "Show AI usage",
                sub: "The Usage page and its warnings",
                on: true,
            },
            Cell::Inert {
                title: "Claude",
                sub: "Team plan, signed in with Claude Code",
                on: true,
            },
            Cell::Inert {
                title: "ChatGPT",
                sub: "As Codex last saw it",
                on: true,
            },
            Cell::Inert {
                title: "Antigravity",
                sub: "Pro plan, signed in with Antigravity",
                on: true,
            },
            Cell::Picker {
                title: "Warn at",
                sub: "Share of a limit, in %",
                key: "warnat",
                label: self.picker_text("warnat"),
            },
            Cell::Picker {
                title: "Warn again at",
                sub: "Share of a limit, in %",
                key: "warnagain",
                label: self.picker_text("warnagain"),
            },
        ];
        self.card(c, view, y, &cells);

        group_title(c, view, y, "AI Agents");
        group_sub(
            c,
            view,
            y,
            "Answer your coding agents from the island when they ask, and tell you when they finish. On by default.",
            2,
        );
        let cells = vec![
            Cell::Inert {
                title: "Claude Code",
                sub: "Allow, deny or always allow, and when it is done",
                on: true,
            },
            Cell::Inert {
                title: "Codex",
                sub: "Allow, deny or always allow, and when it is done",
                on: true,
            },
            Cell::Inert {
                title: "Antigravity",
                sub: "Allow, deny or always allow, and when it is done",
                on: true,
            },
        ];
        self.card(c, view, y, &cells);
    }

    fn about_page(&mut self, c: &mut Canvas, view: Rect, y: &mut f32) {
        group_title(c, view, y, "Settings");
        let cells = vec![Cell::Button {
            title: "Reset all settings",
            sub: "Back to how Isle came; your data stays",
            label: "Reset",
            act: Act::ResetAll,
            fill: RED,
            ink: BLACK,
        }];
        self.card(c, view, y, &cells);
    }
}

impl Prefs {
    /// One frame: shell, page, scrollbar, bar and any open menu.
    fn paint(&mut self) {
        if self.renderer.begin().is_err() {
            return;
        }
        let mut spots = std::mem::take(&mut self.spots);
        spots.clear();
        let mut close = self.close;
        let mut body = self.body;
        let height;
        {
            let mut canvas = Canvas::new(&self.renderer);
            let mut ctx = Ctx {
                settings: &self.settings,
                page: self.page,
                scroll: self.scroll[self.page],
                menu: self.menu.as_ref(),
                menu_anim: self.menu_anim,
                switch_anim: &self.switch_anim,
                extra: &self.extra,
                mouse: self.mouse,
                spots: &mut spots,
                close: &mut close,
                body: &mut body,
            };
            height = ctx.draw(&mut canvas);
        }
        self.content_h[self.page] = height;
        let _ = self.renderer.end();
        let _ = self.renderer.present(self.hwnd);
        self.spots = spots;
        self.close = close;
        self.body = body;
        // The spots were rebuilt, so the remembered highlight is re-read
        // against them rather than left pointing at a stale index.
        self.hover = self.hover_at(self.mouse.0, self.mouse.1);
        self.dirty = false;
    }

    /// Paints at once when something changed. The window has no timer, so
    /// nothing else would come along to do it.
    fn flush(&mut self) {
        if self.dirty {
            self.paint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_rate_menu_starts_from_the_saved_rate() {
        let mut settings = Settings::default();
        let (options, index) = options_for(&settings, "fps");
        assert_eq!(options.len(), 5);
        assert_eq!(index, 0, "no saved rate means V-Sync");
        settings.target_fps = 144.0;
        assert_eq!(options_for(&settings, "fps").1, 3);
    }

    #[test]
    fn the_model_menu_follows_the_provider() {
        let settings = Settings::default();
        let (options, index) = options_for(&settings, "model");
        assert_eq!(options.len(), 3);
        assert_eq!(options[index].0, "Gemini Flash");
        assert_eq!(models_for("Claude Code")[0], "Sonnet");
        assert_eq!(models_for("Codex")[2], "Codex default");
    }

    #[test]
    fn the_pickers_read_the_settings_back() {
        let mut settings = Settings::default();
        assert_eq!(picker_label(&settings, "topbar"), "Glass");
        assert_eq!(picker_label(&settings, "openmode"), "On hover");
        settings.open_mode = "click".into();
        assert_eq!(picker_label(&settings, "openmode"), "On click");
        settings.top_bar_style = "solid".into();
        assert_eq!(picker_label(&settings, "topbar"), "Solid");
    }

    #[test]
    fn the_swatch_names_come_back_in_the_scripts_case() {
        assert_eq!(capital("sky"), "Sky");
        assert_eq!(capital("eyedropper"), "Eyedropper");
        assert_eq!(capital(""), "");
        assert_eq!(
            format!("Shape{}", capital(bot::SHAPES[7])),
            "ShapeGhost",
            "the seventh tile is the shape the defaults chose"
        );
    }

    #[test]
    fn every_alert_kind_has_its_own_preview_code() {
        let kinds = [
            AlertKind::Mini,
            AlertKind::Charging,
            AlertKind::Meeting,
            AlertKind::Message,
            AlertKind::Permission,
        ];
        for (index, kind) in kinds.iter().enumerate() {
            for other in &kinds[index + 1..] {
                assert_ne!(*kind as usize, *other as usize);
            }
        }
    }
}
