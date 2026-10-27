//! Music, the lyric stream and the call card: the three pages whose contents
//! come from somewhere other than a list in `%APPDATA%`.

use crate::app::App;
use crate::lyrics::active_line;
use crate::ui::*;

use super::{empty, media_time, pill};

/// The player: 128 px of cover, the track, a scrubber and three controls.
pub fn music(app: &App, c: &mut Canvas, page: Rect) {
    let now = &app.media.now;
    if !now.has_track() {
        empty(c, page, "There's no audio playing....", 13.0, DIM);
        return;
    }

    // Cover, or the dark placeholder with a pause button.
    const COVER: f32 = 128.0;
    let art = Rect::new(page.x, page.y + (page.h - COVER) / 2.0, COVER, COVER);
    match app.art.as_ref() {
        Some((_, bitmap)) => c.image(bitmap, art.x, art.y, COVER, 16.0),
        None => {
            let s = COVER / 112.0;
            c.rect(art, 16.0, 0x1E1E24);
            c.circle_a(art.x + 46.0 * s, art.y + 44.0 * s, 44.0 * s, 0x2A2A38, 0.8);
            c.circle(art.x + 56.0 * s, art.y + 56.0 * s, 28.0 * s, 0xF2F2F7);
            c.rect(
                Rect::new(art.x + 48.0 * s, art.y + 52.0 * s, 5.0 * s, 8.0 * s),
                2.0,
                0x1C1C1E,
            );
            c.rect(
                Rect::new(art.x + 59.0 * s, art.y + 52.0 * s, 5.0 * s, 8.0 * s),
                2.0,
                0x1C1C1E,
            );
        }
    }

    // The round badge of the app making sound, over the cover's lower-right -
    // the way a player stamps its own mark on the sleeve. Nudged up and in so
    // it sits clear of the artwork's edge instead of straddling the corner.
    source_badge(c, app, art.right() - 12.0, art.bottom() - 12.0);

    let dx = page.x + 144.0;
    let dw = page.right() - dx;
    let y0 = page.y + ((page.h - 96.0) / 2.0).max(0.0);

    c.text(
        &now.title,
        Rect::new(dx, y0, dw - 64.0, 24.0),
        18.0,
        BOLD,
        WHITE,
        Align::Left,
    );
    c.text(
        &now.artist,
        Rect::new(dx, y0 + 24.0, dw - 64.0, 18.0),
        13.0,
        NORMAL,
        DIM,
        Align::Left,
    );

    let output = Rect::new(page.right() - 26.0, y0, 26.0, 26.0);
    let lyrics = Rect::new(output.x - 32.0, y0, 26.0, 26.0);
    c.icon(Icon::Chat, lyrics.x + 13.0, lyrics.y + 13.0, 15.0, DIM, 1.6);
    c.icon(
        Icon::Output,
        output.x + 13.0,
        output.y + 13.0,
        14.0,
        DIM,
        1.6,
    );
    c.hit(lyrics, Action::LyricsMode);
    c.hit(output, Action::OutputDevice);

    // The scrubber: elapsed on the left, what is left on the right.
    let row = Rect::new(dx, y0 + 42.0, dw, 16.0);
    let track = Rect::new(
        dx + 42.0,
        row.y + 6.5,
        (page.right() - 56.0) - (dx + 42.0),
        3.0,
    );
    let ratio = if now.duration_ms > 0 {
        (app.media.position_ms() as f32 / now.duration_ms as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    c.text(
        &media_time(app.media.position_ms()),
        Rect::new(dx, row.y, 34.0, 16.0),
        11.0,
        NORMAL,
        DIM,
        Align::Left,
    );
    c.rect(track, 1.5, 0x3A3A3C);
    c.rect(
        Rect::new(track.x, track.y, track.w * ratio, track.h),
        1.5,
        WHITE,
    );
    c.circle(track.x + track.w * ratio, track.y + 1.5, 3.0, WHITE);
    c.text(
        &format!(
            "-{}",
            media_time(now.duration_ms.saturating_sub(app.media.position_ms()))
        ),
        Rect::new(page.right() - 40.0, row.y, 40.0, 16.0),
        11.0,
        NORMAL,
        DIM,
        Align::Right,
    );
    let seek = Rect::new(track.x - 6.0, row.y, track.w + 12.0, 16.0);
    c.hit(seek, Action::SeekMusic);
    app.seek_bar.set(seek);

    // Previous, play or pause, next.
    let centre = dx + dw / 2.0;
    let play = Rect::new(centre - 13.0, y0 + 70.0, 26.0, 26.0);
    let previous = Rect::new(play.x - 50.0, play.y, 26.0, 26.0);
    let next = Rect::new(play.x + 50.0, play.y, 26.0, 26.0);
    c.icon(
        Icon::Prev,
        previous.x + 13.0,
        previous.y + 13.0,
        16.0,
        WHITE,
        1.0,
    );
    if now.playing {
        c.icon(Icon::Pause, play.x + 13.0, play.y + 13.0, 16.0, WHITE, 1.0);
    } else {
        c.icon(Icon::Play, play.x + 13.0, play.y + 13.0, 16.0, WHITE, 1.0);
    }
    c.icon(Icon::Next, next.x + 13.0, next.y + 13.0, 16.0, WHITE, 1.0);
    c.hit(previous, Action::PrevTrack);
    c.hit(play, Action::PlayPause);
    c.hit(next, Action::NextTrack);
}

/// The round badge of whichever app is making sound, centred on `cx`/`cy`.
/// A dark disc sits beneath so the mark keeps an edge over any sleeve; the
/// icon itself is the source application's, resolved on the media worker.
fn source_badge(c: &mut Canvas, app: &App, cx: f32, cy: f32) {
    if let Some((_, bitmap)) = app.app_icon.as_ref() {
        const SIZE: f32 = 24.0;
        c.circle(cx, cy, SIZE / 2.0 + 2.0, 0x141416);
        c.image(bitmap, cx - SIZE / 2.0, cy - SIZE / 2.0, SIZE, SIZE / 2.0);
    }
}

/// The lyric mode: cover and controls on the left, the stream on the right.
pub fn lyrics_page(app: &App, c: &mut Canvas, page: Rect) {
    let now = &app.media.now;
    let left_w = 250.0;
    let y0 = page.y + ((page.h - 130.0) / 2.0).max(0.0);

    // The cover sits at the left with the track's name stacked beside it, the
    // way the reference lays the lyric card out; the scrubber and the controls
    // then run under both.
    let cover = Rect::new(page.x, y0, 68.0, 68.0);
    match app.art.as_ref() {
        Some((_, bitmap)) => c.image(bitmap, cover.x, cover.y, 68.0, 12.0),
        None => {
            c.rect(cover, 12.0, 0x4A2840);
            c.circle_a(cover.x + 27.0, cover.y + 48.0, 36.0, 0xBA4E3E, 0.75);
        }
    }
    let tx = cover.right() + 12.0;
    let tw = (page.x + left_w) - tx;
    c.text(
        &now.title,
        Rect::new(tx, y0 + 12.0, tw, 16.0),
        14.0,
        BOLD,
        WHITE,
        Align::Left,
    );
    c.text(
        &now.artist,
        Rect::new(tx, y0 + 34.0, tw, 14.0),
        11.0,
        NORMAL,
        DIM,
        Align::Left,
    );

    // The little scrubber under the cover, spanning the column.
    let row = Rect::new(page.x, y0 + 80.0, left_w, 12.0);
    let track = Rect::new(page.x + 34.0, row.y + 5.0, left_w - 34.0 - 34.0, 2.5);
    let ratio = if now.duration_ms > 0 {
        (app.media.position_ms() as f32 / now.duration_ms as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    c.text(
        &media_time(app.media.position_ms()),
        Rect::new(page.x, row.y, 30.0, 12.0),
        10.0,
        NORMAL,
        DIM,
        Align::Left,
    );
    c.rect(track, 1.2, 0x3A3A3C);
    c.rect(
        Rect::new(track.x, track.y, track.w * ratio, track.h),
        1.2,
        WHITE,
    );
    c.text(
        &format!(
            "-{}",
            media_time(now.duration_ms.saturating_sub(app.media.position_ms()))
        ),
        Rect::new(page.x + left_w - 34.0, row.y, 34.0, 12.0),
        10.0,
        NORMAL,
        DIM,
        Align::Right,
    );
    let seek = Rect::new(track.x - 4.0, row.y, track.w + 8.0, 12.0);
    c.hit(seek, Action::SeekLyrics);
    app.seek_bar.set(seek);

    // The control row: back to the player at the left, the transport centred
    // between them, the output device at the right.
    let ctop = y0 + 100.0;
    let back = Rect::new(page.x, ctop, 28.0, 24.0);
    // The active badge is a round disc, a touch fatter than the old oval and
    // nudged down so it sits level with the transport glyphs beside it.
    let bcx = back.x + 14.0;
    let bcy = back.y + 14.0;
    c.circle(bcx, bcy, 15.0, 0x48484A);
    c.icon(Icon::ChatFill, bcx, bcy, 11.0, WHITE, 1.2);
    c.hit(back, Action::BackToMusic);

    let output = Rect::new(page.x + left_w - 26.0, ctop, 26.0, 24.0);
    c.icon(
        Icon::Output,
        output.x + 13.0,
        output.y + 12.0,
        14.0,
        WHITE,
        1.5,
    );
    c.hit(output, Action::OutputDevice);

    let mid = (back.right() + output.x) / 2.0;
    let play = Rect::new(mid - 11.0, ctop + 1.0, 22.0, 22.0);
    let previous = Rect::new(play.x - 34.0, ctop + 1.0, 22.0, 22.0);
    let next = Rect::new(play.right() + 12.0, ctop + 1.0, 22.0, 22.0);
    c.icon(
        Icon::Prev,
        previous.x + 11.0,
        previous.y + 11.0,
        13.0,
        WHITE,
        1.0,
    );
    let kind = if now.playing { Icon::Pause } else { Icon::Play };
    c.icon(kind, play.x + 11.0, play.y + 11.0, 13.0, WHITE, 1.0);
    c.icon(
        Icon::Next,
        next.x + 11.0,
        next.y + 11.0,
        13.0,
        WHITE,
        1.0,
    );
    c.hit(previous, Action::PrevTrack);
    c.hit(play, Action::PlayPause);
    c.hit(next, Action::NextTrack);

    // The lyric stream: the line being sung sits a little above centre and the
    // rest glide past it, the way a karaoke pane drifts upward with the song.
    let view = Rect::new(
        page.x + left_w + 20.0,
        page.y + 4.0,
        page.right() - (page.x + left_w + 20.0),
        page.h - 8.0,
    );
    c.clip(view);

    if app.lyrics.lines.is_empty() {
        let note = app.lyrics.note.clone();
        c.text(&note, view, 13.0, NORMAL, DIM, Align::Center);
        c.unclip();
        return;
    }

    let pitch = crate::lyrics::LINE_PITCH;
    let anchor = view.y + view.h * crate::lyrics::LINE_ANCHOR;
    let active = if app.lyrics.plain {
        None
    } else {
        active_line(&app.lyrics.lines, app.media.position_ms())
    };
    for (index, line) in app.lyrics.lines.iter().enumerate() {
        let centre = anchor + index as f32 * pitch - app.lyric_offset;
        if centre < view.y - pitch || centre > view.bottom() + pitch {
            continue;
        }
        let current = active == Some(index);
        let (size, weight, colour) = if current {
            (16.0, BOLD, WHITE)
        } else {
            // A lavender that survives the sleeve wash behind it.
            (13.0, NORMAL, 0xB4AEBC)
        };
        let slot = Rect::new(view.x, centre - pitch * 0.5, view.w, pitch);
        c.text(&line.text, slot, size, weight, colour, Align::Left);
        if !app.lyrics.plain {
            c.hit(slot, Action::LyricLine(line.at_ms));
        }
    }
    c.unclip();
}

/// A call in progress: a small preview and the controls beside it.
pub fn call(app: &App, c: &mut Canvas, page: Rect) {
    let _ = app;
    // The reference gives the preview a fixed 200 px column and centres the
    // 190 px card in it, so the preview starts five pixels in.
    let preview = Rect::new(page.x + 5.0, page.y + (page.h - 120.0) / 2.0, 190.0, 120.0);
    c.rect(preview, 12.0, 0xD5E3EB);
    c.rect(
        Rect::new(preview.x, preview.y, preview.w, 18.0),
        12.0,
        0x333333,
    );
    c.rect(
        Rect::new(preview.x, preview.y + 12.0, preview.w, 6.0),
        0.0,
        0x333333,
    );
    c.text(
        "Meet - Weekly sync",
        Rect::new(preview.x, preview.y + 1.0, preview.w, 16.0),
        9.0,
        NORMAL,
        0xCCCCCC,
        Align::Center,
    );

    // The participant, and the small self view in the corner.
    let cx = preview.x + preview.w / 2.0;
    c.circle(cx, preview.y + 52.0, 18.0, 0xE6A885);
    c.polygon(
        &[
            (cx - 20.0, preview.y + 78.0),
            (cx - 16.0, preview.y + 66.0),
            (cx + 16.0, preview.y + 66.0),
            (cx + 20.0, preview.y + 78.0),
        ],
        0x446A94,
    );
    c.rect(
        Rect::new(preview.right() - 34.0, preview.bottom() - 34.0, 26.0, 26.0),
        4.0,
        0x5C5C5C,
    );
    c.circle(
        preview.right() - 21.0,
        preview.bottom() - 21.0,
        6.0,
        0xE6A885,
    );

    // The details: service, kind and length, mute and open.
    // The details column begins past the 200 px preview column plus its own
    // 16 px left margin, not snug against the 190 px card.
    let dx = page.x + 216.0;
    let top = page.y + (page.h - 104.0) / 2.0;
    c.icon(Icon::Phone, dx + 7.0, top + 8.0, 14.0, WHITE, 1.4);
    c.text(
        "Google Meet",
        Rect::new(dx + 22.0, top, 220.0, 20.0),
        17.0,
        BOLD,
        WHITE,
        Align::Left,
    );
    c.text(
        "Video call \u{00B7} ",
        Rect::new(dx, top + 26.0, 80.0, 16.0),
        13.0,
        NORMAL,
        DIM,
        Align::Left,
    );
    c.text(
        "1:15",
        Rect::new(dx + 74.0, top + 26.0, 44.0, 16.0),
        13.0,
        BOLD,
        GREEN,
        Align::Left,
    );

    let mut x = dx;
    for (icon, label, action, size) in [
        (Icon::Mic, "Mute", Action::CallMute, 15.0),
        (Icon::Open, "Open", Action::CallJoin, 14.0),
    ] {
        let button = Rect::new(x, top + 52.0, 42.0, 42.0);
        c.rect(button, 21.0, 0x1C1C1E);
        c.icon(icon, button.x + 21.0, button.y + 21.0, size, WHITE, 1.4);
        c.text(
            label,
            Rect::new(button.x - 4.0, button.bottom() + 4.0, button.w + 8.0, 14.0),
            11.0,
            NORMAL,
            DIM,
            Align::Center,
        );
        c.hit(button, action);
        x += 62.0;
    }

    // The call tab is only drawn when a call exists, but the page can be
    // reached from it: the end button lives here so the demo can leave.
    let end = Rect::new(page.right() - 60.0, top + 52.0, 60.0, 26.0);
    pill(c, end, "End", 11.0, WHITE, 0x2C2C2E);
    c.hit(end, Action::CallEnd);
}
