//! What the conversation does with a typed line.
//!
//! The script looked at the text five times, in this order: a volume figure, a
//! program to open, a package to install, a shell command, and finally the
//! assistant. The first three act at once, the two command stages hand back a
//! card that runs only after Run is pressed, and anything else is prose for
//! whichever assistant the preferences name.

use std::ffi::OsStr;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// Windows' "do not show a console" flag for the helpers we start.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What a typed line turned into.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// A share of the master volume, from 0 to 100.
    Volume(u32),
    /// A program, or a name on the Start menu, to open.
    Open(String),
    /// A line that runs only once the card's Run is pressed: the sentence that
    /// introduces it, and the command itself.
    Card { say: String, line: String },
    /// Nothing local claimed it, so the assistant answers.
    Assistant,
}

/// Reads the line the way the script's chain of patterns did.
pub fn plan(text: &str) -> Plan {
    if let Some(share) = volume_share(text) {
        return Plan::Volume(share);
    }
    if let Some(target) = open_target(text) {
        // Terminals are left to the assistant: the script refused to launch
        // them, and fell through to the stages below.
        let lower = target.to_lowercase();
        let terminal = ["terminal", "command", "powershell", "cmd"]
            .iter()
            .any(|word| lower.contains(word));
        if !terminal {
            return Plan::Open(target);
        }
    }
    if let Some(pkg) = package(text) {
        return Plan::Card {
            say: format!("Here is the install for {pkg}:"),
            line: install_line(&pkg),
        };
    }
    if is_shell(text) {
        return Plan::Card {
            say: "I'll run this:".to_string(),
            line: text.trim().to_string(),
        };
    }
    Plan::Assistant
}

/// `(?:volume|sound|suara)\s*(?:to|ke)?\s*(\d{1,3})`, anywhere in the line.
fn volume_share(text: &str) -> Option<u32> {
    let lower = text.to_lowercase();
    let mut best: Option<(usize, u32)> = None;
    for keyword in ["volume", "sound", "suara"] {
        let mut from = 0;
        while let Some(offset) = lower[from..].find(keyword) {
            let at = from + offset;
            from = at + keyword.len();
            if let Some(share) = share_after(&lower, from) {
                if best.is_none_or(|(best_at, _)| at < best_at) {
                    best = Some((at, share));
                }
                break;
            }
        }
    }
    best.map(|(_, share)| share)
}

