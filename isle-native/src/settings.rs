//! Where the island keeps what it remembers, and how it reads it back.
//!
//! Everything lives in `%APPDATA%\Isle`, the same place the script build uses,
//! in the same JSON, so a user can move between the two builds and keep their
//! reminders, clipboard and shelf. The script wrote those files with
//! `Set-Content -Encoding UTF8`, which on PowerShell 5.1 means UTF-8 *with* a
//! byte-order mark, so every read here tolerates one.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `%APPDATA%\Isle`, or the working directory if the environment is missing.
pub fn config_dir() -> PathBuf {
    match std::env::var_os("APPDATA") {
        Some(appdata) => PathBuf::from(appdata).join("Isle"),
        None => PathBuf::from(".isle"),
    }
}

/// Copies the files of a pre-`%APPDATA%` build across once, and leaves the
/// originals where they are: moving house must not throw anything away.
pub fn migrate_legacy(dir: &Path) {
    let legacy = match std::env::var_os("USERPROFILE") {
        Some(home) => PathBuf::from(home).join(".config").join("isle"),
        None => return,
    };
    let marker = dir.join(".migrated");
    if !legacy.is_dir() || marker.exists() {
        return;
    }
    let _ = fs::create_dir_all(dir);
    if let Ok(entries) = fs::read_dir(&legacy) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(name) = path.file_name() else {
                continue;
            };
            let kept = dir.join(name);
            if !kept.exists() {
                let _ = fs::copy(&path, &kept);
            }
        }
    }
    let stamp = crate::sys::now_string();
    let _ = fs::write(&marker, stamp);
}

/// Reads a JSON file, without the BOM the script build left on it. Anything
/// unreadable is simply absent: a corrupt file must never stop the island.
pub fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    let bytes = bytes
        .strip_prefix(&[0xEF, 0xBB, 0xBF][..])
        .unwrap_or(&bytes);
    serde_json::from_slice(bytes).ok()
}

/// Reads a list, tolerating the single-object form PowerShell writes for a
/// one-item array.
pub fn read_list<T: for<'de> Deserialize<'de>>(path: &Path) -> Vec<T> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Vec::new(),
    };
    let bytes = bytes
        .strip_prefix(&[0xEF, 0xBB, 0xBF][..])
        .unwrap_or(&bytes);
    let value: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|item| serde_json::from_value(item).ok())
            .collect(),
        serde_json::Value::Null => Vec::new(),
        other => serde_json::from_value(other).into_iter().collect(),
    }
}

/// Writes a file the way the script did, with the trailing newline that
/// `Set-Content` adds, through a temporary file so a reader never sees half of it.
pub fn write_json<T: Serialize>(path: &Path, value: &T) {
    let Ok(text) = serde_json::to_string_pretty(value) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, text + "\r\n").is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn on_by_default() -> bool {
    true
}

/// The settings the script build shipped with. Field names are the JSON keys,
/// so an existing `settings.json` loads unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub top_bar_style: String,
    pub compact_top_bar: bool,
    pub island_in_top_bar: bool,
    pub island_height: i32,
    pub open_mode: String,
    pub rest_delay: u64,
    #[serde(default = "on_by_default")]
    pub alert_volume: bool,
    #[serde(default = "on_by_default")]
    pub alert_charging: bool,
    #[serde(default = "on_by_default")]
    pub alert_network: bool,
    #[serde(default = "on_by_default")]
    pub alert_bluetooth: bool,
    #[serde(default = "on_by_default")]
    pub alert_caps_lock: bool,
    pub dnd: bool,
    pub focus_work_min: u32,
    pub focus_break_min: u32,
    pub clipboard_keep_days: u32,
    pub ai_provider: String,
    pub ai_model: String,
    pub assistant_shape: String,
    pub assistant_color: String,
    /// Picked from the preferences' frame-rate menu; absent until then, when
    /// the island simply draws as fast as it is woken.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub target_fps: f64,
    /// Written by the glow menu. The script stored it and never read it back.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub glow_mode: String,
    /// The display menu, kept for the same reason as `glow_mode`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_mode: String,
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            top_bar_style: "glass".into(),
            compact_top_bar: true,
            island_in_top_bar: true,
            island_height: 32,
            open_mode: "hover".into(),
            rest_delay: 120,
            alert_volume: true,
            alert_charging: true,
            alert_network: true,
            alert_bluetooth: true,
            alert_caps_lock: true,
            dnd: false,
            focus_work_min: 25,
            focus_break_min: 5,
            clipboard_keep_days: 30,
            ai_provider: "Antigravity".into(),
            ai_model: "Gemini Flash".into(),
            assistant_shape: "ShapeGhost".into(),
            assistant_color: "ColEyedropper".into(),
            target_fps: 0.0,
            glow_mode: String::new(),
            display_mode: String::new(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        read_json::<Settings>(path).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        write_json(path, self);
    }

    /// Opening on hover (the default) versus on click, as the preferences offer.
    pub fn opens_on_hover(&self) -> bool {
        self.open_mode != "click"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("isle-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn defaults_match_the_script_build() {
        let settings = Settings::default();
        assert_eq!(settings.top_bar_style, "glass");
        assert_eq!(settings.island_height, 32);
        assert_eq!(settings.open_mode, "hover");
        assert_eq!(settings.rest_delay, 120);
        assert_eq!(settings.focus_work_min, 25);
        assert_eq!(settings.focus_break_min, 5);
        assert_eq!(settings.ai_provider, "Antigravity");
        assert_eq!(settings.ai_model, "Gemini Flash");
        assert!(settings.alert_caps_lock);
        assert!(!settings.dnd);
        assert!(settings.opens_on_hover());
    }

    #[test]
    fn settings_round_trip_through_the_bom_powershell_wrote() {
        let dir = temp_dir("settings");
        let path = dir.join("settings.json");
        let mut settings = Settings::default();
        settings.dnd = true;
        settings.rest_delay = 200;
        settings.ai_model = "Gemini Pro".into();
        settings.save(&path);

        let text = fs::read(&path).unwrap();
        let with_bom = [&[0xEF, 0xBB, 0xBF][..], &text[..]].concat();
        fs::write(&path, with_bom).unwrap();

        let loaded = Settings::load(&path);
        assert!(loaded.dnd);
        assert_eq!(loaded.rest_delay, 200);
        assert_eq!(loaded.ai_model, "Gemini Pro");
        assert_eq!(loaded.island_height, 32, "missing keys keep their default");
    }

    #[test]
    fn a_broken_file_never_stops_the_island() {
        let dir = temp_dir("broken");
        let path = dir.join("settings.json");
        fs::write(&path, b"{ this is not json").unwrap();
        assert_eq!(Settings::load(&path).top_bar_style, "glass");
    }
}
