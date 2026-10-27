//! What is on the glass: the notch's silhouette, the compact island, the tab
//! header at the top of the open island, the small alert card, and whichever
//! page is up.
//!
//! The sizes here are the ones the script's XAML declared, translated to
//! rectangles: the open island has a 14 px margin, a 24 px tab row, and the
//! page gets whatever height is left.

pub mod media;
pub mod tools;

use crate::app::{AlertKind, App};
use crate::render::Renderer;
use crate::sys::Power;
use crate::ui::*;

/// Offsets inside the notch: the XAML's `ExpandedView Margin="14,6,14,10"`.
const PAD_X: f32 = 14.0;
const PAD_TOP: f32 = 6.0;
const PAD_BOTTOM: f32 = 10.0;
/// The tab row: a `HeaderTab` button in a row with a 2 px top and 8 px bottom
/// margin. The button carries no explicit height, so it is its content plus the
/// style's `Padding="6,4"` - and the tallest content in the row is the 12 px
/// label, whose line box is 16 px, giving 16 + 8 = 24. The 20 px gear and
/// sparkle sit centred in that same height.
const TAB_H: f32 = 24.0;
const HEADER_H: f32 = 2.0 + TAB_H + 8.0;

/// The notch's body: the island with its flared shoulders taken back off.
///
/// `App::body` covers the whole shape, shoulders and all, but the XAML hangs
/// `CompactView`, `AlertView` and `ExpandedView` inside the body only. Every
/// offset in this module is therefore measured from `notch_body(..).x`; using
/// `body.x` instead slides the drawing a shoulder's width - 22 px open, 16 px
/// shut - off to the left.
fn notch_body(body: Rect, shoulder: f32) -> Rect {
    Rect::new(body.x + shoulder, body.y, body.w - 2.0 * shoulder, body.h)
}

/// Paints one frame and hands back the clickable rectangles it left behind.
///
/// The canvas borrows the renderer, and the page functions need the whole of
/// `App`; taking the renderer separately keeps both borrows immutable.
pub fn paint(app: &App, renderer: &Renderer) -> Vec<Hit> {
    let mut canvas = Canvas::new(renderer);
    paint_into(app, &mut canvas);
    canvas.hits
}

fn paint_into(app: &App, c: &mut Canvas) {
    let body = app.body();
    let w = app.animator.width.value;
    let h = app.animator.height.value;
    let r = app.animator.radius.value;
    let s = app.animator.shoulder.value;
    let progress = app.progress();

    if let Ok(shape) = c.r.notch_geometry_at(body.x, w, h, r, s) {
        // The lyric page washes the whole card in the sleeve's own colour, the
        // reference's tinted backdrop; every other face stays the plain ink.
        match (app.page, app.art_color) {
            (Page::Lyrics, Some(tint)) if progress > 0.25 => {
                c.fill(&shape, INK);
                let fade = ((progress - 0.25) / 0.75).clamp(0.0, 1.0);
                c.fade = fade;
                c.fill_gradient(&shape, 0.0, h, &[(tint, 0.55), (tint, 0.12)]);
                c.fade = 1.0;
            }
            _ => c.fill(&shape, INK),
        }
    }

    if app.alert.is_some() {
        alert(app, c, body);
        return;
    }

    // The script's cross-fade: the compact row disappears quickly once the
    // island starts growing, and the page appears after a quarter of the way.
    let compact_alpha = (1.0 - progress * 2.5).clamp(0.0, 1.0);
    if compact_alpha > 0.0 {
        c.fade = compact_alpha;
        compact(app, c, body);
    }
    if progress > 0.25 {
        c.fade = ((progress - 0.25) / 0.75).clamp(0.0, 1.0);
        expanded(app, c, body);
    }
    c.fade = 1.0;
}

// ---- the closed island ----------------------------------------------------