/// The digits that follow the keyword, past an optional "to" or "ke".
fn share_after(text: &str, from: usize) -> Option<u32> {
    let rest = text[from..].trim_start();
    let rest = rest
        .strip_prefix("to")
        .or_else(|| rest.strip_prefix("ke"))
        .unwrap_or(rest)
        .trim_start();
    let digits: String = rest
        .chars()
        .take_while(char::is_ascii_digit)
        .take(3)
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

/// `^(?:open|launch|start|run|buka|jalankan)\s+([a-zA-Z0-9_\-. \t]+)$`.
fn open_target(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for verb in ["open", "launch", "start", "run", "buka", "jalankan"] {
        if !lower.starts_with(verb) {
            continue;
        }
        let rest = &text[verb.len()..];
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let target = rest.trim();
        if target.is_empty() {
            return None;
        }
        let plain = target.chars().all(|c| {
            c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c.is_whitespace()
        });
        return if plain {
            Some(target.to_string())
        } else {
            None
        };
    }
    None
}

/// `(?:install|unduh|pasang)\s+([a-zA-Z0-9_.\-]+)`.
fn package(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for keyword in ["install", "unduh", "pasang"] {
        let mut from = 0;
        while let Some(offset) = lower[from..].find(keyword) {
            let at = from + offset;
            from = at + keyword.len();
            let rest = &text[from..];
            let trimmed = rest.trim_start_matches(char::is_whitespace);
            if trimmed.len() == rest.len() {
                continue;
            }
            let name: String = trimmed
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.' || *c == '-')
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// The prefixes the script ran itself instead of asking the assistant.
fn is_shell(text: &str) -> bool {
    let lower = text.to_lowercase();
    for head in ["ipconfig", "netstat", "systeminfo", "get-"] {
        if lower.starts_with(head) {
            return true;
        }
    }
    for head in ["ping", "dir", "ls"] {
        if let Some(rest) = lower.strip_prefix(head) {
            if rest.starts_with(char::is_whitespace) {
                return true;
            }
        }
    }
    false
}

/// The three packages the script knew by more than one name.
fn install_line(pkg: &str) -> String {
    let lower = pkg.to_lowercase();
    if lower.contains("vscode") {
        "winget install -e --id Microsoft.VisualStudioCode".to_string()
    } else if lower.contains("git") {
        "winget install -e --id Git.Git".to_string()
    } else if lower.contains("node") {
        "winget install -e --id OpenJS.NodeJS".to_string()
    } else {
        format!("winget install {pkg}")
    }
}

// ---- opening a program ----------------------------------------------------

/// Opens a program or a Start-menu entry, the way `Start-Process` did.
pub fn launch(target: &str) -> bool {
    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    if shell_open(OsStr::new(target)) {
        return true;
    }
    // What people type is the name on the Start menu, which is not always a
    // file on the path.
    let needle = target.to_lowercase();
    let menus = start_menus();
    for root in menus {
        if let Some(shortcut) = find_shortcut(&root, &needle, 0) {
            return shell_open(shortcut.as_os_str());
        }
    }
    false
}

fn start_menus() -> Vec<PathBuf> {
    let mut menus = Vec::new();
    for (variable, tail) in [
        ("APPDATA", "Microsoft\\Windows\\Start Menu\\Programs"),
        ("ProgramData", "Microsoft\\Windows\\Start Menu\\Programs"),
    ] {
        if let Ok(base) = std::env::var(variable) {
            menus.push(Path::new(&base).join(tail));
        }
    }
    menus
}

/// The first shortcut under `root` whose name contains `needle`.
fn find_shortcut(root: &Path, needle: &str, depth: u32) -> Option<PathBuf> {
    if depth > 4 {
        return None;
    }
    let entries = std::fs::read_dir(root).ok()?;
    let mut folders = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            folders.push(path);
            continue;
        }
        let shortcut = path
            .extension()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("lnk"));
        if !shortcut {
            continue;
        }
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if stem.contains(needle) {
            return Some(path);
        }
    }
    folders
        .iter()
        .find_map(|folder| find_shortcut(folder, needle, depth + 1))
}

pub(crate) fn shell_open(target: &OsStr) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let file = wide(target);
    let verb = wide(OsStr::new("open"));
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    result.0 as isize > 32
}

fn wide(value: &OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().chain(std::iter::once(0)).collect()
}

// ---- running an approved command ------------------------------------------

/// What a command finished with, and the last of what it printed.
pub struct Outcome {
    pub code: i32,
    pub tail: String,
}

/// Runs an approved line in a hidden PowerShell, exactly as the card did: the
/// words are the user's, so they travel in a script file rather than on a
/// command line where quoting could be got wrong.
pub fn run(line: &str) -> Option<Receiver<Outcome>> {
    let stamp = stamp();
    let folder = std::env::temp_dir();
    let script = folder.join(format!("isle-run-{stamp}.ps1"));
    let log = folder.join(format!("isle-run-{stamp}.log"));

    let log_path = log.to_string_lossy().replace('\'', "''");
    let body = format!(
        "\u{feff}& {{\n{line}\n}} *> '{log_path}'\n\
         # What the program said it finished with, which is what the card reports.\n\
         if ($LASTEXITCODE) {{ exit $LASTEXITCODE }}\nexit 0\n"
    );
    std::fs::write(&script, body).ok()?;

    let powershell = find("powershell").unwrap_or_else(|| {
        PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe")
    });
    let mut command = Command::new(&powershell);
    command
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(&script)
        // Not redirected on purpose: a full pipe with nobody reading it is
        // exactly what makes a talkative command appear to hang forever.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    quiet(&mut command);

    let mut child = command.spawn().ok()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if started.elapsed().as_secs() > 180 {
                        let _ = child.kill();
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(_) => break,
            }
        }
        let code = child
            .wait()
            .map(|status| status.code().unwrap_or(-1))
            .unwrap_or(-1);
        let tail = tail_of(&log);
        let _ = std::fs::remove_file(&script);
        let _ = std::fs::remove_file(&log);
        let _ = tx.send(Outcome { code, tail });
    });
    Some(rx)
}

