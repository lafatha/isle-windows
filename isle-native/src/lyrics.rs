//! Lyrics from LRCLIB, fetched off the interface thread and parsed into timed
//! lines. The cleanup and the search URL are the ones the script used, so the
//! same request goes out for the same track.

use std::sync::mpsc::{self, Receiver, TryRecvError};

/// Vertical spacing between lyric lines, and the fraction of the stream's
/// height the line being sung is parked at. The pane and the frame easing share
/// these so the smooth scroll lands the active line where it is drawn.
pub const LINE_PITCH: f32 = 34.0;
pub const LINE_ANCHOR: f32 = 0.36;

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub at_ms: u64,
    pub text: String,
}

pub struct Lyrics {
    pub lines: Vec<Line>,
    pub plain: bool,
    /// What the pane says while there is nothing to sing.
    pub note: String,
    key: String,
    pending: Option<Receiver<Fetched>>,
}

struct Fetched {
    synced: Option<String>,
    plain: Option<String>,
}

impl Default for Lyrics {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            plain: false,
            note: "No song currently playing".into(),
            key: String::new(),
            pending: None,
        }
    }
}

/// `[ar:...]`, `(Remastered)`, ` - Radio Edit`, `feat. X`: the noise LRCLIB
/// does not index on.
pub fn clean_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut depth = 0i32;
    for ch in title.chars() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    // Everything after " - " is a version note far more often than a title.
    if let Some(index) = out.find(" - ") {
        out.truncate(index);
    }
    let lower = out.to_lowercase();
    for marker in [" feat. ", " feat ", " ft. ", " featuring "] {
        if let Some(index) = lower.find(marker) {
            out.truncate(index);
            break;
        }
    }
    out.trim().to_string()
}

pub fn clean_artist(artist: &str) -> String {
    let trimmed = artist.trim();
    if trimmed.to_lowercase().ends_with("topic") && trimmed.len() >= 5 {
        let head = &trimmed[..trimmed.len() - 5];
        return head.trim_end_matches(['-', ' ']).trim().to_string();
    }
    trimmed.to_string()
}

/// The line the pane shows while the request is out.
pub fn search_note(title: &str) -> String {
    format!("Searching lyrics for {}...", clean_title(title))
}

/// Parses `[mm:ss.xx]line`, keeping every line that has words in it.
pub fn parse_lrc(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.trim_end_matches('\r').trim();
        let Some(rest) = raw.strip_prefix('[') else {
            continue;
        };
        let Some(close) = rest.find(']') else {
            continue;
        };
        let stamp = &rest[..close];
        let body = rest[close + 1..].trim();
        let (minutes, seconds) = match stamp.split_once(':') {
            Some(parts) => parts,
            None => continue,
        };
        let Ok(minutes) = minutes.parse::<f64>() else {
            continue;
        };
        let Ok(seconds) = seconds.parse::<f64>() else {
            continue;
        };
        if body.is_empty() {
            continue;
        }
        let at_ms = ((minutes * 60.0) + seconds) * 1000.0;
        lines.push(Line {
            at_ms: at_ms.max(0.0) as u64,
            text: body.to_string(),
        });
    }
    lines.sort_by_key(|line| line.at_ms);
    lines
}

pub fn parse_plain(text: &str) -> Vec<Line> {
    text.split('\n')
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(|line| Line {
            at_ms: 0,
            text: line.to_string(),
        })
        .collect()
}

/// Index of the line being sung, or None before the first one.
pub fn active_line(lines: &[Line], position_ms: u64) -> Option<usize> {
    let mut active = None;
    for (index, line) in lines.iter().enumerate() {
        if position_ms >= line.at_ms {
            active = Some(index);
        } else {
            break;
        }
    }
    active
}

