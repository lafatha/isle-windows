//! The three plans the usage pane knows about: Antigravity's token windows
//! (counted from its own transcripts), the machine's own resources, and Claude
//! Code, which is listed when it is signed in here.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

/// The 4-hour session cap and the weekly one, as the plan states them.
const SESSION_CAP: f64 = 500_000.0;
const WEEKLY_CAP: f64 = 5_000_000.0;

#[derive(Clone, Debug)]
pub struct Limit {
    pub label: String,
    pub pct: f32,
    pub rem: String,
}

#[derive(Clone, Debug)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub badge: String,
    pub color: u32,
    pub limits: Vec<Limit>,
}

fn to_u64(value: FILETIME) -> u64 {
    ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
}

struct Cpu {
    idle: std::cell::Cell<u64>,
    kernel: std::cell::Cell<u64>,
    user: std::cell::Cell<u64>,
    last: std::cell::Cell<f32>,
}

impl Cpu {
    fn new() -> Self {
        let cpu = Self {
            idle: std::cell::Cell::new(0),
            kernel: std::cell::Cell::new(0),
            user: std::cell::Cell::new(0),
            last: std::cell::Cell::new(0.0),
        };
        cpu.sample();
        cpu
    }

    fn sample(&self) -> f32 {
        let (mut idle, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        unsafe {
            if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_err() {
                return self.last.get();
            }
        }
        let (idle, kernel, user) = (to_u64(idle), to_u64(kernel), to_u64(user));
        let (d_idle, d_kernel, d_user) = (
            idle.saturating_sub(self.idle.get()),
            kernel.saturating_sub(self.kernel.get()),
            user.saturating_sub(self.user.get()),
        );
        self.idle.set(idle);
        self.kernel.set(kernel);
        self.user.set(user);
        let total = d_kernel + d_user;
        if total == 0 {
            return self.last.get();
        }
        let busy = total.saturating_sub(d_idle) as f64;
        let value = ((busy / total as f64) * 100.0).clamp(0.0, 100.0) as f32;
        self.last.set(value);
        value
    }
}

pub struct Usage {
    cpu: Cpu,
    pub agy_4h: u64,
    pub agy_7d: u64,
    pub first_turn_ms: Option<i64>,
    pub index: usize,
    scan: Option<Receiver<(u64, u64, Option<i64>)>>,
    last_scan: Option<Instant>,
    scoped_in: bool,
}

impl Default for Usage {
    fn default() -> Self {
        let mut usage = Self {
            cpu: Cpu::new(),
            agy_4h: 0,
            agy_7d: 0,
            first_turn_ms: None,
            index: 0,
            scan: None,
            last_scan: None,
            scoped_in: claude_signed_in(),
        };
        usage.start_scan();
        usage
    }
}

impl Usage {
    /// At most one transcript read every 30 seconds; between reads the last
    /// numbers stand.
    fn start_scan(&mut self) {
        let now = Instant::now();
        if let Some(at) = self.last_scan {
            if at.elapsed().as_secs() < 30 {
                return;
            }
        }
        self.last_scan = Some(now);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(scan_transcripts());
        });
        self.scan = Some(rx);
    }

    pub fn poll(&mut self) {
        if let Some(rx) = self.scan.as_ref() {
            match rx.try_recv() {
                Ok((tok_4h, tok_7d, first)) => {
                    self.agy_4h = tok_4h;
                    self.agy_7d = tok_7d;
                    self.first_turn_ms = first;
                    self.scan = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.scan = None,
            }
        }
        if self.scan.is_none() {
            self.start_scan();
        }
        if self.scoped_in != claude_signed_in() {
            self.scoped_in = claude_signed_in();
        }
    }

    /// The plan list. It changes only when a plan is signed in or out.
    pub fn providers(&self) -> Vec<Provider> {
        let mut providers = vec![self.antigravity(), self.system()];
        if self.scoped_in {
            providers.push(Provider {
                id: "claude".into(),
                name: "Claude Code".into(),
                badge: "Pro".into(),
                color: 0xD97757,
                limits: Vec::new(),
            });
        }
        providers
    }

    fn antigravity(&self) -> Provider {
        let pct_4h = ((self.agy_4h as f64 / SESSION_CAP) * 100.0).clamp(0.0, 100.0);
        let pct_7d = ((self.agy_7d as f64 / WEEKLY_CAP) * 100.0).clamp(0.0, 100.0);
        let since_epoch = crate::sys::unix_millis();
        let rem_4h = match self.first_turn_ms {
            Some(first) => format_remaining(first + 4 * 3_600_000 - since_epoch),
            None => "no session yet".into(),
        };
        Provider {
            id: "agy".into(),
            name: "Antigravity".into(),
            badge: "Pro".into(),
            color: 0x3584E4,
            limits: vec![
                Limit {
                    label: "Session Limit (4h)".into(),
                    pct: pct_4h as f32,
                    rem: rem_4h,
                },
                Limit {
                    label: "Weekly Usage".into(),
                    pct: pct_7d as f32,
                    rem: format_remaining(next_monday_ms() - since_epoch),
                },
            ],
        }
    }

    fn system(&self) -> Provider {
        let cpu = self.cpu.sample() as f64;

        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        let (mem_pct, mem_rem) = unsafe {
            if GlobalMemoryStatusEx(&mut status).is_ok() {
                let total = status.ullTotalPhys as f64 / 1_073_741_824.0;
                let used = (status.ullTotalPhys - status.ullAvailPhys) as f64 / 1_073_741_824.0;
                (
                    status.dwMemoryLoad as f64,
                    format!("{used:.1} / {total:.1} GB"),
                )
            } else {
                (0.0, String::new())
            }
        };

        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let root = format!("{drive}\\");
        let (disk_pct, disk_rem) = disk_usage(&root)
            .map(|(free, total)| {
                let pct = if total > 0 {
                    ((total - free) as f64 * 100.0 / total as f64).round()
                } else {
                    0.0
                };
                (pct, format!("{:.1} GB free", free as f64 / 1_073_741_824.0))
            })
            .unwrap_or((0.0, String::new()));

        Provider {
            id: "system".into(),
            name: "System Resources".into(),
            badge: "Live".into(),
            color: 0x2EC27E,
            limits: vec![
                Limit {
                    label: "CPU Load".into(),
                    pct: cpu as f32,
                    rem: "processor activity".into(),
                },
                Limit {
                    label: "Memory (RAM)".into(),
                    pct: mem_pct as f32,
                    rem: mem_rem,
                },
                Limit {
                    label: format!("Drive {} Storage", drive.trim_end_matches('\\')),
                    pct: disk_pct as f32,
                    rem: disk_rem,
                },
            ],
        }
    }
}