/// The last three lines the command printed that were not blank.
fn tail_of(log: &Path) -> String {
    let Ok(bytes) = std::fs::read(log) else {
        return String::new();
    };
    let text = decode(&bytes);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(3)..].join("\n")
}

/// PowerShell 5.1's `>` writes UTF-16, everything else UTF-8: read either.
fn decode(bytes: &[u8]) -> String {
    if let [0xFF, 0xFE, rest @ ..] = bytes {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).to_string()
}

// ---- the assistant --------------------------------------------------------

/// The name the settings use, as the script labelled it.
pub fn label(provider: &str) -> &'static str {
    let lower = provider.to_lowercase();
    if lower.starts_with("claude") {
        "Claude Code"
    } else if lower.starts_with("codex") {
        "Codex"
    } else {
        "Antigravity"
    }
}

/// How asking the assistant went.
pub enum Spawn {
    /// Reading an answer off the pipe.
    Started(Receiver<String>),
    /// The chosen assistant is not on this PC.
    NotInstalled,
    /// It is there, but would not start.
    Failed(String),
}

/// One question in, one answer out, off the interface thread.
pub fn ask(provider: &str, model: &str, prompt: &str) -> Spawn {
    let flat = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = provider.to_lowercase();
    let who = label(provider);

    let (program, args) = if lower.starts_with("claude") {
        let Some(program) = find("claude") else {
            return Spawn::NotInstalled;
        };
        let mut args = vec![
            "-p".to_string(),
            flat,
            "--output-format".into(),
            "text".into(),
        ];
        if !model.trim().is_empty() {
            args.push("--model".into());
            args.push(model.to_lowercase());
        }
        (program, args)
    } else if lower.starts_with("codex") {
        let Some(program) = find("codex") else {
            return Spawn::NotInstalled;
        };
        (program, vec!["exec".to_string(), flat])
    } else {
        let Some(program) = agy() else {
            return Spawn::NotInstalled;
        };
        (
            program,
            vec![
                "-p".to_string(),
                flat,
                "--output-format".into(),
                "text".into(),
            ],
        )
    };

    let mut command = assistant_command(&program, &args);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Spawn::Failed(error.to_string()),
    };

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Emptied as it writes, not afterwards: an answer longer than the pipe
        // holds would otherwise leave the assistant waiting on a reader that
        // only starts looking once it has exited.
        let out_reader = child
            .stdout
            .take()
            .map(|pipe| std::thread::spawn(move || read_all(pipe)));
        let err_reader = child
            .stderr
            .take()
            .map(|pipe| std::thread::spawn(move || read_all(pipe)));

        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    // Waiting is not the same as working: after three minutes,
                    // let it go.
                    if started.elapsed().as_secs() > 180 {
                        let _ = child.kill();
                    }
                    std::thread::sleep(Duration::from_millis(120));
                }
                Err(_) => break,
            }
        }
        let _ = child.wait();
        let out = out_reader
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default();
        let err = err_reader
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default();

        let mut answer = clean(&out);
        if answer.is_empty() {
            answer = if err.trim().is_empty() {
                format!("No answer came back. Is {who} signed in?")
            } else {
                err.trim().to_string()
            };
        }
        let _ = tx.send(answer);
    });
    Spawn::Started(rx)
}

/// The assistant as a child process, with both pipes open.
fn assistant_command(program: &Path, args: &[String]) -> Command {
    let shim = program
        .extension()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("cmd") || kind.eq_ignore_ascii_case("bat"));
    let mut command = if shim {
        // An assistant installed by npm arrives as a .cmd shim, which only
        // cmd.exe can start.
        let comspec = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
        let mut line = format!("\"{}\"", program.display());
        for arg in args {
            line.push(' ');
            line.push_str(&quote(arg));
        }
        let mut command = Command::new(comspec);
        command.raw_arg(format!("/c \"{line}\""));
        command
    } else {
        let mut command = Command::new(program);
        command.args(args);
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    quiet(&mut command);
    command
}

fn quote(arg: &str) -> String {
    if arg.is_empty() || arg.chars().any(|c| c.is_whitespace() || c == '"') {
        format!("\"{}\"", arg.replace('"', "'"))
    } else {
        arg.to_string()
    }
}

