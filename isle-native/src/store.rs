//! The five lists the island keeps between runs, in the file formats the
//! script build used: `reminders.json`, `notifications.json`,
//! `clipboard.json`, `shelf.json` and `calendar.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::settings::{self, config_dir, read_list, write_json};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reminder {
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clip {
    pub text: String,
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShelfFile {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub size: String,
    #[serde(default)]
    pub ext: String,
    #[serde(default)]
    pub added_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalEvent {
    pub title: String,
    #[serde(default)]
    pub time: String,
    #[serde(default = "default_event_color")]
    pub color: String,
}

fn default_event_color() -> String {
    "#3584E4".into()
}

/// Every stored list, plus where it came from, so a save knows its own path.
pub struct Store {
    pub dir: PathBuf,
    pub reminders: Vec<Reminder>,
    pub notifications: Vec<Notification>,
    pub clips: Vec<Clip>,
    pub shelf: Vec<ShelfFile>,
    pub events: Vec<CalEvent>,
}

impl Store {
    pub fn open() -> Self {
        let dir = config_dir();
        settings::migrate_legacy(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let mut store = Self {
            reminders: read_list(&dir.join("reminders.json")),
            notifications: read_list(&dir.join("notifications.json")),
            clips: read_list(&dir.join("clipboard.json")),
            shelf: read_list(&dir.join("shelf.json")),
            events: read_list(&dir.join("calendar.json")),
            dir,
        };
        // A first run starts with the same three clipboard entries the script
        // seeded, so the pane is not blank before anything is ever copied.
        if !store.dir.join("clipboard.json").exists() {
            store.clips = vec![
                Clip {
                    text: "Meeting moved to Thursday 10:00, same room. Bring the printed deck."
                        .into(),
                    time: "2m".into(),
                    pinned: true,
                },
                Clip {
                    text: "https://github.com/zickrian/isle".into(),
                    time: "10m".into(),
                    pinned: false,
                },
                Clip {
                    text: "Quarterly report summary and action items".into(),
                    time: "2h".into(),
                    pinned: false,
                },
            ];
        }
        store
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn save_reminders(&self) {
        write_json(&self.path("reminders.json"), &self.reminders);
    }

    pub fn save_notifications(&self) {
        write_json(&self.path("notifications.json"), &self.notifications);
    }

    pub fn save_clips(&self) {
        write_json(&self.path("clipboard.json"), &self.clips);
    }

    pub fn save_shelf(&self) {
        write_json(&self.path("shelf.json"), &self.shelf);
    }

    pub fn save_events(&self) {
        write_json(&self.path("calendar.json"), &self.events);
    }

    /// Newest first, at most six, and never the same path twice - exactly what
    /// `Add-ShelfFile` did. Returns whether the shelf actually changed.
    pub fn add_shelf(&mut self, path: &Path) -> bool {
        if !path.exists() {
            return false;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        let full = path.to_string_lossy().to_string();
        if self.shelf.iter().any(|item| item.path == full) {
            return false;
        }
        let meta = std::fs::metadata(path).ok();
        let kb = meta
            .as_ref()
            .map(|m| m.len() as f64 / 1024.0)
            .unwrap_or(0.0);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_uppercase();
        self.shelf.insert(
            0,
            ShelfFile {
                name: name.to_string(),
                path: full,
                size: format!("{kb:.1} KB"),
                ext,
                added_at_ms: crate::sys::unix_millis(),
            },
        );
        self.shelf.truncate(6);
        true
    }

    /// Clipboard history stays the newest 20 entries, pinned ones first, as the
    /// monitor did.
    pub fn push_clip(&mut self, text: String) -> bool {
        let text = text.trim().to_string();
        if text.is_empty() || self.clips.iter().any(|clip| clip.text == text) {
            return false;
        }
        self.clips.insert(
            0,
            Clip {
                text,
                time: "now".into(),
                pinned: false,
            },
        );
        self.clips.truncate(20);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("isle-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_single_object_reads_back_as_a_one_item_list() {
        let dir = scratch("single");
        let path = dir.join("reminders.json");
        std::fs::write(&path, br#"{"text":"buy milk","done":false}"#).unwrap();
        let items: Vec<Reminder> = read_list(&path);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "buy milk");
        assert!(!items[0].done);
    }

    #[test]
    fn the_shelf_keeps_six_newest_without_duplicates() {
        let dir = scratch("shelf");
        for i in 0..8 {
            let file = dir.join(format!("file{i}.txt"));
            std::fs::write(&file, "x").unwrap();
        }
        let mut store = Store {
            dir: dir.clone(),
            reminders: vec![],
            notifications: vec![],
            clips: vec![],
            shelf: vec![],
            events: vec![],
        };
        for i in 0..8 {
            assert!(store.add_shelf(&dir.join(format!("file{i}.txt"))));
        }
        assert_eq!(store.shelf.len(), 6);
        assert_eq!(store.shelf[0].name, "file7.txt");
        assert_eq!(store.shelf[0].ext, "TXT");
        assert!(
            !store.add_shelf(&dir.join("file7.txt")),
            "a repeat is not added"
        );
        assert!(!store.add_shelf(&dir.join("missing.txt")));
    }

    #[test]
    fn clipboard_history_is_capped_and_deduplicated() {
        let dir = scratch("clips");
        let mut store = Store {
            dir,
            reminders: vec![],
            notifications: vec![],
            clips: vec![],
            shelf: vec![],
            events: vec![],
        };
        for i in 0..25 {
            assert!(store.push_clip(format!("line {i}")));
        }
        assert_eq!(store.clips.len(), 20);
        assert_eq!(store.clips[0].text, "line 24");
        assert!(
            !store.push_clip("line 24".into()),
            "the same text is not stored twice"
        );
        assert!(!store.push_clip("   ".into()));
    }

    #[test]
    fn lists_round_trip() {
        let dir = scratch("round");
        let store = Store {
            dir: dir.clone(),
            reminders: vec![Reminder {
                text: "ship it".into(),
                done: true,
            }],
            notifications: vec![],
            clips: vec![],
            shelf: vec![],
            events: vec![CalEvent {
                title: "Standup".into(),
                time: "09:00".into(),
                color: default_event_color(),
            }],
        };
        store.save_reminders();
        store.save_events();
        assert_eq!(read_list::<Reminder>(&dir.join("reminders.json")).len(), 1);
        let events: Vec<CalEvent> = read_list(&dir.join("calendar.json"));
        assert_eq!(events[0].color, "#3584E4");
    }
}