fn compact(app: &App, c: &mut Canvas, body: Rect) {
    let s = app.animator.shoulder.value;
    let area = notch_body(body, s);
    let cy = area.h / 2.0;
    c.clip(area);

    // Left: while something is making sound, the logo of the app doing it;
    // otherwise, the assistant. The logo is drawn a touch smaller than the slot
    // and kept centred, so it balances the equalizer bars on the right.
    let slot = Rect::new(area.x + 10.0, cy - 12.0, 24.0, 24.0);
    match (app.media.sound, app.app_icon.as_ref()) {
        (true, Some((_, bitmap))) => c.image(bitmap, slot.x + 3.0, cy - 9.0, 18.0, 4.0),
        _ => mascot(c, slot.x + 12.0, cy, crate::bot::Spot::Compact, app),
    }

    // Centre: the clock.
    let clock = Rect::new(area.x + 34.0, 0.0, area.w - 34.0 - 46.0, area.h);
    c.text(
        &app.clock.compact(),
        clock,
        13.0,
        SEMI,
        WHITE,
        Align::Center,
    );

    // Right: the equalizer, at rest while nothing is playing.
    bars(c, area.right() - 30.0, cy, 14.0, app);

    c.unclip();
    c.hit(area, Action::Expand);
}

/// The assistant's body in whatever shape and colour the preferences chose,
/// with the eyes open as wide as the blink allows.
pub(crate) fn mascot(c: &mut Canvas, cx: f32, cy: f32, spot: crate::bot::Spot, app: &App) {
    let (height, glance) = app.blink.eyes();
    // Only the compact bot blinks and glances; the script never animated the
    // other hosts' faces.
    let (openness, glance) = match spot {
        crate::bot::Spot::Compact | crate::bot::Spot::Input => (height / 4.8, glance),
        _ => (1.0, 0),
    };
    crate::bot::draw(
        c,
        cx,
        cy,
        spot,
        &app.settings.assistant_shape,
        &app.settings.assistant_color,
        openness,
        glance,
    );
}

/// Four bars on their own sine curves, the script's equalizer.
pub(crate) fn bars(c: &mut Canvas, x: f32, cy: f32, height: f32, app: &App) {
    const REST: [f32; 4] = [0.35, 0.75, 0.50, 0.25];
    const LO: [f32; 4] = [0.25, 0.35, 0.20, 0.30];
    const HI: [f32; 4] = [0.88, 1.00, 0.78, 0.94];
    const DUR: [f32; 4] = [0.430, 0.340, 0.510, 0.380];
    let playing = app.media.sound;
    let phase = app.seconds();
    for index in 0..4 {
        let scale = if playing {
            let t = (phase / DUR[index]).fract();
            let triangle = 1.0 - (2.0 * t - 1.0).abs();
            let eased = 0.5 - 0.5 * (std::f32::consts::PI * triangle).cos();
            LO[index] + (HI[index] - LO[index]) * eased
        } else {
            REST[index]
        };
        let h = (height * scale).max(1.5);
        c.rect(
            Rect::new(x + index as f32 * 4.5, cy - h / 2.0, 2.5, h),
            1.2,
            WHITE,
        );
    }
}

// ---- alerts ---------------------------------------------------------------