/// Strips the `[agy]` breadcrumbs the Antigravity CLI prints before its answer.
fn clean(out: &str) -> String {
    out.lines()
        .map(|line| match line.trim_start().strip_prefix("[agy]") {
            Some(rest) => rest.trim_start(),
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn read_all(mut pipe: impl Read) -> String {
    let mut bytes = Vec::new();
    let _ = pipe.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).to_string()
}

/// The Antigravity CLI's usual home, then the path.
fn agy() -> Option<PathBuf> {
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        let local = Path::new(&base).join("agy").join("bin").join("agy.exe");
        if local.exists() {
            return Some(local);
        }
    }
    find("agy")
}

/// `Get-Command <name>`: the first match on the path, with its extension.
fn find(name: &str) -> Option<PathBuf> {
    if name.contains('\\') || name.contains('/') {
        let direct = PathBuf::from(name);
        return direct.is_file().then_some(direct);
    }
    let path = std::env::var_os("PATH")?;
    for folder in std::env::split_paths(&path) {
        for suffix in [".exe", ".cmd", ".bat", ""] {
            let candidate = folder.join(format!("{name}{suffix}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn quiet(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

fn stamp() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|span| span.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", std::process::id(), nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_lines_are_read_anywhere_in_the_sentence() {
        assert_eq!(plan("volume 40"), Plan::Volume(40));
        assert_eq!(plan("set volume to 55"), Plan::Volume(55));
        assert_eq!(plan("Suara ke 12 please"), Plan::Volume(12));
        assert_eq!(plan("sound 100%"), Plan::Volume(100));
        assert_eq!(plan("volume 150"), Plan::Volume(150));
        assert_eq!(plan("Soundtrack of the summer"), Plan::Assistant);
        assert_eq!(plan("what is my volume"), Plan::Assistant);
    }

    #[test]
    fn opening_a_program_needs_a_verb_at_the_front() {
        assert_eq!(plan("open spotify"), Plan::Open("spotify".into()));
        assert_eq!(
            plan("buka Visual Studio Code"),
            Plan::Open("Visual Studio Code".into())
        );
        assert_eq!(plan("please open spotify"), Plan::Assistant);
        assert_eq!(plan("open /etc/passwd"), Plan::Assistant);
        // A terminal is not opened for the user; the assistant answers instead.
        assert_eq!(plan("open cmd"), Plan::Assistant);
    }

    #[test]
    fn installs_become_winget_cards() {
        assert_eq!(
            plan("install node"),
            Plan::Card {
                say: "Here is the install for node:".into(),
                line: "winget install -e --id OpenJS.NodeJS".into()
            }
        );
        assert_eq!(
            plan("pasang 7zip"),
            Plan::Card {
                say: "Here is the install for 7zip:".into(),
                line: "winget install 7zip".into()
            }
        );
    }

    #[test]
    fn shell_words_become_cards_that_wait_for_run() {
        assert_eq!(
            plan("ipconfig /all"),
            Plan::Card {
                say: "I'll run this:".into(),
                line: "ipconfig /all".into()
            }
        );
        assert_eq!(
            plan("dir C:\\Windows"),
            Plan::Card {
                say: "I'll run this:".into(),
                line: "dir C:\\Windows".into()
            }
        );
        assert_eq!(plan("directory listing"), Plan::Assistant);
    }

    #[test]
    fn everything_else_is_for_the_assistant() {
        assert_eq!(plan("how do I rename a branch?"), Plan::Assistant);
        assert_eq!(plan(""), Plan::Assistant);
    }

    #[test]
    fn the_agy_breadcrumb_line_is_dropped_from_an_answer() {
        assert_eq!(clean("[agy] hello\nthere\n  [agy]  x"), "hello\nthere\nx");
        // Only the ends are trimmed; an indent inside the answer survives.
        assert_eq!(clean("one\n  two"), "one\n  two");
    }

    #[test]
    fn utf16_logs_are_decoded_like_powershell_wrote_them() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "done\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode(&bytes), "done\n");
        assert_eq!(decode(b"done\n"), "done\n");
    }

    #[test]
    fn labels_follow_the_provider() {
        assert_eq!(label("Claude"), "Claude Code");
        assert_eq!(label("codex"), "Codex");
        assert_eq!(label("Antigravity"), "Antigravity");
        assert_eq!(label(""), "Antigravity");
    }
}
