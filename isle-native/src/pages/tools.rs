//! The seven panes that are lists of one kind or another: the focus timer, the
//! reminders, the shelf, the calendar, the notification feed, the conversation
//! and the usage meters.
//!
//! Each one keeps the column widths, type sizes and colours the script's XAML
//! declared; the arithmetic that used to be done by a layout pass is done here
//! in plain pixels.

use crate::app::{App, Card, CardState, Field};
use crate::ui::*;

use super::{empty, mascot, pill, scroll_hint};

// ---- focus ---------------------------------------------------------------

pub fn focus(app: &App, c: &mut Canvas, page: Rect) {
    // Left: the ring, its white pip and the remaining time.
    let ring = Rect::new(page.x + 4.0, page.y + (page.h - 104.0) / 2.0, 104.0, 104.0);
    c.ring(ring.x + 52.0, ring.y + 52.0, 50.2, 0x272729, 3.5);
    c.circle(ring.x + 52.0, ring.y + 1.0, 3.0, WHITE);
    c.text(
        &app.focus.clock_text(),
        Rect::new(ring.x, ring.y + 26.0, 104.0, 30.0),
        24.0,
        BOLD,
        WHITE,
        Align::Center,
    );
    c.text(
        if app.focus.work { "Focus" } else { "Break" },
        Rect::new(ring.x, ring.y + 58.0, 104.0, 16.0),
        12.0,
        NORMAL,
        DIM,
        Align::Center,
    );

    // Middle: the two pills, each with a label, a value and two buttons.
    const PILL_W: f32 = 156.0;
    const PILL_H: f32 = 32.0;
    let middle_x = page.x + 130.0;
    let middle_w = (page.w - 280.0).max(PILL_W);
    let px = middle_x + (middle_w - PILL_W) / 2.0;
    let work = Rect::new(px, page.y + page.h / 2.0 - (PILL_H + 4.0), PILL_W, PILL_H);
    let brk = Rect::new(px, work.bottom() + 8.0, PILL_W, PILL_H);

    stepper(
        c,
        work,
        "Work",
        &app.settings.focus_work_min.to_string(),
        Action::WorkDec,
        Action::WorkInc,
        Action::FocusWork(0),
        app.focus.work,
    );
    stepper(
        c,
        brk,
        "Break",
        &app.settings.focus_break_min.to_string(),
        Action::BreakDec,
        Action::BreakInc,
        Action::FocusWork(1),
        !app.focus.work,
    );

    // Right: reset, then start or pause.
    let start_label = if app.focus.running { "Pause" } else { "Start" };
    let start = Rect::new(
        page.right() - 92.0,
        page.y + page.h / 2.0 - 15.0,
        92.0,
        30.0,
    );
    let reset = Rect::new(
        start.x - 10.0 - 36.0,
        page.y + page.h / 2.0 - 18.0,
        36.0,
        36.0,
    );
    c.rect(reset, 18.0, PILL);
    c.icon(
        Icon::Reset,
        reset.x + 18.0,
        reset.y + 18.0,
        14.0,
        WHITE,
        1.4,
    );
    c.hit(reset, Action::FocusReset);

    c.rect(start, 15.0, ACTIVE_TAB);
    // The script keeps the pause bars at all times; only the label moves
    // between Start, Pause and Resume.
    c.icon(
        Icon::PauseSlim,
        start.x + 22.0,
        start.y + 15.0,
        11.0,
        WHITE,
        1.0,
    );
    c.text(
        start_label,
        Rect::new(start.x + 36.0, start.y, 46.0, start.h),
        13.0,
        SEMI,
        WHITE,
        Align::Left,
    );
    c.hit(start, Action::FocusStart);
}

/// A `Work 25 − +` pill: the label switches phase, the two buttons step it.
#[allow(clippy::too_many_arguments)]
fn stepper(
    c: &mut Canvas,
    slot: Rect,
    label: &str,
    value: &str,
    down: Action,
    up: Action,
    pick: Action,
    active: bool,
) {
    c.rect(slot, 16.0, PILL);
    let colour = if active { WHITE } else { DIM };
    c.text(
        label,
        Rect::new(slot.x + 14.0, slot.y, 44.0, slot.h),
        13.0,
        NORMAL,
        colour,
        Align::Left,
    );
    c.text(
        value,
        Rect::new(slot.x + 58.0, slot.y, 36.0, slot.h),
        15.0,
        BOLD,
        WHITE,
        Align::Center,
    );
    c.line(
        slot.x + 104.0,
        slot.y + 14.0,
        slot.x + 104.0,
        slot.y + 18.0,
        WHITE,
        1.4,
    );
    // The `+` carries the script's `Margin="8,0,0,0"`, so it starts eight
    // past the `−` button's right edge (94 + 20 + 8), not flush against it.
    c.text(
        "+",
        Rect::new(slot.x + 122.0, slot.y, 20.0, slot.h),
        15.0,
        NORMAL,
        WHITE,
        Align::Center,
    );
    // The minus is drawn as a rule rather than a glyph so it sits on the
    // same optical line as the plus.
    c.hit(Rect::new(slot.x + 94.0, slot.y, 20.0, slot.h), down);
    c.hit(Rect::new(slot.x + 122.0, slot.y, 20.0, slot.h), up);
    c.hit(Rect::new(slot.x, slot.y, 94.0, slot.h), pick);
}

// ---- reminders -----------------------------------------------------------