impl Lyrics {
    /// Starts a fetch when the track changes; the pane shows the search note
    /// straight away, as the script did. `duration_ms` (0 when the player has
    /// not reported one yet) only nudges the search toward the right take.
    pub fn ensure(&mut self, title: &str, artist: &str, duration_ms: u64) {
        if title.trim().is_empty() {
            if self.key != "|" {
                self.key = "|".into();
                self.lines.clear();
                self.note = "No song currently playing".into();
            }
            return;
        }
        let clean = clean_title(title);
        let artist = clean_artist(artist);
        let key = format!("{clean}|{artist}");
        if key == self.key {
            return;
        }
        self.key = key;
        self.lines.clear();
        self.note = search_note(&clean);
        self.pending = Some(fetch(clean, artist, duration_ms));
    }

    /// Picks up a finished fetch. Cheap enough to call every frame.
    pub fn poll(&mut self) {
        let Some(rx) = self.pending.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(fetched) => {
                self.pending = None;
                if let Some(synced) = fetched.synced {
                    self.lines = parse_lrc(&synced);
                    self.plain = false;
                }
                if self.lines.is_empty() {
                    if let Some(plain) = fetched.plain {
                        self.lines = parse_plain(&plain);
                        self.plain = true;
                    }
                }
                self.note = if self.lines.is_empty() {
                    "Lyrics unavailable for this track".into()
                } else {
                    String::new()
                };
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                self.note = "Lyrics unavailable for this track".into();
            }
        }
    }
}

