//! Apps that are already open somewhere else still open inside spacewm.
//!
//! Many desktop apps are "single instance": starting one a second time doesn't open a
//! window, it tells the copy that is already running to open one and exits. That copy
//! may live outside spacewm (on the desktop spacewm is nested in, or another session), so
//! the window would appear THERE. How they find each other:
//!
//! - D-Bus: GTK/GNOME apps (GApplication), many KDE/Qt apps, Firefox on Wayland register
//!   a name on the session bus; a second start finds the name and forwards.
//! - A lock/socket in the profile directory: Telegram, Chromium-based browsers.
//!
//! So when an instance of the same program is running and is NOT one of our own
//! children, spacewm starts this one apart from it: browsers and Telegram get their own
//! profile directory under ~/.local/share/spacewm/apps/ (log in once there; it is kept),
//! everything else gets a private D-Bus session bus. Starting it again from spacewm then
//! reaches the copy inside spacewm, as it should. When nothing runs outside, apps start
//! normally with your usual profile.

use std::{
    io::{BufRead, BufReader},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
};

/// How to keep one app apart from its other running copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Firefox family: `--profile <dir>` (a different profile never talks to the other).
    FirefoxProfile,
    /// Chromium family: `--user-data-dir=<dir>`.
    ChromiumDataDir,
    /// Telegram Desktop: `-workdir <dir>`.
    TelegramWorkdir,
    /// Anything else: a private D-Bus session bus.
    PrivateBus,
}

pub fn rule_for(program: &str) -> Rule {
    match program {
        "firefox"
        | "firefox-esr"
        | "firefox-developer-edition"
        | "librewolf"
        | "waterfox"
        | "floorp"
        | "zen"
        | "zen-browser" => Rule::FirefoxProfile,
        "chromium"
        | "chrome"
        | "google-chrome"
        | "google-chrome-stable"
        | "brave"
        | "brave-browser"
        | "vivaldi"
        | "vivaldi-stable"
        | "microsoft-edge"
        | "microsoft-edge-stable" => Rule::ChromiumDataDir,
        "Telegram" | "telegram-desktop" | "telegram" => Rule::TelegramWorkdir,
        _ => Rule::PrivateBus,
    }
}

/// The command line with the rule's profile option added right after the program (so
/// it comes before any `--` that ends the options).
pub fn apply(rule: Rule, args: &[String], dir: &Path) -> Vec<String> {
    let dir = dir.to_string_lossy().into_owned();
    let extra: Vec<String> = match rule {
        Rule::FirefoxProfile => vec!["--profile".into(), dir],
        Rule::ChromiumDataDir => vec![format!("--user-data-dir={dir}")],
        Rule::TelegramWorkdir => vec!["-workdir".into(), dir],
        Rule::PrivateBus => Vec::new(),
    };
    let mut out = Vec::with_capacity(args.len() + extra.len());
    out.extend(args.first().cloned());
    out.extend(extra);
    out.extend(args.iter().skip(1).cloned());
    out
}

/// The command already chooses its own profile: leave it alone.
fn has_own_profile(rule: Rule, args: &[String]) -> bool {
    let flags: &[&str] = match rule {
        Rule::FirefoxProfile => &["--profile", "-profile", "-P", "--no-remote", "-no-remote"],
        Rule::ChromiumDataDir => &["--user-data-dir"],
        Rule::TelegramWorkdir => &["-workdir", "-many"],
        Rule::PrivateBus => &[],
    };
    args.iter().skip(1).any(|a| flags.iter().any(|f| a == f || a.starts_with(&format!("{f}="))))
}

/// The file name of the program a command runs, following PATH and symlinks
/// (/usr/bin/firefox -> firefox; `Telegram` -> Telegram).
fn program_name(program: &str) -> Option<String> {
    let path = if program.contains('/') {
        PathBuf::from(program)
    } else {
        let paths = std::env::var_os("PATH")?;
        std::env::split_paths(&paths).map(|d| d.join(program)).find(|p| p.is_file())?
    };
    let real = std::fs::canonicalize(&path).unwrap_or(path);
    Some(real.file_name()?.to_string_lossy().into_owned())
}