pub fn reminders(app: &App, c: &mut Canvas, page: Rect) {
    let pending = app.store.reminders.iter().filter(|item| !item.done).count();
    let head = Rect::new(page.x + 4.0, page.y + 2.0, page.w - 8.0, 22.0);
    c.text(
        "Reminders",
        Rect::new(head.x, head.y, 200.0, head.h),
        16.0,
        BOLD,
        ORANGE,
        Align::Left,
    );
    // The pending count in orange, then a plain plus at the far right; the plus
    // focuses the add row (and files it when something is already typed).
    let plus = Rect::new(head.right() - 20.0, head.y, 20.0, head.h);
    c.text("+", plus, 18.0, NORMAL, DIM, Align::Center);
    c.hit(plus, Action::ReminderAdd);
    c.text(
        &format!("{pending}"),
        Rect::new(plus.x - 44.0, head.y, 40.0, head.h),
        15.0,
        BOLD,
        ORANGE,
        Align::Right,
    );

    // The list, then the input row pinned to the bottom.
    let input_h = 28.0;
    let list = Rect::new(
        page.x,
        head.bottom() + 8.0,
        page.w,
        (page.bottom() - 8.0 - input_h) - (head.bottom() + 8.0),
    );
    c.clip(list);

    if app.store.reminders.is_empty() {
        c.text(
            "No reminders",
            Rect::new(list.x, list.y + 24.0, list.w, 20.0),
            13.5,
            NORMAL,
            HINT,
            Align::Center,
        );
    } else {
        let mut y = list.y - app.scrolls.reminders;
        for (index, item) in app.store.reminders.iter().enumerate() {
            let lines = wrapped(&item.text, 12.5, list.w - 28.0);
            let row_h = 17.0 * lines as f32;
            y += 4.0;
            let row = Rect::new(list.x, y, list.w, row_h);
            let cx = row.x + 8.5;
            let cy = row.y + row_h / 2.0;
            if item.done {
                // A solid green circle, no check mark inside.
                c.circle(cx, cy, 8.5, GREEN);
            } else {
                c.ring(cx, cy, 8.5, 0x555555, 1.8);
            }
            let colour = if item.done { DIM } else { WHITE };
            c.text_wrap(
                &item.text,
                Rect::new(row.x + 28.0, row.y, row.w - 28.0, row_h),
                12.5,
                NORMAL,
                colour,
                17.0,
            );
            c.hit(row, Action::ReminderToggle(index));
            y += row_h + 6.0;
            c.rule(list.x + 28.0, y, list.w - 28.0, DIVIDER, 1.0);
            y += 7.0;
        }
        scroll_hint(c, list, app.scrolls.reminders, rem_content(app, list.w));
    }
    c.unclip();

    // The add row reads like a reminder itself: an empty ring and a placeholder,
    // the way the reference stacks it under the list rather than a boxed field.
    let focused = app.input.field == Some(Field::Reminder);
    let row = Rect::new(page.x, page.bottom() - input_h, page.w, input_h);
    let rcx = row.x + 8.5;
    let rcy = row.y + input_h / 2.0;
    c.ring(rcx, rcy, 8.5, if focused { ORANGE } else { 0x555555 }, 1.8);
    let typed = if focused { app.input.text.as_str() } else { "" };
    let label = if typed.is_empty() { "New Reminder" } else { typed };
    let colour = if typed.is_empty() { HINT } else { WHITE };
    c.text(
        label,
        Rect::new(row.x + 28.0, row.y, row.w - 32.0, row.h),
        13.0,
        NORMAL,
        colour,
        Align::Left,
    );
    if focused && !typed.is_empty() {
        let caret = row.x + 28.0 + typed.chars().count() as f32 * 6.4;
        c.rect(Rect::new(caret, rcy - 8.0, 1.2, 16.0), 0.6, WHITE);
    }
    c.hit(row, Action::ReminderField);
}

fn rem_content(app: &App, width: f32) -> f32 {
    app.store
        .reminders
        .iter()
        .map(|item| 4.0 + 17.0 * wrapped(&item.text, 12.5, width - 28.0) as f32 + 13.0)
        .sum()
}

// ---- shelf ---------------------------------------------------------------

pub fn shelf(app: &App, c: &mut Canvas, page: Rect) {
    // The Clipboard / Files segmented control.
    let seg = Rect::new(page.x, page.y + 1.0, 158.0, 24.0);
    c.rect(seg, 12.0, PILL);
    let clip_tab = Rect::new(seg.x + 2.0, seg.y + 2.0, 79.0, 20.0);
    let files_tab = Rect::new(clip_tab.right(), seg.y + 2.0, 75.0, 20.0);
    // The raised pill glides from one segment to the other as `shelf_tab` eases,
    // and the two labels cross-fade between white and grey.
    let t = app.shelf_tab;
    let glide = Rect::new(
        clip_tab.x + (files_tab.x - clip_tab.x) * t,
        seg.y + 2.0,
        clip_tab.w + (files_tab.w - clip_tab.w) * t,
        20.0,
    );
    c.rect(glide, 10.0, RAISED);
    c.text(
        "Clipboard",
        clip_tab,
        11.5,
        SEMI,
        mix(WHITE, DIM, t),
        Align::Center,
    );
    c.text("Files", files_tab, 11.5, SEMI, mix(DIM, WHITE, t), Align::Center);
    c.hit(clip_tab, Action::ShelfClip);
    c.hit(files_tab, Action::ShelfFiles);

    let trash = Rect::new(page.right() - 26.0, page.y, 26.0, 26.0);
    if app.shelf_files {
        let hint = format!(
            "{} of 6 \u{2022} click to copy, or drag into any app",
            app.store.shelf.len()
        );
        c.text(
            &hint,
            Rect::new(
                seg.right() + 12.0,
                page.y,
                trash.x - 8.0 - (seg.right() + 12.0),
                24.0,
            ),
            11.5,
            NORMAL,
            SOFT,
            Align::Right,
        );
    } else {
        // The four filters, only while the clipboard is the tab in view.
        let filters = Rect::new(seg.right() + 12.0, page.y, 106.0, 24.0);
        c.rect(filters, 12.0, PILL);
        for index in 0..4usize {
            let slot = Rect::new(
                filters.x + 2.0 + index as f32 * 25.5,
                filters.y + 2.0,
                24.0,
                20.0,
            );
            let active = app.shelf_filter == index;
            if active {
                c.rect(slot, 8.0, RAISED);
            }
            let colour = if active { WHITE } else { DIM };
            match index {
                0 => c.text("\u{2261}", slot, 12.0, BOLD, colour, Align::Center),
                1 => c.icon(Icon::Link, slot.x + 12.0, slot.y + 10.0, 10.0, colour, 1.2),
                2 => c.icon(Icon::Image, slot.x + 12.0, slot.y + 10.0, 10.0, colour, 1.2),
                _ => c.icon(Icon::File, slot.x + 12.0, slot.y + 10.0, 10.0, colour, 1.2),
            }
            c.hit(slot, Action::ShelfFilter(index as u8));
        }
        c.rect(trash, 13.0, PILL);
        c.icon(Icon::Trash, trash.x + 13.0, trash.y + 13.0, 12.0, DIM, 1.2);
        c.hit(trash, Action::ShelfClear);

        // The magnifier, or the box it opens, sits left of the trash exactly
        // where the script's ShelfClipActions stack put it.
        if app.input.field == Some(Field::Search) {
            let slot = Rect::new(trash.x - 6.0 - 143.0, page.y, 143.0, 26.0);
            c.rect(slot, 12.0, PILL);
            c.stroke_rect(slot, 12.0, BORDER, 1.0);
            c.icon(Icon::Search, slot.x + 14.0, slot.y + 13.0, 10.0, DIM, 1.1);
            let typed = app.input.text.as_str();
            c.text(
                typed,
                Rect::new(slot.x + 24.0, slot.y, slot.w - 50.0, slot.h),
                11.0,
                NORMAL,
                WHITE,
                Align::Left,
            );
            let caret = slot.x + 24.0 + typed.chars().count() as f32 * 5.4;
            c.rect(
                Rect::new(caret, slot.y + 6.0, 1.1, slot.h - 12.0),
                0.6,
                WHITE,
            );
            let shut = Rect::new(slot.right() - 20.0, slot.y + 3.0, 20.0, 20.0);
            c.icon(Icon::Clear, shut.x + 10.0, shut.y + 10.0, 8.0, DIM, 1.1);
            c.hit(slot, Action::ShelfSearch);
            c.hit(shut, Action::ShelfSearchClose);
        } else {
            let seek = Rect::new(trash.x - 32.0, page.y, 26.0, 26.0);
            c.rect(seek, 13.0, PILL);
            c.icon(Icon::Search, seek.x + 13.0, seek.y + 13.0, 11.0, DIM, 1.2);
            c.hit(seek, Action::ShelfSearch);
        }
    }

    // The cards themselves, scrolled sideways. The two rows cross-fade as the
    // tab eases, so switching glides instead of snapping.
    let view = Rect::new(page.x, page.y + 34.0, page.w, page.h - 34.0);
    c.clip(view);
    let layer = c.fade;
    let t = app.shelf_tab;
    if t < 0.995 {
        c.fade = layer * (1.0 - t);
        shelf_clips(app, c, view);
    }
    if t > 0.005 {
        c.fade = layer * t;
        shelf_files(app, c, view);
    }
    c.fade = layer;
    c.unclip();
}