fn alert(app: &App, c: &mut Canvas, body: Rect) {
    let Some(alert) = app.alert.as_ref() else {
        return;
    };
    let s = app.animator.shoulder.value;
    let area = notch_body(body, s);
    let cy = area.h / 2.0;
    c.clip(area);

    match alert.kind {
        AlertKind::Charging => {
            let ring = Rect::new(area.x + 10.0, cy - 15.0, 30.0, 30.0);
            c.ring(ring.x + 15.0, cy, 14.0, GREEN, 2.5);
            c.polygon(
                &[
                    (ring.x + 17.0, cy - 7.0),
                    (ring.x + 12.0, cy + 1.0),
                    (ring.x + 15.0, cy + 1.0),
                    (ring.x + 13.0, cy + 8.0),
                    (ring.x + 19.0, cy - 1.0),
                    (ring.x + 16.0, cy - 1.0),
                ],
                GREEN,
            );
            c.text(
                "Charging",
                Rect::new(area.x + 48.0, cy - 13.0, 140.0, 14.0),
                12.5,
                BOLD,
                WHITE,
                Align::Left,
            );
            c.text(
                "Charger connected",
                Rect::new(area.x + 48.0, cy - 1.0, 160.0, 14.0),
                11.0,
                NORMAL,
                DIM,
                Align::Left,
            );
            let percent = Power::read().percent;
            c.text(
                &format!("{percent}%"),
                Rect::new(area.right() - 80.0, 0.0, 66.0, area.h),
                18.0,
                BOLD,
                WHITE,
                Align::Right,
            );
        }
        AlertKind::Mini => {
            // The XAML's placeholder minute, left static there as well.
            c.circle(area.x + 12.0 + 7.0, cy, 7.0, ORANGE);
            c.text(
                "01:48",
                Rect::new(area.x + 36.0, 0.0, area.w - 72.0, area.h),
                13.0,
                SEMI,
                WHITE,
                Align::Center,
            );
            let x = area.right() - 12.0 - 13.5;
            for (i, height) in [8.0_f32, 14.0, 10.0].into_iter().enumerate() {
                c.rect(
                    Rect::new(x + 1.0 + i as f32 * 4.5, cy - height / 2.0, 2.5, height),
                    1.2,
                    WHITE,
                );
            }
        }
        AlertKind::Meeting => {
            c.circle(area.x + 10.0 + 16.0, cy, 16.0, BLUE);
            // The script put the camera emoji in the blue circle.
            c.text(
                "\u{1F4F9}",
                Rect::new(area.x + 13.0, cy - 8.0, 26.0, 16.0),
                13.0,
                NORMAL,
                WHITE,
                Align::Center,
            );
            c.text(
                "Weekly sync in 5 min",
                Rect::new(area.x + 58.0, cy - 13.0, 220.0, 14.0),
                12.5,
                BOLD,
                WHITE,
                Align::Left,
            );
            c.text(
                "01:53 – 02:23 · Google Meet",
                Rect::new(area.x + 58.0, cy - 1.0, 220.0, 14.0),
                11.0,
                NORMAL,
                DIM,
                Align::Left,
            );
            let pill = Rect::new(area.right() - 64.0, cy - 11.0, 52.0, 22.0);
            c.rect(pill, 11.0, BLUE);
            c.text("Join", pill, 11.5, BOLD, WHITE, Align::Center);
            c.hit(pill, Action::AlertJoin);
        }
        AlertKind::Message => {
            c.circle(area.x + 10.0 + 16.0, cy, 16.0, RAISED);
            // The script put the envelope emoji in the grey circle.
            c.text(
                "\u{2709}",
                Rect::new(area.x + 13.0, cy - 8.0, 26.0, 16.0),
                12.0,
                NORMAL,
                WHITE,
                Align::Center,
            );
            c.text(
                "Notification",
                Rect::new(area.x + 58.0, cy - 13.0, 220.0, 14.0),
                12.5,
                BOLD,
                WHITE,
                Align::Left,
            );
            c.text(
                "",
                Rect::new(area.x + 58.0, cy - 1.0, 220.0, 14.0),
                11.0,
                NORMAL,
                DIM,
                Align::Left,
            );
            let pill = Rect::new(area.right() - 70.0, cy - 12.0, 58.0, 24.0);
            c.rect(pill, 12.0, RAISED);
            c.text("Reply", pill, 11.0, NORMAL, WHITE, Align::Center);
            c.hit(pill, Action::AlertReply);
        }
        AlertKind::Permission => {
            let top = area.y + 7.0;
            // The assistant, with the amber badge that asks for an answer.
            c.circle(area.x + 13.0 + 17.0, top + 17.0, 17.0, WHITE);
            for side in [-3.5_f32, 3.5] {
                c.rect(
                    Rect::new(area.x + 29.0 + side - 1.5, top + 13.0, 3.0, 7.0),
                    1.5,
                    0x000000,
                );
            }
            c.circle(area.x + 10.0 + 7.0, top + 2.0 + 7.0, 7.0, ORANGE);
            c.text(
                "!",
                Rect::new(area.x + 10.0, top + 2.0, 14.0, 14.0),
                9.5,
                BOLD,
                0x000000,
                Align::Center,
            );
            c.text(
                "shop needs permission",
                Rect::new(area.x + 62.0, top, 300.0, 16.0),
                12.5,
                BOLD,
                WHITE,
                Align::Left,
            );
            c.text(
                "Claude Code",
                Rect::new(area.x + 62.0, top + 16.0, 200.0, 14.0),
                11.0,
                NORMAL,
                DIM,
                Align::Left,
            );
            let chip = Rect::new(area.x + 62.0, top + 33.0, 150.0, 20.0);
            c.rect(chip, 6.0, 0x161618);
            c.text_mono(
                "git push origin main",
                Rect::new(chip.x + 8.0, chip.y, 132.0, chip.h),
                11.0,
                NORMAL,
                WHITE,
            );
            let mut x = area.x + 62.0;
            for (label, always) in [("Deny", false), ("Allow", false), ("Always", true)] {
                let wide = label.len() as f32 * 6.2 + 22.0;
                let pill = Rect::new(x, top + 58.0, wide, 22.0);
                if always {
                    c.rect(pill, 11.0, WHITE);
                    c.text(label, pill, 11.0, BOLD, 0x000000, Align::Center);
                } else {
                    c.rect(pill, 11.0, RAISED);
                    c.text(label, pill, 11.0, NORMAL, WHITE, Align::Center);
                }
                let action = match label {
                    "Deny" => Action::AlertDeny,
                    "Allow" => Action::AlertAllow,
                    _ => Action::AlertAlways,
                };
                c.hit(pill, action);
                x += wide + 6.0;
            }
        }
    }

    c.unclip();
    c.hit(area, Action::AlertDismiss);
}