/// The parent pid of `pid`, from /proc/<pid>/stat ("pid (comm) state ppid ...", where
/// comm may itself contain spaces and parentheses: parse after the LAST ')').
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

fn is_descendant_of(mut pid: u32, ancestor: u32) -> bool {
    // Bounded: a pid chain can't be longer than this, and /proc may change under us.
    for _ in 0..256 {
        if pid == ancestor {
            return true;
        }
        match parent_of(pid) {
            Some(parent) if parent != pid && parent != 0 => pid = parent,
            _ => return false,
        }
    }
    false
}

/// Processes running this program (same executable file name) that belong to us (same
/// user) but are not spacewm's own descendants.
fn running_elsewhere(name: &str) -> Vec<u32> {
    let me = std::process::id();
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            use std::os::unix::fs::MetadataExt;
            (entry.metadata().ok()?.uid() == uid).then_some((pid, entry))
        })
        .filter(|(pid, entry)| {
            // A program updated while running shows as "... (deleted)".
            let exe = std::fs::read_link(entry.path().join("exe")).ok();
            let exe_name = exe
                .as_ref()
                .and_then(|e| e.file_name())
                .map(|n| n.to_string_lossy().trim_end_matches(" (deleted)").to_string());
            exe_name.as_deref() == Some(name) && !is_descendant_of(*pid, me)
        })
        .map(|(pid, _)| pid)
        .collect()
}

/// Well-known names that don't mean "single instance" (media controls, tray icons).
const GENERIC_NAMES: &[&str] =
    &["org.mpris.MediaPlayer2.", "org.kde.StatusNotifierItem-", "org.a11y."];

/// Does one of `pids` own a well-known name on the session bus? That is how D-Bus
/// single-instance apps (GApplication: org.gnome.Nautilus...) find each other; a plain
/// program like a terminal owns none, and then needs no private bus.
fn owns_bus_name(pids: &[u32]) -> bool {
    let out = Command::new("busctl")
        .args(["--user", "--timeout=2", "list", "--no-legend", "--no-pager"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else { return false };
    String::from_utf8_lossy(&out.stdout).lines().any(|line| {
        let mut cols = line.split_whitespace();
        let (Some(name), Some(pid)) = (cols.next(), cols.next()) else { return false };
        !name.starts_with(':')
            && !GENERIC_NAMES.iter().any(|g| name.starts_with(g))
            && pid.parse::<u32>().is_ok_and(|p| pids.contains(&p))
    })
}

/// Where spacewm keeps an app's own profile.
fn profile_dir(name: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    let dir = base.join("spacewm/apps").join(name);
    std::fs::create_dir_all(&dir).ok()?;
    // Profiles hold logins: owner only.
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    Some(dir)
}

/// How a command will actually be started.
#[derive(Debug, Default)]
pub struct Prepared {
    pub args: Vec<String>,
    /// DBUS_SESSION_BUS_ADDRESS to use instead of ours.
    pub bus: Option<String>,
    /// Shown to the user: why this copy is separate.
    pub note: Option<String>,
}

/// Decide how to start `args` (program first).
pub fn prepare(args: &[String]) -> Prepared {
    let plain = Prepared { args: args.to_vec(), ..Default::default() };
    let Some(name) = args.first().and_then(|p| program_name(p)) else { return plain };
    // Shells and helpers we start ourselves are never "single instance".
    if matches!(name.as_str(), "sh" | "bash" | "dash" | "zsh" | "fish" | "env" | "spacectl") {
        return plain;
    }
    let elsewhere = running_elsewhere(&name);
    if elsewhere.is_empty() {
        return plain;
    }
    let rule = rule_for(&name);
    if has_own_profile(rule, args) {
        return plain;
    }
    match rule {
        // Not reachable through the bus (a terminal...): it opens here anyway.
        Rule::PrivateBus if !owns_bus_name(&elsewhere) => plain,
        Rule::PrivateBus => match private_bus() {
            Some(bus) => {
                tracing::info!(app = name, "already running outside spacewm: private D-Bus");
                Prepared {
                    args: args.to_vec(),
                    bus: Some(bus),
                    note: Some(format!(
                        "{name} is also open outside spacewm: this copy runs separately"
                    )),
                }
            }
            None => plain,
        },
        _ => match profile_dir(&name) {
            Some(dir) => {
                tracing::info!(app = name, dir = %dir.display(), "already running outside spacewm: own profile");
                Prepared {
                    args: apply(rule, args, &dir),
                    bus: None,
                    note: Some(format!(
                        "{name} is also open outside spacewm: opened here with its own profile ({})",
                        dir.display()
                    )),
                }
            }
            None => plain,
        },
    }
}

/// The private session bus: a dbus-daemon of our own, started on first use and stopped
/// with spacewm (also if spacewm crashes: it gets SIGTERM when we die).
static BUS: Mutex<Option<(Child, String)>> = Mutex::new(None);

fn private_bus() -> Option<String> {
    let mut guard = BUS.lock().ok()?;
    if let Some((child, address)) = guard.as_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            return Some(address.clone());
        }
        tracing::warn!("private D-Bus daemon died; starting a new one");
    }
    let mut command = Command::new("dbus-daemon");
    command
        .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(|| {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::pthread_sigmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("cannot start a private D-Bus daemon: {e}");
            return None;
        }
    };
    // The daemon prints its address once it listens. Read it on a helper thread with a
    // timeout: a daemon that hangs must not hang spacewm.
    let (tx, rx) = std::sync::mpsc::channel();
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(out).read_line(&mut line);
            let _ = tx.send(line);
        });
    }
    let address = rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .map(|l| l.trim().to_string())
        .unwrap_or_default();
    if address.is_empty() {
        tracing::warn!("private D-Bus daemon gave no address");
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    tracing::info!(address, pid = child.id(), "private D-Bus session bus started");
    *guard = Some((child, address.clone()));
    Some(address)
}