fn shelf_clips(app: &App, c: &mut Canvas, view: Rect) {
    let query = app.shelf_query();
    let items: Vec<usize> = app
        .store
        .clips
        .iter()
        .enumerate()
        .filter(|(_, clip)| listed(&clip.text, app.shelf_filter, &query))
        .map(|(index, _)| index)
        .collect();
    if items.is_empty() {
        // The script changed the wording once a search was narrowing the list.
        let message = if query.is_empty() {
            "Clipboard is empty \u{00B7} copy any text to see it here"
        } else {
            "No matching clipboard items"
        };
        c.text(
            message,
            Rect::new(view.x, view.y, view.w, view.h.min(40.0)),
            12.0,
            NORMAL,
            0xD1D1D6,
            Align::Center,
        );
        return;
    }
    let span = items.len() as f32 * 144.0;

    let card_h = view.h;
    let mut x = view.x - app.scrolls.shelf;
    for index in items {
        let clip = &app.store.clips[index];
        let card = Rect::new(x, view.y, 136.0, card_h);
        c.rect(card, 12.0, PILL);
        match clip_kind(&clip.text) {
            // A picture copied as a path previews itself in a banner; the file
            // name rides below it. Until the loader delivers, an icon holds the
            // frame open.
            ClipKind::Image => {
                let banner = Rect::new(card.x + 8.0, card.y + 8.0, card.w - 16.0, card_h - 52.0);
                let path = clip.text.trim();
                match app.clip_thumbs.get(path) {
                    Some(bitmap) => c.image_box(bitmap, banner.x, banner.y, banner.w, banner.h, 10.0),
                    None => {
                        c.rect(banner, 10.0, RAISED);
                        c.icon(
                            Icon::Image,
                            banner.x + banner.w / 2.0,
                            banner.y + banner.h / 2.0,
                            26.0,
                            DIM,
                            1.5,
                        );
                    }
                }
                c.text(
                    &base_name(path),
                    Rect::new(card.x + 10.0, banner.bottom() + 6.0, card.w - 20.0, 14.0),
                    11.0,
                    NORMAL,
                    WHITE,
                    Align::Left,
                );
            }
            // A link gets a globe chip and its host on the first line, the whole
            // address wrapped small underneath.
            ClipKind::Link => {
                c.rect(Rect::new(card.x + 10.0, card.y + 12.0, 22.0, 22.0), 6.0, RAISED);
                c.icon(
                    Icon::Link,
                    card.x + 21.0,
                    card.y + 23.0,
                    12.0,
                    LINK_BLUE,
                    1.3,
                );
                c.text(
                    &link_host(&clip.text),
                    Rect::new(card.x + 38.0, card.y + 12.0, card.w - 48.0, 22.0),
                    12.0,
                    SEMI,
                    WHITE,
                    Align::Left,
                );
                c.text_wrap(
                    &clip.text,
                    Rect::new(card.x + 10.0, card.y + 44.0, card.w - 20.0, card_h - 66.0),
                    11.0,
                    NORMAL,
                    SOFT,
                    15.0,
                );
            }
            ClipKind::File => {
                c.rect(Rect::new(card.x + 10.0, card.y + 12.0, 22.0, 22.0), 6.0, RAISED);
                c.icon(
                    Icon::File,
                    card.x + 21.0,
                    card.y + 23.0,
                    12.0,
                    LINK_BLUE,
                    1.3,
                );
                c.text(
                    &base_name(&clip.text),
                    Rect::new(card.x + 38.0, card.y + 12.0, card.w - 48.0, 22.0),
                    12.0,
                    SEMI,
                    WHITE,
                    Align::Left,
                );
                c.text_wrap(
                    &clip.text,
                    Rect::new(card.x + 10.0, card.y + 44.0, card.w - 20.0, card_h - 66.0),
                    11.0,
                    NORMAL,
                    SOFT,
                    15.0,
                );
            }
            ClipKind::Text => {
                c.text_wrap(
                    &clip.text,
                    Rect::new(card.x + 10.0, card.y + 10.0, card.w - 20.0, card.h - 32.0),
                    12.0,
                    NORMAL,
                    WHITE,
                    15.0,
                );
            }
        }
        c.text(
            &clip.time,
            Rect::new(card.x + 10.0, card.bottom() - 18.0, 60.0, 14.0),
            10.0,
            NORMAL,
            HINT,
            Align::Left,
        );
        if clip.pinned {
            // The script's pin is the grey pushpin emoji at 10 px.
            c.icon(
                Icon::Pin,
                card.right() - 16.0,
                card.bottom() - 15.0,
                10.0,
                DIM,
                1.2,
            );
        }
        c.hit(card, Action::ShelfCopy(index));
        x += 144.0;
    }
    scroll_hint(c, view, app.scrolls.shelf, span);
}