fn fetch(title: String, artist: String, duration_ms: u64) -> Receiver<Fetched> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = request(&title, &artist, duration_ms).unwrap_or(Fetched {
            synced: None,
            plain: None,
        });
        let _ = tx.send(result);
    });
    rx
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn request(title: &str, artist: &str, duration_ms: u64) -> Option<Fetched> {
    use windows::core::HSTRING;
    use windows::Foundation::Uri;
    use windows::Web::Http::HttpClient;

    unsafe {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let client = HttpClient::new().ok()?;
    let wanted = (duration_ms / 1000) as f64;
    let have_dur = duration_ms > 0;

    // One GET, parsed into a list of candidate records: `/search` answers with
    // an array, `/get` with a single object, so a lone object is wrapped.
    let get = |url: String| -> Option<Vec<serde_json::Value>> {
        let uri = Uri::CreateUri(&HSTRING::from(url)).ok()?;
        let body = client.GetStringAsync(&uri).ok()?.get().ok()?.to_string();
        match serde_json::from_str::<Vec<serde_json::Value>>(&body) {
            Ok(list) => Some(list),
            Err(_) => serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .filter(|single| !single.is_null())
                .map(|single| vec![single]),
        }
    };

    // The loose queries run without a duration filter: a browser that reports
    // the length a second off would otherwise hide the only match, which is why
    // so many tracks came back empty. The duration only ranks the candidates.
    let mut fallback_plain: Option<String> = None;
    let queries = [
        format!(
            "https://lrclib.net/api/search?track_name={}&artist_name={}",
            escape(title),
            escape(artist)
        ),
        format!("https://lrclib.net/api/search?track_name={}", escape(title)),
        format!(
            "https://lrclib.net/api/search?q={}",
            escape(&format!("{title} {artist}"))
        ),
    ];
    for url in queries {
        if let Some(list) = get(url) {
            if let Some(fetched) = pick(&list, wanted, have_dur) {
                if fetched.synced.is_some() {
                    return Some(fetched);
                }
                if fallback_plain.is_none() {
                    fallback_plain = fetched.plain;
                }
            }
        }
    }
    // The strict endpoint last, when a duration is known to satisfy it.
    if have_dur {
        let url = format!(
            "https://lrclib.net/api/get?track_name={}&artist_name={}&duration={}",
            escape(title),
            escape(artist),
            (duration_ms / 1000) as i64
        );
        if let Some(list) = get(url) {
            if let Some(fetched) = pick(&list, wanted, have_dur) {
                if fetched.synced.is_some() {
                    return Some(fetched);
                }
                if fallback_plain.is_none() {
                    fallback_plain = fetched.plain;
                }
            }
        }
    }
    Some(Fetched {
        synced: None,
        plain: fallback_plain,
    })
}

/// From a batch of LRCLIB records, the best synced lyrics - the one whose
/// length is nearest ours (within a few seconds), else any timed one - and,
/// failing that, any plain text. `None` when the batch holds nothing usable.
fn pick(values: &[serde_json::Value], wanted: f64, have_dur: bool) -> Option<Fetched> {
    let mut best: Option<(f64, String)> = None;
    let mut any_synced: Option<String> = None;
    let mut any_plain: Option<String> = None;
    for value in values {
        let synced = value
            .get("syncedLyrics")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let plain = value
            .get("plainLyrics")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        if let Some(text) = synced {
            if any_synced.is_none() {
                any_synced = Some(text.to_string());
            }
            if have_dur {
                let seconds = value.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let distance = (seconds - wanted).abs();
                if distance <= 3.0
                    && best.as_ref().is_none_or(|(found, _)| distance < *found)
                {
                    best = Some((distance, text.to_string()));
                }
            }
        }
        if any_plain.is_none() {
            if let Some(text) = plain {
                any_plain = Some(text.to_string());
            }
        }
    }
    let synced = best.map(|(_, text)| text).or(any_synced);
    if synced.is_none() && any_plain.is_none() {
        return None;
    }
    Some(Fetched { synced, plain: any_plain })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_lose_their_brackets_and_version_notes() {
        assert_eq!(clean_title("Song (Remastered 2011)"), "Song");
        assert_eq!(clean_title("Song - Radio Edit"), "Song");
        assert_eq!(clean_title("Song feat. Someone"), "Song");
        assert_eq!(clean_title("Song (feat. Someone) [Live]"), "Song");
        assert_eq!(clean_title("Song - Live at Wembley"), "Song");
    }

    #[test]
    fn artist_loses_the_topic_suffix() {
        assert_eq!(clean_artist("Someone - Topic"), "Someone");
        assert_eq!(clean_artist("Someone"), "Someone");
    }

    #[test]
    fn lrc_lines_are_timed_and_sorted() {
        let text =
            "[ar:Someone]\r\n[00:12.50]Second line\r\n[00:01.00]First line\r\n[00:20.00]\r\n";
        let lines = parse_lrc(text);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].at_ms, 1000);
        assert_eq!(lines[0].text, "First line");
        assert_eq!(lines[1].at_ms, 12_500);
        assert_eq!(lines[1].text, "Second line");
    }

    #[test]
    fn lyrics_that_are_not_synced_still_read() {
        let lines = parse_plain("one\n\n  two  \r\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].text, "two");
        assert_eq!(lines[1].at_ms, 0);
    }

    #[test]
    fn the_highlight_follows_the_position() {
        let lines = parse_lrc("[00:01.00]a\n[00:02.00]b\n[00:03.00]c\n");
        assert_eq!(active_line(&lines, 0), None);
        assert_eq!(active_line(&lines, 1500), Some(0));
        assert_eq!(active_line(&lines, 2000), Some(1));
        assert_eq!(active_line(&lines, 99_000), Some(2));
    }

    #[test]
    fn a_track_with_no_name_asks_for_nothing() {
        let mut lyrics = Lyrics::default();
        lyrics.ensure("", "", 0);
        assert_eq!(lyrics.note, "No song currently playing");
        assert!(lyrics.pending.is_none());
    }

    #[test]
    fn the_search_note_names_the_cleaned_title() {
        assert_eq!(search_note("Song (Live)"), "Searching lyrics for Song...");
    }

    #[test]
    fn asking_twice_for_the_same_track_starts_one_search() {
        let mut lyrics = Lyrics::default();
        assert_eq!(lyrics.note, "No song currently playing");
        lyrics.key = "Song|Artist".into();
        lyrics.ensure("Song", "Artist", 0);
        assert!(
            lyrics.pending.is_none(),
            "an unchanged track is not fetched again"
        );
    }
}