// ---- the open island ------------------------------------------------------

fn expanded(app: &App, c: &mut Canvas, body: Rect) {
    let h = app.animator.height.value;
    // `body` spans the whole island, the flared shoulders included; the XAML's
    // `ExpandedView` lives inside the notch body, so every offset here is
    // measured from past the shoulder, the way `compact` and `alert` do it.
    let area = notch_body(body, app.animator.shoulder.value);
    let left = area.x;
    let outer = Rect::new(
        left + PAD_X,
        PAD_TOP,
        area.w - PAD_X * 2.0,
        h - PAD_TOP - PAD_BOTTOM,
    );
    c.clip(outer);

    if app.tabs_visible {
        header(app, c, area);
    }

    // The page row: below the tabs, or from the very top when the tab row is
    // folded away for the conversation.
    let page = if app.tabs_visible {
        Rect::new(
            left + PAD_X,
            PAD_TOP + HEADER_H,
            area.w - PAD_X * 2.0,
            h - PAD_TOP - PAD_BOTTOM - HEADER_H,
        )
    } else {
        Rect::new(
            left + PAD_X,
            PAD_TOP,
            area.w - PAD_X * 2.0,
            h - PAD_TOP - PAD_BOTTOM,
        )
    };

    // The page content rides a short cross-fade on a switch (the header stays
    // put); `page_fade` is eased to 1 by the frame clock.
    let layer = c.fade;
    c.fade = layer * app.page_fade;
    match app.page {
        Page::Music => media::music(app, c, page),
        Page::Lyrics => media::lyrics_page(app, c, page),
        Page::Call => media::call(app, c, page),
        Page::Focus => tools::focus(app, c, page),
        Page::Reminders => tools::reminders(app, c, page),
        Page::Shelf => tools::shelf(app, c, page),
        Page::Calendar => tools::calendar(app, c, page),
        Page::Notifications => tools::notifications(app, c, page),
        Page::Voice => tools::conversation(app, c, page),
        Page::Usage => tools::usage(app, c, page),
    }
    c.fade = layer;

    c.unclip();
}

