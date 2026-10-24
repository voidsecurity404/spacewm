//! Installed applications from `.desktop` files (the freedesktop "Desktop Entry" spec).
//!
//! Files live in `applications/` under $XDG_DATA_HOME (~/.local/share) and each of
//! $XDG_DATA_DIRS (/usr/local/share:/usr/share). A file in an earlier directory hides one
//! with the same id (file name) in a later one, so a user's copy overrides the system's.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq)]
pub struct DesktopEntry {
    /// File name, e.g. "firefox.desktop".
    pub id: String,
    pub name: String,
    /// The command line with field codes (%f, %U, ...) removed.
    pub exec: String,
    /// Needs a terminal (htop, vim).
    pub terminal: bool,
}

/// Parse one file's text. None for entries that should not be listed.
pub fn parse(id: &str, text: &str) -> Option<DesktopEntry> {
    let mut in_main = false;
    let (mut name, mut exec, mut terminal) = (None, None, false);
    let (mut kind, mut hidden, mut try_exec) = (None, false, None);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            // Only the main group; "[Desktop Action new-window]" etc. are extra actions.
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        match key {
            // Unlocalized Name; "Name[uz]" etc. are translations.
            "Name" => name = Some(unescape(value)),
            "Exec" => exec = Some(value.to_string()),
            "Type" => kind = Some(value.to_string()),
            "Terminal" => terminal = value == "true",
            "NoDisplay" | "Hidden" => hidden |= value == "true",
            "TryExec" => try_exec = Some(value.to_string()),
            _ => {}
        }
    }
    if hidden || kind.as_deref() != Some("Application") {
        return None;
    }
    if let Some(program) = try_exec {
        if !program_exists(&program) {
            return None;
        }
    }
    let exec = strip_field_codes(&exec?);
    if exec.trim().is_empty() {
        return None;
    }
    Some(DesktopEntry { id: id.to_string(), name: name?, exec, terminal })
}

/// Values may contain \s \n \t \r \\ escapes.
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Remove %f %F %u %U %d %D %n %N %i %c %k %v %m (we launch without files) and turn %%
/// into %.
pub fn strip_field_codes(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            // %% is a literal %, any other code is dropped.
            if let Some('%') = chars.next() {
                out.push('%');
            }
        } else {
            out.push(c);
        }
    }
    // Collapse the gaps left behind ("firefox %u" -> "firefox").
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn program_exists(program: &str) -> bool {
    let p = Path::new(program);
    if p.is_absolute() {
        return p.is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// The `applications` directories in priority order.
pub fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(d) => dirs.push(PathBuf::from(d)),
        None => {
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(home).join(".local/share"));
            }
        }
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(data_dirs.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    dirs.into_iter().map(|d| d.join("applications")).collect()
}

/// Every listed application, sorted by name.
pub fn load_all() -> Vec<DesktopEntry> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for dir in application_dirs() {
        let Ok(files) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<_> = files.filter_map(|f| f.ok()).map(|f| f.path()).collect();
        files.sort();
        for path in files {
            let Some(id) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
                continue;
            };
            if !id.ends_with(".desktop") || !seen.insert(id.clone()) {
                continue;
            }
            // Desktop files are small; refuse anything absurd instead of reading it all.
            let small = std::fs::metadata(&path).is_ok_and(|m| m.len() < 256 * 1024);
            let Ok(text) = (if small { std::fs::read_to_string(&path) } else { continue }) else {
                continue;
            };
            if let Some(entry) = parse(&id, &text) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by_key(|e| e.name.to_lowercase());
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "[Desktop Entry]\nName=Firefox\nName[uz]=Firefoks\n\
        Exec=/usr/lib/firefox/firefox %u\nType=Application\n\
        [Desktop Action new-window]\nName=New Window\nExec=firefox --new-window %u\n";

    #[test]
    fn parses_the_main_group_only() {
        let e = parse("firefox.desktop", FIREFOX).expect("listed");
        assert_eq!(e.name, "Firefox");
        assert_eq!(e.exec, "/usr/lib/firefox/firefox");
        assert!(!e.terminal);
    }

    #[test]
    fn hidden_and_non_apps_are_skipped() {
        assert!(parse("a", "[Desktop Entry]\nName=A\nExec=a\nType=Application\nNoDisplay=true\n")
            .is_none());
        assert!(parse("l", "[Desktop Entry]\nName=L\nType=Link\nURL=https://x\n").is_none());
        assert!(parse(
            "t",
            "[Desktop Entry]\nName=T\nExec=t\nType=Application\n\
                            TryExec=definitely-not-installed-spacewm\n"
        )
        .is_none());
        assert!(parse("n", "[Desktop Entry]\nExec=n\nType=Application\n").is_none(), "no Name");
    }

    #[test]
    fn field_codes_are_removed() {
        assert_eq!(strip_field_codes("foo %U --bar %f"), "foo --bar");
        assert_eq!(strip_field_codes("printf 100%%"), "printf 100%");
        assert_eq!(strip_field_codes("vlc --started-from-file %U"), "vlc --started-from-file");
    }

    #[test]
    fn terminal_apps_are_marked() {
        let e = parse(
            "htop.desktop",
            "[Desktop Entry]\nName=htop\nExec=htop\nTerminal=true\nType=Application\n",
        )
        .expect("listed");
        assert!(e.terminal);
    }

    #[test]
    fn escapes() {
        assert_eq!(unescape(r"a\sb\\c"), r"a b\c");
    }
}
