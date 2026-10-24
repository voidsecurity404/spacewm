//! Small readings for the HUD and the on-screen display: battery, date, keyboard layout,
//! and parsing what volume/brightness tools print.

use std::path::Path;

/// A percentage from a tool's output. wpctl prints "Volume: 0.45 [MUTED]" (a 0..1
/// number), brightnessctl -m prints "intel_backlight,backlight,12000,50%,24000", pactl
/// prints "... 45% ...": the first token ending in '%' wins, else the first number.
pub fn parse_percent(output: &str) -> Option<u32> {
    let tokens = || output.split(|c: char| c.is_whitespace() || c == ',' || c == '/');
    if let Some(p) = tokens().find_map(|t| t.strip_suffix('%')?.parse::<f64>().ok()) {
        return Some(p.round().clamp(0.0, 1000.0) as u32);
    }
    tokens()
        .find_map(|t| t.parse::<f64>().ok())
        .filter(|v| (0.0..=10.0).contains(v))
        .map(|v| (v * 100.0).round() as u32)
}

pub fn parse_muted(output: &str) -> bool {
    let o = output.to_ascii_lowercase();
    o.contains("[muted]") || o.contains("mute: yes")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    pub percent: u32,
    pub charging: bool,
}

/// The first battery under /sys/class/power_supply (laptops; desktops have none).
pub fn battery() -> Option<Battery> {
    battery_in(Path::new("/sys/class/power_supply"))
}

fn battery_in(root: &Path) -> Option<Battery> {
    let read =
        |p: &Path, f: &str| std::fs::read_to_string(p.join(f)).ok().map(|s| s.trim().to_string());
    let mut dirs: Vec<_> = std::fs::read_dir(root).ok()?.flatten().map(|e| e.path()).collect();
    dirs.sort();
    dirs.into_iter().find_map(|dir| {
        if read(&dir, "type").as_deref() != Some("Battery") {
            return None;
        }
        // Some batteries report scope "Device" (a wireless mouse): not the laptop's.
        if read(&dir, "scope").as_deref() == Some("Device") {
            return None;
        }
        let percent = read(&dir, "capacity")?.parse().ok()?;
        let status = read(&dir, "status").unwrap_or_default();
        Some(Battery { percent, charging: matches!(status.as_str(), "Charging" | "Full") })
    })
}

/// Wi-Fi signal (0..100) from /proc/net/wireless, if a wireless interface is up.
pub fn wifi() -> Option<u8> {
    parse_wireless(&std::fs::read_to_string("/proc/net/wireless").ok()?)
}

/// The file's table: two header lines, then "wlan0: 0000   54.  -56.  -256 ...", where
/// the third column is the link quality out of 70.
pub fn parse_wireless(text: &str) -> Option<u8> {
    text.lines().skip(2).find_map(|line| {
        let (_, rest) = line.split_once(':')?;
        let quality: f32 = rest.split_whitespace().nth(1)?.trim_end_matches('.').parse().ok()?;
        Some((quality / 70.0 * 100.0).round().clamp(0.0, 100.0) as u8)
    })
}

/// Local time as (year, month 1-12, day, weekday 0=Sunday, hour, minute, second).
pub fn local_now() -> (i32, u32, u32, u32, u32, u32, u32) {
    // SAFETY: time(NULL) and localtime_r with a zeroed out-struct are plain libc calls.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return (1970, 1, 1, 4, 0, 0, 0);
        }
        (
            tm.tm_year + 1900,
            tm.tm_mon as u32 + 1,
            tm.tm_mday as u32,
            tm.tm_wday as u32,
            tm.tm_hour as u32,
            tm.tm_min as u32,
            tm.tm_sec as u32,
        )
    }
}

/// "Sat 24 Oct".
pub fn short_date(weekday: u32, day: u32, month: u32) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let wd = DAYS.get(weekday as usize).unwrap_or(&"?");
    let mo = MONTHS.get(month.saturating_sub(1) as usize).unwrap_or(&"?");
    format!("{wd} {day} {mo}")
}

/// A short tag for an xkb layout name: "English (US)" -> "EN", "Russian" -> "RU".
pub fn layout_tag(name: &str) -> String {
    name.chars().filter(|c| c.is_alphabetic()).take(2).collect::<String>().to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_from_tools() {
        assert_eq!(parse_percent("Volume: 0.45"), Some(45));
        assert_eq!(parse_percent("Volume: 0.30 [MUTED]"), Some(30));
        assert_eq!(parse_percent("Volume: 1.00"), Some(100));
        assert_eq!(parse_percent("intel_backlight,backlight,12000,50%,24000"), Some(50));
        assert_eq!(parse_percent("Volume: front-left: 29491 /  45% / -20.81 dB"), Some(45));
        assert_eq!(parse_percent("nothing here"), None);
        assert!(parse_muted("Volume: 0.30 [MUTED]"));
        assert!(parse_muted("Mute: yes"));
        assert!(!parse_muted("Volume: 0.30"));
    }

    #[test]
    fn battery_from_sysfs() {
        let root = std::env::temp_dir().join(format!("spacewm-bat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mk = |name: &str, files: &[(&str, &str)]| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).expect("dir");
            for (f, v) in files {
                std::fs::write(d.join(f), format!("{v}\n")).expect("write");
            }
        };
        mk("AC0", &[("type", "Mains"), ("online", "1")]);
        mk("BAT0", &[("type", "Battery"), ("capacity", "87"), ("status", "Charging")]);
        mk("hidpp_battery_0", &[("type", "Battery"), ("scope", "Device"), ("capacity", "5")]);
        assert_eq!(battery_in(&root), Some(Battery { percent: 87, charging: true }));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn wireless_quality() {
        let text = "Inter-| sta-|   Quality        |   Discarded packets\n face | tus | link level noise |  nwid  crypt\n wlan0: 0000   54.  -56.  -256        0      0\n";
        assert_eq!(parse_wireless(text), Some(77));
        assert_eq!(parse_wireless("h1\nh2\n"), None);
    }

    #[test]
    fn date_and_layout() {
        assert_eq!(short_date(6, 24, 10), "Sat 24 Oct");
        assert_eq!(layout_tag("English (US)"), "EN");
        assert_eq!(layout_tag("Russian"), "RU");
        assert_eq!(layout_tag("Uzbek"), "UZ");
    }
}