fn header(app: &App, c: &mut Canvas, area: Rect) {
    let top = PAD_TOP + 2.0;
    let mut x = area.x + PAD_X;
    let (mx, my) = app.mouse;

    for (index, tab) in Page::TABS.iter().enumerate() {
        let tab = *tab;
        // The script kept `TabCall` collapsed until a call was on, and nothing
        // ever turned it on, so the header starts at Audio.
        if tab == Page::Call {
            continue;
        }
        // `open` rides 0..1 as the pill grows or shrinks; the frame clock eases
        // it toward the selected tab so the highlight glides between them.
        let open = app.tab_open[index];
        let active = app.page.tab() == tab;
        let label = match tab {
            Page::Music => " Audio",
            Page::Notifications => " Notifications",
            Page::Focus => " Focus",
            Page::Reminders => " Reminders",
            Page::Shelf => " Shelf",
            Page::Calendar => " Calendar",
            _ => " Usage",
        };
        // `HeaderTab`: 6,4 of padding, a 13 px icon, and a margin of 1 either
        // side, which is the two pixels between the pills. The label's width is
        // measured for real (not a per-character guess, which ran wide on narrow
        // letters) and folded in only as far as the pill has opened.
        let text = label.trim_start();
        let lw = c.r.measure(text, 12.0, SEMI);
        // A 5 px breath between the icon and the label, folded in only as far
        // as the pill has opened.
        let width = 12.0 + 13.0 + (5.0 + lw) * open;
        let slot = Rect::new(x + 1.0, top, width, TAB_H);
        let over = slot.contains(mx, my);
        let pill = if over { 1.0 } else { open };
        if pill > 0.01 {
            let layer = c.fade;
            c.fade = layer * pill;
            c.rect(slot, 12.0, ACTIVE_TAB);
            c.fade = layer;
        }
        let colour = if active || over { WHITE } else { DIM };
        let icon = match tab {
            Page::Music => Icon::Music,
            Page::Notifications => Icon::Bell,
            Page::Focus => Icon::Timer,
            Page::Reminders => Icon::Check,
            Page::Shelf => Icon::Shelf,
            Page::Calendar => Icon::Calendar,
            _ => Icon::Gauge,
        };
        c.icon(icon, slot.x + 6.0 + 6.5, top + 12.0, 13.0, colour, 1.5);
        if open > 0.01 {
            let layer = c.fade;
            c.fade = layer * open;
            c.text(
                text,
                Rect::new(slot.x + 24.0, top, lw + 4.0, TAB_H),
                12.0,
                SEMI,
                colour,
                Align::Left,
            );
            c.fade = layer;
        }
        if tab == Page::Notifications && !app.store.notifications.is_empty() {
            // The 5 px dot, right- and top-aligned in the icon's grid with
            // the script's `Margin="0,-2,-2,0"`: centre at icon right + 2,
            // two pixels above the icon's own top edge.
            c.circle(slot.x + 18.5, top + 6.0, 2.5, RED);
        }
        c.hit(slot, Action::Tab(tab));
        x += width + 2.0;
    }

    // The right-hand group, laid out right to left the way the script's
    // horizontal stack was: gear, 10 px, time, date, 10 px, sparkle, 10 px,
    // the equalizer.
    let right = area.right() - PAD_X;
    let gear = Rect::new(right - 20.0, top + (TAB_H - 20.0) / 2.0, 20.0, 20.0);
    let date = app.clock.header_date();
    let time = app.clock.header_time();
    let time_w = c.r.measure(&time, 12.0, BOLD);
    // The trailing pair of spaces the date carries is not painted, so the gap
    // it stood for is measured in and added back by hand.
    let date_w = c.r.measure(date.trim_end(), 12.0, NORMAL) + 7.0;
    let time_x = gear.x - 10.0 - time_w;
    let date_x = time_x - date_w;
    let sparkle = Rect::new(date_x - 10.0 - 20.0, top + (TAB_H - 20.0) / 2.0, 20.0, 20.0);
    let bar_x = sparkle.x - 10.0 - 18.0;

    bars(c, bar_x, top + TAB_H / 2.0, 14.0, app);
    // The script fills the sparkle with #8E8E93 and never brightens it on
    // hover, unlike the tabs.
    c.icon(Icon::Sparkle, sparkle.x + 10.0, top + 12.0, 12.0, DIM, 1.2);
    c.text(
        &date,
        Rect::new(date_x, top, date_w + 2.0, TAB_H),
        12.0,
        NORMAL,
        DIM,
        Align::Left,
    );
    c.text(
        &time,
        Rect::new(time_x, top, time_w + 2.0, TAB_H),
        12.0,
        BOLD,
        WHITE,
        Align::Left,
    );
    // The gear too is a fixed #8E8E93 fill with no hover state.
    c.icon(Icon::Gear, gear.x + 10.0, top + 12.0, 13.0, DIM, 1.3);

    c.hit(gear, Action::Gear);
    c.hit(sparkle, Action::VoiceButton);
}