/// Stop the private bus (spacewm is exiting).
pub fn shutdown() {
    if let Ok(mut guard) = BUS.lock() {
        if let Some((mut child, _)) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn rules_put_the_profile_before_other_arguments() {
        let dir = Path::new("/p");
        assert_eq!(
            apply(Rule::TelegramWorkdir, &v(&["Telegram", "--", "tg://x"]), dir),
            v(&["Telegram", "-workdir", "/p", "--", "tg://x"])
        );
        assert_eq!(
            apply(Rule::FirefoxProfile, &v(&["firefox", "https://a"]), dir),
            v(&["firefox", "--profile", "/p", "https://a"])
        );
        assert_eq!(
            apply(Rule::ChromiumDataDir, &v(&["chromium"]), dir),
            v(&["chromium", "--user-data-dir=/p"])
        );
        assert_eq!(apply(Rule::PrivateBus, &v(&["nautilus", "-w"]), dir), v(&["nautilus", "-w"]));
    }

    #[test]
    fn known_apps() {
        assert_eq!(rule_for("firefox"), Rule::FirefoxProfile);
        assert_eq!(rule_for("Telegram"), Rule::TelegramWorkdir);
        assert_eq!(rule_for("brave"), Rule::ChromiumDataDir);
        assert_eq!(rule_for("nautilus"), Rule::PrivateBus);
    }

    #[test]
    fn own_profile_is_respected() {
        assert!(has_own_profile(Rule::FirefoxProfile, &v(&["firefox", "--no-remote"])));
        assert!(has_own_profile(Rule::FirefoxProfile, &v(&["firefox", "--profile", "/x"])));
        assert!(has_own_profile(Rule::ChromiumDataDir, &v(&["chromium", "--user-data-dir=/x"])));
        assert!(!has_own_profile(Rule::FirefoxProfile, &v(&["firefox", "https://a"])));
    }

    #[test]
    fn process_tree() {
        let me = std::process::id();
        assert!(is_descendant_of(me, me));
        // Our parent is not our descendant.
        let parent = parent_of(me).expect("own stat");
        assert!(!is_descendant_of(parent, me));
        // A program nobody runs is never found.
        assert!(running_elsewhere("spacewm-no-such-program-xyz").is_empty());
    }
}