fn shelf_files(app: &App, c: &mut Canvas, view: Rect) {
    if app.store.shelf.is_empty() {
        c.text(
            "Drop files here or copy in Explorer (holds up to 6)",
            Rect::new(view.x, view.y, view.w, view.h.min(40.0)),
            12.0,
            NORMAL,
            DIM,
            Align::Center,
        );
        return;
    }

    let mut x = view.x - app.scrolls.shelf + 6.0;
    let top = view.y + (view.h - 104.0).max(0.0) / 2.0;
    for (index, file) in app.store.shelf.iter().enumerate() {
        let card = Rect::new(x, top, 92.0, 104.0);
        let frame = Rect::new(card.x + 10.0, card.y, 72.0, 72.0);
        // An image file previews itself; anything else keeps the extension tile.
        match app.clip_thumbs.get(&file.path) {
            Some(bitmap) => c.image_box(bitmap, frame.x, frame.y, frame.w, frame.h, 14.0),
            None => {
                c.rect(frame, 14.0, RAISED);
                let ext = if file.ext.is_empty() {
                    "FILE".to_string()
                } else {
                    file.ext.clone()
                };
                let ext: String = ext.chars().take(4).collect();
                c.text(&ext, frame, 12.0, BOLD, 0x3584E4, Align::Center);
            }
        }
        c.text(
            &file.name,
            Rect::new(card.x, card.y + 78.0, 92.0, 22.0),
            10.5,
            NORMAL,
            WHITE,
            Align::Center,
        );
        c.hit(card, Action::ShelfCopyFile(index));
        x += 106.0;
    }
    scroll_hint(
        c,
        view,
        app.scrolls.shelf,
        app.store.shelf.len() as f32 * 106.0,
    );
}