// ---- shared pieces --------------------------------------------------------

/// A pill button, the `PillBtn` style: radius 12, 12 px of horizontal padding.
pub(crate) fn pill(c: &mut Canvas, slot: Rect, label: &str, size: f32, fg: u32, bg: u32) {
    c.rect(slot, 12.0, bg);
    c.text(
        label,
        Rect::new(slot.x, slot.y, slot.w, slot.h),
        size,
        SEMI,
        fg,
        Align::Center,
    );
}

/// "1:04:11" past an hour, "4:11" under it, exactly as `Format-MediaTime` did.
pub(crate) fn media_time(ms: u64) -> String {
    let total = ms / 1000;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// A tall, faint bar on the right showing how much of a list is out of sight.
pub(crate) fn scroll_hint(c: &mut Canvas, track: Rect, offset: f32, content: f32) {
    if content <= track.h {
        return;
    }
    let height = (track.h * track.h / content).max(18.0);
    let max = (content - track.h).max(1.0);
    let y = track.y + (track.h - height) * (offset / max).clamp(0.0, 1.0);
    c.rect(
        Rect::new(track.right() - 2.0, y, 2.0, height),
        1.0,
        0x3A3A3C,
    );
}

/// The hint a page shows when it has nothing to list.
pub(crate) fn empty(c: &mut Canvas, page: Rect, text: &str, size: f32, hex: u32) {
    c.text(text, page, size, NORMAL, hex, Align::Center);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The island's rect as `App::body` builds it, without needing an `App`.
    fn island(width: f32, shoulder: f32, height: f32) -> Rect {
        let total = width + 2.0 * shoulder;
        Rect::new((crate::app::HOST_W - total) / 2.0, 0.0, total, height)
    }

    /// Numbers taken off the reference's own expanded capture at 120 dpi: the
    /// notch body fills device x 125..874, the `Audio` pill's left margin lands
    /// on 144, and the empty-state line centres on 499.5 - the host's own
    /// middle. Measuring from `body.x` instead of the notch body moves all
    /// three a shoulder's width to the left, which is 22 px open and 16 shut.
    #[test]
    fn the_open_island_is_laid_out_inside_the_notch_body() {
        let body = island(crate::app::EXPANDED_W, 22.0, crate::app::PAGE_H);
        let area = notch_body(body, 22.0);
        assert_eq!(area.x, 100.0, "the notch body's left edge, device 125");
        assert_eq!(area.right(), 700.0, "device 875");

        let page_x = area.x + PAD_X;
        let page_w = area.w - PAD_X * 2.0;
        assert_eq!(
            page_x + page_w / 2.0,
            crate::app::HOST_W / 2.0,
            "the page row stays centred on the host"
        );
        // The first tab carries the style's 1 px margin, and the capture shows
        // that margin's edge at device 144.
        assert_eq!(((page_x + 1.0) * 1.25).round() as i32, 144);
    }

    /// The closed island and the alert cards hang in the same rect, with the
    /// shoulder they were animated with rather than the open one.
    #[test]
    fn the_closed_island_shares_the_same_rect() {
        let body = island(crate::app::COMPACT_W, 16.0, 32.0);
        let area = notch_body(body, 16.0);
        assert_eq!(area.x, (crate::app::HOST_W - crate::app::COMPACT_W) / 2.0);
        assert_eq!(area.w, crate::app::COMPACT_W);
        // Taking the island's own left edge would start the clip 16 px early.
        assert_ne!(area.x, body.x);
    }

    /// `HeaderTab` has no height of its own, so the row is the 12 px label's
    /// line box plus the `Padding="6,4"`, and the page row sits below that with
    /// the tabs' own 2/8 margins. The reference's pill measures 30 device px
    /// tall at 120 dpi, which is 24 DIP.
    #[test]
    fn the_tab_row_is_the_height_the_capture_measures() {
        assert_eq!(TAB_H, 24.0);
        assert_eq!(HEADER_H, 34.0);
        assert_eq!((TAB_H * 1.25).round() as i32, 30, "device rows of pill");
        // The page row therefore starts 40 DIP down, not 37.
        assert_eq!(PAD_TOP + HEADER_H, 40.0);
    }
}