/// "3h 20m", "45m" and "resets soon", as `Format-UsageRemaining` printed them.
pub fn format_remaining(span_ms: i64) -> String {
    if span_ms <= 0 {
        return "resets soon".into();
    }
    let minutes = span_ms / 60_000;
    let hours = minutes / 60;
    if hours > 0 {
        format!("resets in {hours}h {}m", minutes % 60)
    } else {
        format!("resets in {minutes}m")
    }
}

/// The next Monday at 00:00 UTC, in milliseconds.
pub fn next_monday_ms() -> i64 {
    let now = crate::sys::unix_millis();
    let days = now.div_euclid(86_400_000);
    // 1970-01-01 was a Thursday, so day 0 is weekday 4 with Sunday as 0.
    let weekday = (days + 4) % 7;
    let mut until_monday = (8 - weekday) % 7;
    if until_monday == 0 {
        until_monday = 7;
    }
    (days + until_monday) * 86_400_000
}

fn disk_usage(root: &str) -> Option<(u64, u64)> {
    use windows::core::HSTRING;
    let wide = HSTRING::from(root);
    let mut free = 0u64;
    let mut total = 0u64;
    let mut available = 0u64;
    unsafe {
        GetDiskFreeSpaceExW(
            &wide,
            Some(&mut available),
            Some(&mut total),
            Some(&mut free),
        )
        .ok()?;
    }
    Some((free, total))
}

fn claude_signed_in() -> bool {
    crate::sys::home_dir()
        .map(|home| home.join(".claude").join(".credentials.json").exists())
        .unwrap_or(false)
}