/// The clipboard's four filters, as `Render-Clipboard` applied them.
fn keeps(text: &str, filter: usize) -> bool {
    let lower = text.to_lowercase();
    match filter {
        1 => lower.contains("http://") || lower.contains("https://") || lower.contains("www."),
        2 => [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg"]
            .iter()
            .any(|ext| lower.contains(ext)),
        3 => {
            let drive = lower.as_bytes().len() > 2
                && lower.as_bytes()[1] == b':'
                && lower.as_bytes()[2] == b'\\';
            drive || lower.contains('/') || lower.contains("file://")
        }
        _ => true,
    }
}

/// A clipboard entry survives both the filter pill and the search box. The
/// query arrives already trimmed and lower case, as `Render-Clipboard` did it.
fn listed(text: &str, filter: usize, query: &str) -> bool {
    keeps(text, filter) && (query.is_empty() || text.to_lowercase().contains(query))
}

/// The blue used for the link and file chips on a clipboard card.
const LINK_BLUE: u32 = 0x8AB4F8;

/// Blend two 0xRRGGBB colours, `t` running from the first to the second.
fn mix(from: u32, to: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |shift: u32| {
        let a = (from >> shift) & 0xFF;
        let b = (to >> shift) & 0xFF;
        (a as f32 + (b as f32 - a as f32) * t).round() as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// The four looks a clipboard card can take.
enum ClipKind {
    Link,
    Image,
    File,
    Text,
}

fn clip_kind(text: &str) -> ClipKind {
    let t = text.trim();
    let lower = t.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("www.") {
        ClipKind::Link
    } else if is_image_path(t) {
        ClipKind::Image
    } else if keeps(text, 3) {
        ClipKind::File
    } else {
        ClipKind::Text
    }
}

/// True when the text names an image file present on disk, so the shelf can
/// decode it into a thumbnail. "Copy as path" in Explorer yields exactly this.
pub fn is_image_path(text: &str) -> bool {
    const EXT: [&str; 7] = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg"];
    let t = text.trim();
    let lower = t.to_lowercase();
    EXT.iter().any(|e| lower.ends_with(e)) && std::path::Path::new(t).is_file()
}

/// The last segment of a path, for a card's title line.
fn base_name(text: &str) -> String {
    let t = text.trim().trim_end_matches(['\\', '/']);
    t.rsplit(['\\', '/']).next().unwrap_or(t).to_string()
}

/// The host of a URL, without the scheme or a leading `www.`.
fn link_host(text: &str) -> String {
    let t = text.trim();
    let rest = t
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    rest.split('/').next().unwrap_or(rest).to_string()
}

// ---- calendar ------------------------------------------------------------

pub fn calendar(app: &App, c: &mut Canvas, page: Rect) {
    let clock = &app.clock;
    let left = Rect::new(page.x, page.y, 200.0, page.h);
    let month = (app.calendar_month % 12) as u16 + 1;
    let year = clock.year;

    // Month and year, with the two arrows at the right.
    const MONTHS: [&str; 12] = [
        "JANUARY",
        "FEBRUARY",
        "MARCH",
        "APRIL",
        "MAY",
        "JUNE",
        "JULY",
        "AUGUST",
        "SEPTEMBER",
        "OCTOBER",
        "NOVEMBER",
        "DECEMBER",
    ];
    let name = MONTHS[(month - 1) as usize];
    let head = Rect::new(left.x, left.y, left.w, 18.0);
    c.text(
        name,
        Rect::new(head.x, head.y, 120.0, head.h),
        11.5,
        BOLD,
        RED,
        Align::Left,
    );
    c.text(
        &format!("{year}"),
        Rect::new(
            head.x + name.chars().count() as f32 * 7.4 + 6.0,
            head.y,
            60.0,
            head.h,
        ),
        11.5,
        NORMAL,
        HINT,
        Align::Left,
    );
    let next = Rect::new(head.right() - 16.0, head.y - 2.0, 16.0, 22.0);
    let prev = Rect::new(next.x - 12.0 - 16.0, head.y - 2.0, 16.0, 22.0);
    // The month arrows are the single angle quotes, grey.
    c.text("\u{2039}", prev, 15.0, NORMAL, DIM, Align::Center);
    c.text("\u{203A}", next, 15.0, NORMAL, DIM, Align::Center);
    c.hit(prev, Action::CalPrev);
    c.hit(next, Action::CalNext);

    // The weekday letters, then the grid sized to however many weeks the month
    // spans, so a five- and a six-row month both fill the column evenly.
    let cell_w = left.w / 7.0;
    let wk_y = head.bottom() + 8.0;
    for (index, letter) in ["S", "M", "T", "W", "T", "F", "S"].iter().enumerate() {
        c.text(
            letter,
            Rect::new(left.x + index as f32 * cell_w, wk_y, cell_w, 14.0),
            10.0,
            NORMAL,
            DIM,
            Align::Center,
        );
    }
    let first = weekday_of_first(year, month) as u32;
    let count = days_in_month(year, month);
    let weeks = ((first + count + 6) / 7) as f32;
    let grid_top = wk_y + 20.0;
    let row_h = (left.bottom() - grid_top).max(weeks * 18.0) / weeks;
    let dot = (row_h.min(cell_w) * 0.5).min(13.0);
    for day in 1..=count {
        let slot = first + day - 1;
        let column = (slot % 7) as f32;
        let row = (slot / 7) as f32;
        let cx = left.x + column * cell_w + cell_w / 2.0;
        let cy = grid_top + row * row_h + row_h / 2.0;
        let is_today = day == clock.day as u32 && month == clock.month && year == clock.year;
        let weekday = slot % 7;
        if is_today {
            c.circle(cx, cy, dot, RED);
            c.text(
                &day.to_string(),
                Rect::new(cx - 12.0, cy - 9.0, 24.0, 18.0),
                11.0,
                BOLD,
                WHITE,
                Align::Center,
            );
        } else {
            let colour = if weekday == 0 || weekday == 6 {
                HINT
            } else {
                WHITE
            };
            c.text(
                &day.to_string(),
                Rect::new(cx - 12.0, cy - 9.0, 24.0, 18.0),
                11.0,
                NORMAL,
                colour,
                Align::Center,
            );
        }
    }

    // Right: today's heading, the add field, the events, and a world clock.
    let right = Rect::new(
        left.right() + 18.0,
        page.y,
        page.right() - (left.right() + 18.0),
        page.h,
    );
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let head_h = 22.0;
    c.text(
        &format!(
            "Today, {} {}",
            clock.day,
            NAMES[(clock.month.saturating_sub(1) as usize) % 12]
        ),
        Rect::new(right.x, right.y, right.w - 26.0, head_h),
        16.0,
        BOLD,
        WHITE,
        Align::Left,
    );
    // A small globe at the heading's right, drawn from a ring and a cross.
    let gx = right.right() - 9.0;
    let gy = right.y + head_h / 2.0;
    c.ring(gx, gy, 7.0, DIM, 1.2);
    c.rect(Rect::new(gx - 7.0, gy - 0.6, 14.0, 1.2), 0.6, DIM);
    c.rect(Rect::new(gx - 0.6, gy - 7.0, 1.2, 14.0), 0.6, DIM);

    // The add-event field: a pill with a leading plus disc, as the reference.
    let field = Rect::new(right.x, right.y + head_h + 10.0, right.w, 30.0);
    let focused = app.input.field == Some(Field::Calendar);
    c.rect(field, 15.0, PILL);
    c.stroke_rect(field, 15.0, if focused { BLUE } else { BORDER }, 1.0);
    let plus = Rect::new(field.x + 6.0, field.y + 6.0, 18.0, 18.0);
    c.circle(plus.x + 9.0, plus.y + 9.0, 9.0, RAISED);
    c.text("+", plus, 15.0, BOLD, WHITE, Align::Center);
    let text_x = field.x + 32.0;
    let typed = if focused { app.input.text.as_str() } else { "" };
    let label = if typed.is_empty() {
        "Add event \u{00B7} e.g. Dinner 20:00"
    } else {
        typed
    };
    c.text(
        label,
        Rect::new(text_x, field.y, field.right() - text_x - 12.0, field.h),
        12.5,
        NORMAL,
        if typed.is_empty() { HINT } else { WHITE },
        Align::Left,
    );
    if focused && !typed.is_empty() {
        let caret = text_x + typed.chars().count() as f32 * 6.2;
        c.rect(Rect::new(caret, field.y + 7.0, 1.2, field.h - 14.0), 0.6, WHITE);
    }
    c.hit(field, Action::CalField);
    c.hit(plus, Action::CalAdd);

    // The events, scrolling. Each is a card tinted with its own colour, a bar
    // at the left and the time at the right.
    let list = Rect::new(
        right.x,
        field.bottom() + 12.0,
        right.w,
        (right.bottom() - 4.0 - (field.bottom() + 12.0)).max(0.0),
    );
    c.clip(list);
    if app.store.events.is_empty() {
        c.text(
            "No events yet \u{00B7} add one above",
            list,
            12.5,
            NORMAL,
            HINT,
            Align::Center,
        );
    } else {
        let mut y = list.y - app.scrolls.calendar;
        for event in &app.store.events {
            let card = Rect::new(list.x, y, list.w, 36.0);
            let colour = hex_of(&event.color, 0x3584E4);
            c.rect(card, 10.0, mix(colour, 0x141210, 0.86));
            c.rect(
                Rect::new(card.x + 9.0, card.y + 9.0, 3.0, card.h - 18.0),
                1.5,
                colour,
            );
            c.text(
                &event.title,
                Rect::new(card.x + 20.0, card.y, card.w - 78.0, card.h),
                13.0,
                SEMI,
                WHITE,
                Align::Left,
            );
            c.text(
                &event.time,
                Rect::new(card.right() - 56.0, card.y, 46.0, card.h),
                12.0,
                NORMAL,
                DIM,
                Align::Right,
            );
            y += 42.0;
        }
        scroll_hint(
            c,
            list,
            app.scrolls.calendar,
            app.store.events.len() as f32 * 42.0,
        );
    }
    c.unclip();
}

/// `#3584E4` to a number, with a fallback for anything malformed.
fn hex_of(value: &str, fallback: u32) -> u32 {
    let digits = value.trim_start_matches('#');
    u32::from_str_radix(digits, 16).unwrap_or(fallback)
}

/// The weekday of the first of a month, Sunday as 0.
fn weekday_of_first(year: u16, month: u16) -> u8 {
    const OFFSETS: [u32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y = year as u32;
    if month < 3 {
        y -= 1;
    }
    let index = (month - 1) as usize;
    ((y + y / 4 - y / 100 + y / 400 + OFFSETS[index] + 1) % 7) as u8
}

fn days_in_month(year: u16, month: u16) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap {
                29
            } else {
                28
            }
        }
    }
}

// ---- notifications -------------------------------------------------------

pub fn notifications(app: &App, c: &mut Canvas, page: Rect) {
    let head = Rect::new(page.x, page.y, page.w, 22.0);
    c.text(
        "Notifications",
        Rect::new(head.x, head.y, 160.0, head.h),
        13.0,
        SEMI,
        DIM,
        Align::Left,
    );

    let clear = Rect::new(head.right() - 56.0, head.y, 56.0, 22.0);
    let bell = Rect::new(clear.x - 10.0 - 14.0, head.y, 14.0, 22.0);
    c.icon(
        Icon::Bell,
        bell.x + 7.0,
        bell.y + 11.0,
        13.0,
        if app.settings.dnd { ORANGE } else { DIM },
        1.5,
    );
    c.hit(bell, Action::NotifDnd);
    pill(c, clear, "Clear", 11.0, WHITE, RAISED);
    c.hit(clear, Action::NotifClear);

    let list = Rect::new(
        page.x,
        head.bottom() + 10.0,
        page.w,
        page.bottom() - (head.bottom() + 10.0),
    );
    c.clip(list);

    if app.store.notifications.is_empty() {
        let cy = list.y + list.h / 2.0 - 26.0;
        c.icon(Icon::Bell, list.x + list.w / 2.0, cy, 26.0, DIM, 1.8);
        c.text(
            "No Notifications",
            Rect::new(list.x, cy + 18.0, list.w, 18.0),
            14.0,
            BOLD,
            WHITE,
            Align::Center,
        );
        c.text(
            "You're all caught up",
            Rect::new(list.x, cy + 38.0, list.w, 16.0),
            11.5,
            NORMAL,
            DIM,
            Align::Center,
        );
        c.unclip();
        return;
    }

    let mut y = list.y - app.scrolls.notifications;
    let height = app.store.notifications.len() as f32 * 36.0;
    for item in &app.store.notifications {
        let row = Rect::new(list.x, y, list.w, 26.0);
        c.rect(Rect::new(row.x, row.y, 26.0, 26.0), 6.0, RAISED);
        let initials: String = item.app.chars().take(2).collect();
        c.text(
            &initials,
            Rect::new(row.x, row.y, 26.0, 26.0),
            10.0,
            BOLD,
            WHITE,
            Align::Center,
        );
        c.text(
            &item.title,
            Rect::new(row.x + 34.0, row.y, (row.w - 34.0 - 48.0).min(260.0), 26.0),
            12.0,
            BOLD,
            WHITE,
            Align::Left,
        );
        if !item.detail.is_empty() {
            let title_w = (item.title.chars().count() as f32 * 6.6 + 4.0).min(260.0);
            c.text(
                &item.detail,
                Rect::new(
                    row.x + 34.0 + title_w,
                    row.y,
                    (row.right() - 48.0 - (row.x + 34.0 + title_w))
                        .max(0.0)
                        .min(220.0),
                    26.0,
                ),
                11.5,
                NORMAL,
                DIM,
                Align::Left,
            );
        }
        c.text(
            &item.time,
            Rect::new(row.right() - 44.0, row.y, 44.0, 26.0),
            11.0,
            NORMAL,
            HINT,
            Align::Right,
        );
        y += 36.0;
    }
    scroll_hint(c, list, app.scrolls.notifications, height);
    c.unclip();
}

// ---- conversation --------------------------------------------------------

pub fn conversation(app: &App, c: &mut Canvas, page: Rect) {
    let head = Rect::new(page.x + 4.0, page.y, page.w - 8.0, 24.0);
    c.text(
        "Conversation",
        Rect::new(head.x, head.y, 160.0, head.h),
        13.5,
        SEMI,
        DIM,
        Align::Left,
    );

    let chevron = Rect::new(head.right() - 24.0, head.y, 24.0, 24.0);
    let plus = Rect::new(chevron.x - 6.0 - 24.0, head.y, 24.0, 24.0);
    c.rect(plus, 7.0, RAISED);
    c.text("+", plus, 14.0, SEMI, WHITE, Align::Center);
    c.hit(plus, Action::ChatNew);
    c.rect(chevron, 7.0, RAISED);
    c.icon(
        if app.tabs_visible {
            Icon::ChevronUp
        } else {
            Icon::ChevronDown
        },
        chevron.x + 12.0,
        chevron.y + 12.0,
        9.0,
        WHITE,
        1.6,
    );
    c.hit(chevron, Action::ToggleMenu);

    // The stream between the header and the input row.
    let bar = Rect::new(page.x, page.bottom() - 30.0, page.w, 30.0);
    let stream = Rect::new(
        page.x + 4.0,
        head.bottom() + 6.0,
        page.w - 8.0,
        (bar.y - 6.0) - (head.bottom() + 6.0),
    );
    c.clip(stream);

    let content = chat_height(app, stream.w);
    let mut y = stream.y - app.scrolls.chat;
    for (index, message) in app.voice.messages.iter().enumerate() {
        if let Some(card) = message.card.as_ref() {
            y += card_row(index, card, c, stream, y, stream.w - 38.0);
        } else if message.from_user {
            let room = (stream.w - 50.0).max(80.0);
            let lines = wrapped(&message.text, 12.0, room - 28.0);
            let natural = message.text.chars().count() as f32 * 5.9 + 28.0;
            let width = natural.clamp(60.0, room);
            let h = 15.0 * lines as f32 + 14.0;
            let bubble = Rect::new(stream.right() - width, y + 2.0, width, h);
            c.rect(bubble, 14.0, RAISED);
            c.text_wrap(&message.text, bubble.inset(14.0), 12.0, NORMAL, WHITE, 15.0);
            y += h + 10.0;
        } else {
            let width = (stream.w - 40.0 - 30.0).max(80.0);
            let lines = wrapped(&message.text, 12.0, width);
            let h = 15.0 * lines as f32;
            mascot(c, stream.x + 11.0, y + 10.0, crate::bot::Spot::Avatar, app);
            c.text_wrap(
                &message.text,
                Rect::new(stream.x + 30.0, y + 2.0, width, h),
                12.0,
                NORMAL,
                WHITE,
                15.0,
            );
            y += h + 12.0;
        }
    }
    scroll_hint(c, stream, app.scrolls.chat, content);
    c.unclip();

    // The mascot and the input pill.
    mascot(c, bar.x + 15.0, bar.y + 15.0, crate::bot::Spot::Input, app);
    let field = Rect::new(bar.x + 42.0, bar.y + 1.0, bar.w - 42.0, 28.0);
    c.rect(field, 14.0, PILL);
    let focused = app.input.field == Some(Field::Chat);
    let text = if focused { app.input.text.as_str() } else { "" };
    if text.is_empty() {
        c.text(
            "Ask anything...",
            Rect::new(field.x + 12.0, field.y, field.w - 44.0, field.h),
            12.0,
            NORMAL,
            HINT,
            Align::Left,
        );
    } else {
        c.text(
            text,
            Rect::new(field.x + 12.0, field.y, field.w - 44.0, field.h),
            12.0,
            NORMAL,
            WHITE,
            Align::Left,
        );
    }
    c.icon(
        Icon::Mic,
        field.right() - 18.0,
        field.y + 14.0,
        12.0,
        DIM,
        1.2,
    );
    // The glyph is the script's BtnChatMic, which sent whatever was typed.
    c.hit(
        Rect::new(field.right() - 30.0, field.y, 30.0, field.h),
        Action::ChatSend,
    );
    c.hit(field, Action::ChatField);
}

/// How tall the whole conversation is, so the scrollbar knows its range.
fn chat_height(app: &App, width: f32) -> f32 {
    app.voice
        .messages
        .iter()
        .map(|message| {
            if message.card.is_some() {
                CARD_ROW
            } else if message.from_user {
                let lines = wrapped(&message.text, 12.0, (width - 50.0).max(80.0) - 28.0);
                15.0 * lines as f32 + 24.0
            } else {
                let lines = wrapped(&message.text, 12.0, (width - 70.0).max(80.0));
                15.0 * lines as f32 + 12.0
            }
        })
        .sum()
}

/// A command card with its margins: 30 px in from the left, 8 from the right,
/// 2 above and 8 below.
const CARD_ROW: f32 = 40.0;

/// One command card, built the way `Add-CommandCard` built it: the line on the
/// left in the terminal's own face, and the answer to it on the right.
fn card_row(index: usize, card: &Card, c: &mut Canvas, stream: Rect, y: f32, width: f32) -> f32 {
    const HEIGHT: f32 = 30.0;
    const PAD: f32 = 12.0;

    let rect = Rect::new(stream.x + 30.0, y + 2.0, width, HEIGHT);
    c.rect(rect, 10.0, 0x18181A);
    let mid = rect.y + HEIGHT / 2.0;
    let room = Rect::new(rect.x + PAD, rect.y, (rect.w - PAD * 2.0).max(20.0), HEIGHT);

    let line_width = match card.state {
        CardState::Pending => {
            let run = Rect::new(room.right() - 46.0, mid - 9.0, 46.0, 18.0);
            c.rect(run, 9.0, WHITE);
            c.text("Run ↩", run, 10.5, BOLD, INK, Align::Center);
            let deny = Rect::new(run.x - 4.0 - 42.0, mid - 9.0, 42.0, 18.0);
            c.rect(deny, 9.0, RAISED);
            c.text("Deny", deny, 10.5, NORMAL, WHITE, Align::Center);
            // A card that has scrolled out of the stream cannot be clicked.
            if rect.y >= stream.y && rect.bottom() <= stream.bottom() {
                c.hit(run, Action::CardRun(index));
                c.hit(deny, Action::CardDeny(index));
            }
            deny.x - 8.0 - room.x
        }
        CardState::Running(since) => {
            let elapsed = since.elapsed().as_secs();
            let label = format!("Running... {}:{:02}", elapsed / 60, elapsed % 60);
            c.text(
                &label,
                Rect::new(room.right() - 84.0, rect.y, 84.0, HEIGHT),
                11.0,
                NORMAL,
                DIM,
                Align::Right,
            );
            room.w - 92.0
        }
        CardState::Denied => {
            let label = Rect::new(room.right() - 50.0, rect.y, 50.0, HEIGHT);
            c.text("Denied", label, 11.0, NORMAL, DIM, Align::Right);
            room.w - 58.0
        }
        CardState::Done { ok, code } => {
            let text = if ok {
                "✓ Done".to_string()
            } else {
                format!("✕ Failed ({code})")
            };
            let wide = text.chars().count() as f32 * 6.2 + 6.0;
            let label = Rect::new(room.right() - wide, rect.y, wide, HEIGHT);
            if ok {
                c.text(&text, label, 11.5, BOLD, GREEN, Align::Right);
            } else {
                c.text(&text, label, 11.0, NORMAL, RED, Align::Right);
            }
            room.w - wide - 8.0
        }
    };

    // "$ ", the command itself in orange, and its arguments in white.
    let boxed = Rect::new(room.x, rect.y, line_width.max(24.0), HEIGHT);
    c.clip(boxed);
    let mut x = boxed.x;
    c.text_mono("$ ", Rect::new(x, rect.y, 20.0, HEIGHT), 11.5, NORMAL, DIM);
    x += 14.0;
    let mut tokens = card.line.split_whitespace();
    if let Some(first) = tokens.next() {
        let first = format!("{first} ");
        let wide = first.chars().count() as f32 * 6.4;
        c.text_mono(
            &first,
            Rect::new(x, rect.y, wide, HEIGHT),
            11.5,
            BOLD,
            ORANGE,
        );
        x += wide;
        let rest = tokens.collect::<Vec<_>>().join(" ");
        let left = (boxed.right() - x).max(1.0);
        c.text_mono(
            &rest,
            Rect::new(x, rect.y, left, HEIGHT),
            11.5,
            NORMAL,
            WHITE,
        );
    }
    c.unclip();

    CARD_ROW
}

// ---- usage ---------------------------------------------------------------
pub fn usage(app: &App, c: &mut Canvas, page: Rect) {
    let providers = app.usage.providers();
    if providers.is_empty() {
        empty(c, page, "No usage to show", 13.0, DIM);
        return;
    }
    let index = app.usage.index.min(providers.len() - 1);
    let provider = &providers[index];

    let card = Rect::new(page.x + 2.0, page.y, page.w - 32.0, page.h);
    let content = 34.0 + provider.limits.len() as f32 * 40.0;
    let mut y = card.y + ((card.h - content) / 2.0).max(0.0);

    // The header: logo, name, badge, and when it was read.
    let logo = Rect::new(card.x, y, 20.0, 20.0);
    match provider.id.as_str() {
        "agy" => c.icon(
            Icon::Sparkle,
            logo.x + 10.0,
            logo.y + 10.0,
            18.0,
            0x3B82F6,
            1.0,
        ),
        "system" => c.icon(
            Icon::Gauge,
            logo.x + 8.0,
            logo.y + 11.0,
            16.0,
            0x2EC27E,
            1.6,
        ),
        _ => c.circle(logo.x + 6.0, logo.y + 10.0, 6.0, provider.color),
    }
    c.text(
        &provider.name,
        Rect::new(logo.right() + 8.0, y, 200.0, 20.0),
        14.0,
        BOLD,
        WHITE,
        Align::Left,
    );
    if !provider.badge.is_empty() {
        let badge_x = logo.right() + 8.0 + provider.name.chars().count() as f32 * 7.6 + 8.0;
        c.text(
            &provider.badge,
            Rect::new(badge_x, y + 1.0, 60.0, 18.0),
            10.5,
            NORMAL,
            0x9A9996,
            Align::Left,
        );
    }
    c.text(
        &format!(
            "Updated {:02}:{:02}:{:02}",
            app.clock.hour, app.clock.minute, app.clock.second
        ),
        Rect::new(card.right() - 120.0, y, 120.0, 20.0),
        11.0,
        NORMAL,
        HINT,
        Align::Right,
    );
    y += 34.0;

    // One row per limit: label and reading, then the bar.
    for limit in &provider.limits {
        c.text(
            &limit.label,
            Rect::new(card.x, y, card.w - 170.0, 18.0),
            12.5,
            SEMI,
            WHITE,
            Align::Left,
        );
        c.text(
            &limit.rem,
            Rect::new(card.right() - 156.0, y, 104.0, 18.0),
            11.0,
            NORMAL,
            DIM,
            Align::Right,
        );
        let pct = limit.pct.round().clamp(0.0, 100.0);
        let (reading, fill) = if pct >= 95.0 {
            (0xE01B24u32, 0xE01B24u32)
        } else if pct >= 80.0 {
            (0xE66100, 0xE66100)
        } else {
            (WHITE, provider.color)
        };
        c.text(
            &format!("{pct:.0}%"),
            Rect::new(card.right() - 46.0, y, 46.0, 18.0),
            12.0,
            BOLD,
            reading,
            Align::Right,
        );
        y += 22.0;
        c.rect(Rect::new(card.x, y, card.w, 4.0), 2.0, PILL);
        let used = (card.w * (pct.max(1.0) / 100.0)).max(1.0);
        c.rect(Rect::new(card.x, y, used, 4.0), 2.0, fill);
        y += 18.0;
    }

    // The dots down the right: a tall white one for the plan in view.
    if providers.len() > 1 {
        let dots_h = providers.len() as f32 * 20.0 - 6.0;
        let mut dot_y = page.y + (page.h - dots_h) / 2.0;
        for (position, _) in providers.iter().enumerate() {
            let active = position == index;
            let height = if active { 14.0 } else { 6.0 };
            let dot = Rect::new(
                page.right() - 8.0,
                dot_y + (if active { 3.0 } else { 3.0 }),
                6.0,
                height,
            );
            c.rect(dot, 3.0, if active { WHITE } else { 0x48484A });
            c.hit(
                Rect::new(dot.x - 4.0, dot_y, 14.0, 14.0),
                Action::UsageDot(position),
            );
            dot_y += 20.0;
        }
    }
}

// ---- shared --------------------------------------------------------------

/// How many lines a run of text takes in a fixed width. DirectWrite measures
/// for real when it draws; this estimate is only used for spacing.
fn wrapped(text: &str, size: f32, width: f32) -> u32 {
    if width <= 8.0 {
        return 1;
    }
    let per_line = (width / (size * 0.52)).max(4.0);
    let mut lines = 0;
    for paragraph in text.split('\n') {
        let count = paragraph.chars().count() as f32;
        lines += (count / per_line).ceil().max(1.0) as u32;
    }
    lines.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_box_narrows_the_clipboard_ignoring_case() {
        assert!(listed("Hello World", 0, ""));
        assert!(listed("Hello World", 0, "hello"));
        assert!(listed("Hello World", 0, "world"));
        assert!(!listed("Hello World", 0, "bye"));
    }

    #[test]
    fn the_search_stacks_on_top_of_the_filter_pill() {
        assert!(listed("https://example.com", 1, "example"));
        assert!(!listed("https://example.com", 1, "photo"));
        // A link under the links filter still has to match the query.
        assert!(!listed("https://example.com", 1, "shop"));
        // The images filter keeps pictures, and the query then narrows them.
        assert!(listed("holiday.png", 2, "holi"));
        assert!(!listed("holiday.png", 2, "work"));
    }

    #[test]
    fn months_have_the_right_number_of_days() {
        assert_eq!(days_in_month(2026, 1), 31);
        assert_eq!(days_in_month(2026, 4), 30);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
    }

    #[test]
    fn the_first_of_a_month_lands_on_the_right_weekday() {
        // 1 October 2026 is a Thursday; 1 January 2026 a Thursday too.
        assert_eq!(weekday_of_first(2026, 10), 4);
        assert_eq!(weekday_of_first(2026, 1), 4);
        // 1 January 2024 was a Monday.
        assert_eq!(weekday_of_first(2024, 1), 1);
        // 1 March 2024 was a Friday.
        assert_eq!(weekday_of_first(2024, 3), 5);
    }

    #[test]
    fn long_text_takes_more_lines() {
        assert_eq!(wrapped("short", 12.0, 200.0), 1);
        assert!(wrapped(&"word ".repeat(40), 12.0, 200.0) > 3);
        assert_eq!(wrapped("", 12.0, 200.0), 1);
    }

    #[test]
    fn the_clipboard_filters_match_the_script() {
        assert!(keeps("https://example.com", 1));
        assert!(!keeps("hello there", 1));
        assert!(keeps("a picture.png", 2));
        assert!(!keeps("a document.txt", 2));
        assert!(keeps("C:\\Users\\me\\file.txt", 3));
        assert!(!keeps("plain words", 3));
        assert!(keeps("anything at all", 0));
    }

    #[test]
    fn colours_survive_a_round_trip() {
        assert_eq!(hex_of("#3584E4", 0), 0x3584E4);
        assert_eq!(hex_of("3584E4", 0), 0x3584E4);
        assert_eq!(hex_of("nonsense", 0x111111), 0x111111);
    }
}
