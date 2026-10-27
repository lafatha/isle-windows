//! The island's drawing kit: rectangles, text, strokes and the little stroke
//! icons of the tab bar, plus the record of what was drawn where so a click can
//! be matched back to the control under the pointer.
//!
//! Everything is painted immediately, in logical pixels, into the host window's
//! coordinates: the Direct2D target carries the display scale.

use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::*;

use crate::icons;
use crate::render::Renderer;

pub const WHITE: u32 = 0xFFFFFF;
pub const DIM: u32 = 0x8E8E93;
pub const HINT: u32 = 0x636366;
pub const INK: u32 = 0x000000;
pub const GREEN: u32 = 0x30D158;
pub const ORANGE: u32 = 0xFF9F0A;
pub const RED: u32 = 0xFF453A;
pub const BLUE: u32 = 0x0A84FF;
pub const PILL: u32 = 0x1C1C1E;
pub const RAISED: u32 = 0x2C2C2E;
pub const BORDER: u32 = 0x333333;
pub const DIVIDER: u32 = 0x222222;
pub const SOFT: u32 = 0xE5E5EA;
pub const ACTIVE_TAB: u32 = 0x272729;

pub const NORMAL: i32 = 400;
pub const SEMI: i32 = 600;
pub const BOLD: i32 = 700;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Align {
    fn code(self) -> u8 {
        match self {
            Align::Left => 0,
            Align::Center => 1,
            Align::Right => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn inset(&self, by: f32) -> Self {
        Self {
            x: self.x + by,
            y: self.y + by,
            w: (self.w - by * 2.0).max(0.0),
            h: (self.h - by * 2.0).max(0.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Music,
    Lyrics,
    Call,
    Focus,
    Reminders,
    Shelf,
    Calendar,
    Notifications,
    Voice,
    Usage,
}

impl Page {
    /// The eight tabs, left to right, exactly as the header lays them out.
    pub const TABS: [Page; 8] = [
        Page::Call,
        Page::Music,
        Page::Notifications,
        Page::Focus,
        Page::Reminders,
        Page::Shelf,
        Page::Calendar,
        Page::Usage,
    ];

    pub fn tab(self) -> Page {
        match self {
            Page::Lyrics => Page::Music,
            other => other,
        }
    }
}

/// Every control that can be clicked. The action is recorded next to the
/// rectangle it was drawn in, so hit testing is just a reverse scan.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Expand,
    Tab(Page),
    ToggleMenu,
    VoiceButton,
    Gear,
    // music
    PlayPause,
    NextTrack,
    PrevTrack,
    /// The Output button: opens the Windows sound settings, as `BtnOutput` did.
    OutputDevice,
    SeekMusic,
    LyricsMode,
    BackToMusic,
    SeekLyrics,
    LyricLine(u64),
    // focus
    FocusStart,
    FocusReset,
    FocusWork(u8),
    WorkInc,
    WorkDec,
    BreakInc,
    BreakDec,
    // reminders
    ReminderToggle(usize),
    ReminderAdd,
    ReminderField,
    // shelf
    ShelfClip,
    ShelfFiles,
    ShelfFilter(u8),
    ShelfClear,
    ShelfSearch,
    ShelfSearchClose,
    ShelfCopy(usize),
    ShelfCopyFile(usize),
    // calendar
    CalPrev,
    CalNext,
    CalAdd,
    CalField,
    // notifications
    NotifDnd,
    NotifClear,
    // usage
    UsageDot(usize),
    // conversation
    ChatField,
    ChatSend,
    ChatNew,
    /// Run, or leave alone, the command card on this message.
    CardRun(usize),
    CardDeny(usize),
    // call
    CallMute,
    CallJoin,
    CallEnd,
    // alerts
    AlertDismiss,
    AlertDeny,
    AlertAllow,
    AlertAlways,
    AlertReply,
    AlertJoin,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub rect: Rect,
    pub action: Action,
}

pub struct Canvas<'a> {
    pub r: &'a Renderer,
    pub hits: Vec<Hit>,
    /// Multiplies every colour's alpha, so a whole layer can cross-fade. A
    /// layer that is this faint cannot be clicked either.
    pub fade: f32,
}

impl<'a> Canvas<'a> {
    pub fn new(r: &'a Renderer) -> Self {
        Self {
            r,
            hits: Vec::new(),
            fade: 1.0,
        }
    }

    /// Restricts everything drawn next to `rect`, as WPF's ClipToBounds did
    /// for the content inside the notch. The push/pop is counted on the
    /// renderer, which owns the render target, so nesting is tracked by depth
    /// and any imbalance is unwound at the start of the next frame.
    pub fn clip(&self, rect: Rect) {
        let clip = D2D_RECT_F {
            left: rect.x,
            top: rect.y,
            right: rect.right(),
            bottom: rect.bottom(),
        };
        self.r.push_clip(&clip);
    }

    pub fn unclip(&self) {
        self.r.pop_clip();
    }

    fn a(&self, alpha: f32) -> f32 {
        (alpha * self.fade).clamp(0.0, 1.0)
    }

    /// Records a clickable area. Later draws sit on top, as they do on screen.
    pub fn hit(&mut self, rect: Rect, action: Action) {
        if self.fade < 0.5 {
            return;
        }
        if rect.w > 0.0 && rect.h > 0.0 {
            self.hits.push(Hit { rect, action });
        }
    }

    // ---- shapes -----------------------------------------------------------

    fn fill_rect(&self, rect: Rect, radius: f32, hex: u32, alpha: f32) {
        let alpha = self.a(alpha);
        if alpha <= 0.001 {
            return;
        }
        let Ok(brush) = self.r.solid(hex, alpha) else {
            return;
        };
        if radius <= 0.0 {
            let d2d = D2D_RECT_F {
                left: rect.x,
                top: rect.y,
                right: rect.right(),
                bottom: rect.bottom(),
            };
            unsafe {
                let _ = self.r.target.FillRectangle(&d2d, &brush);
            }
            return;
        }
        let Ok(geometry) = self
            .r
            .rounded_geometry(rect.x, rect.y, rect.w, rect.h, radius)
        else {
            return;
        };
        if let Ok(geometry) = geometry.cast::<ID2D1Geometry>() {
            unsafe {
                let _ = self.r.target.FillGeometry(&geometry, &brush, None);
            }
        }
    }

    pub fn rect(&self, rect: Rect, radius: f32, hex: u32) {
        self.fill_rect(rect, radius, hex, 1.0);
    }

    /// A rounded rectangle at a given alpha, for tints and hover highlights.
    pub fn rect_a(&self, rect: Rect, radius: f32, hex: u32, alpha: f32) {
        self.fill_rect(rect, radius, hex, alpha);
    }

    /// A one-pixel rule, used for the dividers under reminders and rows.
    pub fn rule(&self, x: f32, y: f32, w: f32, hex: u32, alpha: f32) {
        self.fill_rect(Rect::new(x, y, w, 1.0), 0.0, hex, alpha);
    }

    pub fn stroke_rect(&self, rect: Rect, radius: f32, hex: u32, thickness: f32) {
        let Ok(brush) = self.r.solid(hex, self.a(1.0)) else {
            return;
        };

        let Ok(geometry) = self
            .r
            .rounded_geometry(rect.x, rect.y, rect.w, rect.h, radius)
        else {
            return;
        };
        if let Ok(geometry) = geometry.cast::<ID2D1Geometry>() {
            unsafe {
                let _ = self
                    .r
                    .target
                    .DrawGeometry(&geometry, &brush, thickness, None);
            }
        }
    }

    pub fn circle(&self, cx: f32, cy: f32, radius: f32, hex: u32) {
        self.circle_a(cx, cy, radius, hex, 1.0);
    }

    pub fn circle_a(&self, cx: f32, cy: f32, radius: f32, hex: u32, alpha: f32) {
        let alpha = self.a(alpha);
        if alpha <= 0.001 || radius <= 0.0 {
            return;
        }
        let Ok(brush) = self.r.solid(hex, alpha) else {
            return;
        };
        let ellipse = D2D1_ELLIPSE {
            point: windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F { x: cx, y: cy },
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            let _ = self.r.target.FillEllipse(&ellipse, &brush);
        }
    }

    pub fn ring(&self, cx: f32, cy: f32, radius: f32, hex: u32, thickness: f32) {
        self.ring_a(cx, cy, radius, hex, thickness, 1.0);
    }

    pub fn ring_a(&self, cx: f32, cy: f32, radius: f32, hex: u32, thickness: f32, alpha: f32) {
        let Ok(brush) = self.r.solid(hex, self.a(alpha)) else {
            return;
        };
        let ellipse = D2D1_ELLIPSE {
            point: windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F { x: cx, y: cy },
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            let _ = self.r.target.DrawEllipse(&ellipse, &brush, thickness, None);
        }
    }

    pub fn line(&self, x1: f32, y1: f32, x2: f32, y2: f32, hex: u32, thickness: f32) {
        self.line_a(x1, y1, x2, y2, hex, thickness, 1.0);
    }

    pub fn line_a(&self, x1: f32, y1: f32, x2: f32, y2: f32, hex: u32, thickness: f32, alpha: f32) {
        let Ok(brush) = self.r.solid(hex, self.a(alpha)) else {
            return;
        };
        unsafe {
            let _ = self.r.target.DrawLine(
                windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F { x: x1, y: y1 },
                windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F { x: x2, y: y2 },
                &brush,
                thickness,
                None,
            );
        }
    }

    pub fn polygon(&self, points: &[(f32, f32)], hex: u32) {
        self.polygon_a(points, hex, 1.0);
    }

    /// A filled outline at a given alpha, which is how the assistant's body
    /// builds its shading out of copies of the same shape.
    pub fn polygon_a(&self, points: &[(f32, f32)], hex: u32, alpha: f32) {
        let alpha = self.a(alpha);
        if alpha <= 0.001 || points.len() < 3 {
            return;
        }
        let Ok(geometry) = self.r.polygon(points) else {
            return;
        };
        let Ok(brush) = self.r.solid(hex, alpha) else {
            return;
        };
        if let Ok(geometry) = geometry.cast::<ID2D1Geometry>() {
            unsafe {
                let _ = self.r.target.FillGeometry(&geometry, &brush, None);
            }
        }
    }

    // ---- text -------------------------------------------------------------

    /// The compact island's bot halo: the shape filled at a low alpha, in
    /// layers that grow outwards, standing in for the script's blurred
    /// `DropShadowEffect`.
    pub fn fill_path(&self, cx: f32, cy: f32, size: f32, data: &str, hex: u32, alpha: f32) {
        let alpha = self.a(alpha);
        if alpha <= 0.001 {
            return;
        }
        self.r.fill_path(cx, cy, size, data, hex, alpha);
    }

    /// The assistant's body: `data` filled with the script's radial brush
    /// inside a `size` box centred on `cx`/`cy`.
    pub fn radial_path(
        &self,
        cx: f32,
        cy: f32,
        size: f32,
        data: &str,
        light: u32,
        base: u32,
        shade: u32,
    ) {
        self.r
            .radial_path(cx, cy, size, data, light, base, shade, self.a(1.0));
    }

    // ---- text -------------------------------------------------------------

    /// One line of text, vertically centred in its box. Long text is trimmed
    /// with an ellipsis, as WPF's TextBlock did inside the fixed columns.
    #[allow(clippy::too_many_arguments)]
    pub fn text(&self, value: &str, rect: Rect, size: f32, weight: i32, hex: u32, align: Align) {
        self.text_a(value, rect, size, weight, hex, align, 1.0);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn text_a(
        &self,
        value: &str,
        rect: Rect,
        size: f32,
        weight: i32,
        hex: u32,
        align: Align,
        alpha: f32,
    ) {
        let alpha = self.a(alpha);
        if alpha <= 0.001 || value.is_empty() {
            return;
        }
        let Ok(layout) = self
            .r
            .layout(value, size, weight, align.code(), false, 0.0, 0, rect.w, rect.h)
        else {
            return;
        };
        let Ok(brush) = self.r.solid(hex, alpha) else {
            return;
        };
        let origin = windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F {
            x: rect.x,
            y: rect.y,
        };
        unsafe {
            let _ = self
                .r
                .target
                .DrawTextLayout(origin, &layout, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
    }

    /// Wrapped text, with the uniform line height WPF's LineHeight gave.
    #[allow(clippy::too_many_arguments)]
    pub fn text_wrap(&self, value: &str, rect: Rect, size: f32, weight: i32, hex: u32, line: f32) {
        if value.is_empty() {
            return;
        }
        let Ok(layout) = self
            .r
            .layout(value, size, weight, 0, true, line, 0, rect.w, rect.h)
        else {
            return;
        };
        let Ok(brush) = self.r.solid(hex, self.a(1.0)) else {
            return;
        };
        let origin = windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F {
            x: rect.x,
            y: rect.y,
        };
        unsafe {
            let _ = self
                .r
                .target
                .DrawTextLayout(origin, &layout, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
    }

    /// One line in the fixed-width face the command card prints in.
    pub fn text_mono(&self, value: &str, rect: Rect, size: f32, weight: i32, hex: u32) {
        if value.is_empty() {
            return;
        }
        let Ok(layout) = self
            .r
            .layout(value, size, weight, 0, false, 0.0, 1, rect.w, rect.h)
        else {
            return;
        };
        let Ok(brush) = self.r.solid(hex, self.a(1.0)) else {
            return;
        };
        let origin = windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F {
            x: rect.x,
            y: rect.y,
        };
        unsafe {
            let _ = self
                .r
                .target
                .DrawTextLayout(origin, &layout, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
    }

    /// Album art, clipped to the rounded box the script's Border used.
    pub fn image(&self, bitmap: &ID2D1Bitmap, x: f32, y: f32, size: f32, radius: f32) {
        self.r.image(bitmap, x, y, size, radius);
    }

    /// A cover-filled image in a rounded `w`x`h` box, for clipboard previews.
    pub fn image_box(
        &self,
        bitmap: &ID2D1Bitmap,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
    ) {
        self.r.image_box(bitmap, x, y, w, h, radius);
    }

    /// Fills any path the renderer built, such as the notch silhouette.
    pub fn fill(&self, geometry: &ID2D1PathGeometry, hex: u32) {
        let Ok(brush) = self.r.solid(hex, self.a(1.0)) else {
            return;
        };
        if let Ok(geometry) = geometry.cast::<ID2D1Geometry>() {
            unsafe {
                let _ = self.r.target.FillGeometry(&geometry, &brush, None);
            }
        }
    }

    /// Fills a geometry with a vertical gradient, the lyric page's sleeve wash.
    /// `stops` are `0xRRGGBB` colours with an alpha each, run from `top` down to
    /// `bottom`; the whole thing is dimmed by the layer's cross-fade.
    pub fn fill_gradient(
        &self,
        geometry: &ID2D1PathGeometry,
        top: f32,
        bottom: f32,
        stops: &[(u32, f32)],
    ) {
        let dimmed: Vec<(u32, f32)> = stops.iter().map(|(hex, a)| (*hex, self.a(*a))).collect();
        let Ok(brush) = self.r.linear_gradient(top, bottom, &dimmed) else {
            return;
        };
        if let Ok(geometry) = geometry.cast::<ID2D1Geometry>() {
            unsafe {
                let _ = self.r.target.FillGeometry(&geometry, &brush, None);
            }
        }
    }

    // ---- icons ------------------------------------------------------------

    /// Icons at `size`, centred on `cx`/`cy`. The geometric ones are the exact
    /// `Path` data the script's XAML used, drawn through `path_icon` so the
    /// stretch semantics match WPF; the rest are the glyphs the script put in
    /// `TextBlock`s.
    #[allow(clippy::too_many_arguments)]
    pub fn icon(&self, kind: Icon, cx: f32, cy: f32, size: f32, hex: u32, thickness: f32) {
        let t = thickness;
        match kind {
            // The tab row: stroked at 1.5, apart from the filled note.
            Icon::Music => {
                self.r
                    .path_icon(cx, cy, size, icons::TAB_MUSIC, hex, true, 0.0, false, 0.0)
            }
            Icon::Bell => {
                self.r
                    .path_icon(cx, cy, size, icons::TAB_BELL, hex, false, t, false, 0.0)
            }
            Icon::Timer => {
                self.r
                    .path_icon(cx, cy, size, icons::TAB_TIMER, hex, false, t, false, 0.0)
            }
            Icon::Shelf => {
                self.r
                    .path_icon(cx, cy, size, icons::TAB_SHELF, hex, false, t, false, 0.0)
            }
            Icon::Calendar => {
                self.r
                    .path_icon(cx, cy, size, icons::TAB_CALENDAR, hex, false, t, false, 0.0)
            }
            Icon::Gauge => {
                self.r
                    .path_icon(cx, cy, size, icons::TAB_USAGE, hex, false, t, false, 0.0)
            }
            Icon::Sparkle => {
                self.r
                    .path_icon(cx, cy, size, icons::SPARKLE, hex, true, 0.0, false, 0.0)
            }
            Icon::Gear => self
                .r
                .path_icon(cx, cy, size, icons::GEAR, hex, true, 0.0, false, 0.0),
            Icon::ChevronDown => {
                self.r
                    .path_icon(cx, cy, size, icons::CHEVRON, hex, false, 1.6, true, 0.0)
            }
            Icon::ChevronUp => {
                self.r
                    .path_icon(cx, cy, size, icons::CHEVRON, hex, false, 1.6, true, 180.0)
            }
            // Media controls, all filled.
            Icon::Play => {
                self.r
                    .path_icon(cx, cy, size, icons::PLAY_TRI, hex, true, 0.0, false, 0.0)
            }
            Icon::Pause => self
                .r
                .path_icon(cx, cy, size, icons::PAUSE, hex, true, 0.0, false, 0.0),
            Icon::PauseSlim => {
                self.r
                    .path_icon(cx, cy, size, icons::PAUSE_SLIM, hex, true, 0.0, false, 0.0)
            }
            Icon::Prev => self
                .r
                .path_icon(cx, cy, size, icons::PREV, hex, true, 0.0, false, 0.0),
            Icon::Next => self
                .r
                .path_icon(cx, cy, size, icons::NEXT, hex, true, 0.0, false, 0.0),
            Icon::Mic => self
                .r
                .path_icon(cx, cy, size, icons::MIC, hex, true, 0.0, false, 0.0),
            Icon::Open => self
                .r
                .path_icon(cx, cy, size, icons::OPEN, hex, true, 1.8, true, 0.0),
            // The script's `TextBlock` glyphs, centred where the icon goes.
            Icon::Trash => self.text(
                "\u{1F5D1}",
                Rect::new(cx - 8.0, cy - 8.0, 16.0, 16.0),
                11.0,
                NORMAL,
                hex,
                Align::Center,
            ),
            Icon::Search => self.text(
                "\u{1F50D}",
                Rect::new(cx - 8.0, cy - 8.0, 16.0, 16.0),
                size,
                NORMAL,
                hex,
                Align::Center,
            ),
            Icon::Clear => self.text(
                "\u{2715}",
                Rect::new(cx - 8.0, cy - 8.0, 16.0, 16.0),
                10.0,
                NORMAL,
                hex,
                Align::Center,
            ),
            Icon::Check => {
                self.r
                    .path_icon(cx, cy, size, icons::TAB_CHECK, hex, false, t, false, 0.0)
            }
            Icon::Link => self.text(
                "\u{1F517}",
                Rect::new(cx - 8.0, cy - 8.0, 16.0, 16.0),
                10.0,
                NORMAL,
                hex,
                Align::Center,
            ),
            Icon::Image => self.text(
                "\u{1F5BC}",
                Rect::new(cx - 8.0, cy - 8.0, 16.0, 16.0),
                10.0,
                NORMAL,
                hex,
                Align::Center,
            ),
            Icon::File => self.text(
                "\u{1F4C4}",
                Rect::new(cx - 8.0, cy - 8.0, 16.0, 16.0),
                10.0,
                NORMAL,
                hex,
                Align::Center,
            ),
            Icon::Reset => self
                .r
                .path_icon(cx, cy, size, icons::RESET, hex, true, 0.0, false, 0.0),
            Icon::Pin => self.text(
                "\u{1F4CC}",
                Rect::new(cx - 8.0, cy - 8.0, 16.0, 16.0),
                10.0,
                NORMAL,
                hex,
                Align::Center,
            ),
            Icon::Phone => {
                self.r
                    .path_icon(cx, cy, size, icons::TAB_PHONE, hex, true, 0.0, false, 0.0)
            }
            Icon::Chat => self
                .r
                .path_icon(cx, cy, size, icons::CHAT, hex, false, 1.6, true, 0.0),
            Icon::ChatFill => {
                self.r
                    .path_icon(cx, cy, size, icons::CHAT_BUBBLE, hex, true, 0.0, false, 0.0)
            }
            // The output device: the stroked double note with the small play
            // triangle dropped on it, exactly as the script's grid laid it.
            Icon::Output => {
                self.r
                    .path_icon(cx, cy, size, icons::DOUBLE_NOTE, hex, false, t, false, 0.0);
                self.r.path_icon(
                    cx,
                    cy + 2.0,
                    size * 8.0 / 14.0,
                    icons::PLAY_SMALL,
                    hex,
                    true,
                    0.0,
                    false,
                    0.0,
                );
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Music,
    Bell,
    Timer,
    Shelf,
    Calendar,
    Gauge,
    Sparkle,
    Gear,
    ChevronDown,
    ChevronUp,
    Play,
    Pause,
    PauseSlim,
    Prev,
    Next,
    Mic,
    Open,
    Trash,
    Search,
    Clear,
    Check,
    Link,
    Image,
    File,
    Reset,
    Pin,
    Phone,
    Chat,
    ChatFill,
    Output,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_know_their_edges() {
        let rect = Rect::new(10.0, 4.0, 100.0, 32.0);
        assert!(rect.contains(10.0, 4.0));
        assert!(rect.contains(110.0, 36.0));
        assert!(!rect.contains(110.1, 36.0));
        assert_eq!(rect.right(), 110.0);
        assert_eq!(rect.bottom(), 36.0);
        assert_eq!(rect.inset(5.0), Rect::new(15.0, 9.0, 90.0, 22.0));
    }

    #[test]
    fn the_tab_row_is_the_order_the_script_had() {
        assert_eq!(Page::TABS[0], Page::Call);
        assert_eq!(Page::TABS[1], Page::Music);
        assert_eq!(Page::TABS[2], Page::Notifications);
        assert_eq!(Page::TABS[7], Page::Usage);
        assert_eq!(Page::Lyrics.tab(), Page::Music);
    }

    #[test]
    fn the_topmost_hit_wins() {
        let mut hits = Vec::new();
        hits.push(Hit {
            rect: Rect::new(0.0, 0.0, 100.0, 100.0),
            action: Action::Expand,
        });
        hits.push(Hit {
            rect: Rect::new(10.0, 10.0, 20.0, 20.0),
            action: Action::Gear,
        });
        let pick = |x: f32, y: f32| {
            hits.iter()
                .rev()
                .find(|hit| hit.rect.contains(x, y))
                .map(|hit| hit.action.clone())
        };
        assert_eq!(pick(15.0, 15.0), Some(Action::Gear));
        assert_eq!(pick(80.0, 80.0), Some(Action::Expand));
        assert_eq!(pick(200.0, 80.0), None);
    }
}