/// Walks the Antigravity brain folders and totals the tokens of the turns in
/// the last 4 hours and the last 7 days.
pub fn scan_transcripts() -> (u64, u64, Option<i64>) {
    let now = crate::sys::unix_millis();
    let cutoff_4h = now - 4 * 3_600_000;
    let cutoff_7d = now - 7 * 86_400_000;
    let mut total_4h = 0u64;
    let mut total_7d = 0u64;
    let mut first_turn: Option<i64> = None;

    let Some(home) = crate::sys::home_dir() else {
        return (0, 0, None);
    };
    let roots = [
        home.join(".gemini").join("antigravity-ide").join("brain"),
        home.join(".gemini").join("antigravity-cli").join("brain"),
    ];

    let mut files = Vec::new();
    for root in roots.iter().filter(|path| path.is_dir()) {
        collect(&root.clone(), &cutoff_7d, &mut files, 0);
        if files.len() >= 40 {
            break;
        }
    }
    files.truncate(40);

    for file in files {
        for line in tail_lines(&file, 30) {
            if !line.contains("\"type\":\"PLANNER_RESPONSE\"")
                && !line.contains("\"input_tokens\":")
            {
                continue;
            }
            let input = number_after(&line, "\"input_tokens\":").unwrap_or(0);
            let output = number_after(&line, "\"output_tokens\":").unwrap_or(0);
            let stamp = quoted_after(&line, "\"created_at\":")
                .and_then(|value| crate::sys::parse_iso8601(&value))
                .or_else(|| file_millis(&file))
                .unwrap_or(0);
            let sum = input + output;
            if stamp >= cutoff_7d {
                total_7d += sum;
            }
            if stamp >= cutoff_4h {
                total_4h += sum;
                if first_turn.map(|first| stamp < first).unwrap_or(true) {
                    first_turn = Some(stamp);
                }
            }
        }
    }
    (total_4h, total_7d, first_turn)
}

fn collect(dir: &Path, cutoff: &i64, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 4 || out.len() >= 40 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect(&path, cutoff, out, depth + 1);
            if out.len() >= 40 {
                return;
            }
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("transcript") || !name.ends_with(".jsonl") {
            continue;
        }
        if file_millis(&path)
            .map(|stamp| stamp >= *cutoff)
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
}

fn file_millis(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(since.as_millis() as i64)
}

fn tail_lines(path: &Path, count: usize) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].iter().map(|line| line.to_string()).collect()
}

fn number_after(line: &str, key: &str) -> Option<u64> {
    let rest = &line[line.find(key)? + key.len()..];
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn quoted_after(line: &str, key: &str) -> Option<String> {
    let rest = &line[line.find(key)? + key.len()..];
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_time_reads_like_the_script() {
        assert_eq!(format_remaining(-1), "resets soon");
        assert_eq!(format_remaining(0), "resets soon");
        assert_eq!(format_remaining(45 * 60_000), "resets in 45m");
        assert_eq!(
            format_remaining(3 * 3_600_000 + 20 * 60_000),
            "resets in 3h 20m"
        );
    }

    #[test]
    fn the_weekly_reset_is_the_next_monday() {
        let monday = next_monday_ms();
        let days = monday.div_euclid(86_400_000);
        assert_eq!((days + 4) % 7, 1, "day 0 is a Thursday, so Monday is 1");
        assert!(monday > crate::sys::unix_millis());
        assert!(monday - crate::sys::unix_millis() <= 7 * 86_400_000);
    }

    #[test]
    fn token_counts_are_read_out_of_a_transcript_line() {
        let line = r#"{"type":"PLANNER_RESPONSE","created_at":"2026-10-27T09:05:00Z","usage":{"input_tokens":1200,"output_tokens":34}}"#;
        assert_eq!(number_after(line, "\"input_tokens\":"), Some(1200));
        assert_eq!(number_after(line, "\"output_tokens\":"), Some(34));
        assert_eq!(
            quoted_after(line, "\"created_at\":").as_deref(),
            Some("2026-10-27T09:05:00Z")
        );
        assert_eq!(number_after(line, "\"missing\":"), None);
    }

    #[test]
    fn percentages_never_exceed_the_bar() {
        let usage = Usage {
            cpu: Cpu::new(),
            agy_4h: 900_000,
            agy_7d: 9_000_000,
            first_turn_ms: None,
            index: 0,
            scan: None,
            last_scan: Some(Instant::now()),
            scoped_in: false,
        };
        let agy = usage.antigravity();
        assert_eq!(agy.limits[0].pct, 100.0);
        assert_eq!(agy.limits[1].pct, 100.0);
        assert_eq!(agy.limits[0].rem, "no session yet");
        let providers = usage.providers();
        assert_eq!(providers[0].id, "agy");
        assert_eq!(providers[1].id, "system");
        assert_eq!(providers[1].limits.len(), 3);
    }
}
