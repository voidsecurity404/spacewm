//! Apps already open outside spacewm: noticing that a start went there, and moving them.
//!
//! Many desktop apps are "single instance": starting one a second time doesn't open a
//! window, it tells the copy that is already running to open one and exits. How they
//! find that copy differs (a D-Bus name, a socket or lock in the profile folder, a pipe
//! like Steam's), and that copy may live outside spacewm (on the desktop spacewm is nested
//! in, or another session), so the window appears THERE.
//!
//! spacewm does not guess per app. It watches what happens: while a start is on its way,
//! it notes every program in its process tree (launcher scripts start the real app).
//! If the start then exits fine without ever showing a window, and one of those
//! programs is running outside spacewm (started before, not by us), the request was
//! handed to that copy. The only way to have the app in spacewm then is to close that
//! copy and start it again here, with the user's usual profile (logins, settings and
//! open tabs all stay). spacewm asks first (desktop.running_elsewhere), and only asks the
//! app to quit (SIGTERM, as closing a window does), so it can save its work.

use std::{collections::HashMap, path::PathBuf};

/// Programs that never mean "this app": shells and helpers a launcher script runs.
const NEVER: &[&str] = &[
    "sh",
    "bash",
    "dash",
    "zsh",
    "fish",
    "env",
    "spacectl",
    "spacewm",
    "xdg-open",
    "gio",
    "python",
    "python3",
    "perl",
    "setsid",
    "nohup",
    "timeout",
    "sleep",
    "flatpak",
    "bwrap",
    "dbus-daemon",
    "dbus-launch",
    "systemd-run",
    "uwsm",
    "uwsm-app",
];

/// The file name of the program a command runs, following PATH and symlinks
/// (/usr/bin/firefox -> firefox; `Telegram` -> Telegram).
pub fn program_name(program: &str) -> Option<String> {
    let path = if program.contains('/') {
        PathBuf::from(program)
    } else {
        let paths = std::env::var_os("PATH")?;
        std::env::split_paths(&paths).map(|d| d.join(program)).find(|p| p.is_file())?
    };
    let real = std::fs::canonicalize(&path).unwrap_or(path);
    Some(real.file_name()?.to_string_lossy().into_owned())
}

/// What /proc/<pid>/stat tells: parent pid, state letter and start time (clock ticks
/// after boot). The line is "pid (comm) state ppid ... starttime ...", where comm may
/// itself contain spaces and parentheses: parse after the LAST ')'. After it, field 3
/// (state) is index 0, so ppid is index 1 and starttime (field 22) index 19.
fn stat(pid: u32) -> Option<(u32, char, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest: Vec<&str> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
    let state = rest.first()?.chars().next()?;
    Some((rest.get(1)?.parse().ok()?, state, rest.get(19)?.parse().ok()?))
}

fn parent_of(pid: u32) -> Option<u32> {
    stat(pid).map(|(ppid, _, _)| ppid)
}

pub fn is_descendant_of(mut pid: u32, ancestor: u32) -> bool {
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

/// The program file name a process runs. A program updated while running shows as
/// "... (deleted)".
fn exe_name(pid: u32) -> Option<String> {
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    Some(exe.file_name()?.to_string_lossy().trim_end_matches(" (deleted)").to_string())
}

/// Our own processes (same user), as pid -> parent pid.
fn our_processes() -> HashMap<u32, u32> {
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let Ok(entries) = std::fs::read_dir("/proc") else { return HashMap::new() };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            use std::os::unix::fs::MetadataExt;
            if entry.metadata().ok()?.uid() != uid {
                return None;
            }
            Some((pid, parent_of(pid)?))
        })
        .collect()
}

/// `root` and every process below it, with the program each runs (helpers left out).
pub fn tree(root: u32) -> Vec<(u32, String)> {
    let procs = our_processes();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, &ppid) in &procs {
        children.entry(ppid).or_default().push(pid);
    }
    let mut out = Vec::new();
    let mut todo = vec![root];
    while let Some(pid) = todo.pop() {
        if out.len() > 512 {
            break;
        }
        if let Some(name) = exe_name(pid) {
            out.push((pid, name));
        }
        todo.extend(children.get(&pid).into_iter().flatten());
    }
    out.retain(|(_, name)| !NEVER.contains(&name.as_str()));
    out
}

/// Clock ticks since boot now (the unit of a process start time).
pub fn now_ticks() -> u64 {
    // SAFETY: sysconf has no preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    let up = std::fs::read_to_string("/proc/uptime").ok();
    let secs = up.and_then(|u| u.split_whitespace().next()?.parse::<f64>().ok()).unwrap_or(0.0);
    (secs * hz) as u64
}

/// Processes running one of `names`, of our user, NOT spacewm's own descendants, started
/// before `before` (ticks): copies that were already open somewhere else. Only the top
/// ones: a browser's helper processes (same program) go when the browser quits.
pub fn running_elsewhere(names: &[String], before: u64) -> Vec<u32> {
    let me = std::process::id();
    let names: Vec<&String> = names.iter().filter(|n| !NEVER.contains(&n.as_str())).collect();
    if names.is_empty() {
        return Vec::new();
    }
    let procs = our_processes();
    let found: Vec<u32> = procs
        .keys()
        .copied()
        .filter(|&pid| {
            exe_name(pid).is_some_and(|n| names.contains(&&n))
                && stat(pid).is_some_and(|(_, state, start)| state != 'Z' && start < before)
                && !is_descendant_of(pid, me)
        })
        .collect();
    let mut tops: Vec<u32> = found
        .iter()
        .copied()
        .filter(|pid| procs.get(pid).is_none_or(|parent| !found.contains(parent)))
        .collect();
    tops.sort_unstable();
    tops
}

/// Still running (a zombie has quit: only its parent hasn't noticed yet)?
pub fn alive(pid: u32) -> bool {
    stat(pid).is_some_and(|(_, state, _)| state != 'Z' && state != 'X')
}

/// Ask a process to quit, as closing its window would (it may save its work first).
pub fn ask_to_quit(pid: u32) {
    // SAFETY: kill(2) with a valid signal number has no memory-safety preconditions.
    let r = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    tracing::info!(pid, ok = r == 0, "asked an app outside spacewm to quit");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_tree() {
        let me = std::process::id();
        assert!(is_descendant_of(me, me));
        // Our parent is not our descendant.
        let parent = parent_of(me).expect("own stat");
        assert!(!is_descendant_of(parent, me));
        // A program nobody runs is never found.
        assert!(running_elsewhere(&["spacewm-no-such-program-xyz".into()], u64::MAX).is_empty());
        assert!(alive(me));
        // Our own tree has us in it (the test binary isn't a helper).
        assert!(tree(me).iter().any(|(pid, _)| *pid == me));
    }

    #[test]
    fn helpers_never_count() {
        // A shell open outside is never "the app" (launcher scripts are shells).
        assert!(running_elsewhere(&["bash".into(), "sh".into()], u64::MAX).is_empty());
    }

    #[test]
    fn start_times() {
        let (_, state, start) = stat(std::process::id()).expect("stat");
        assert!(state != 'Z');
        assert!(start <= now_ticks() + 1, "{start} vs {}", now_ticks());
    }
}
