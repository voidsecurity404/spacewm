//! tracing setup: logs go to stderr AND to a file.
//!
//! Why the file matters: in DRM mode (Stage 6) spacewm owns the screen, so there is
//! no visible terminal. After a crash the log file is the only way to see what
//! happened — read it from another TTY or over SSH.

use std::{
    fs::{self, File},
    io::IsTerminal,
    path::PathBuf,
    sync::Mutex,
};

use anyhow::Context;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// `$XDG_STATE_HOME/spacewm`, falling back to `~/.local/state/spacewm`.
pub fn state_dir() -> anyhow::Result<PathBuf> {
    let dirs = directories::BaseDirs::new().context("cannot determine home directory")?;
    let base = match dirs.state_dir() {
        Some(dir) => dir.to_path_buf(),
        None => dirs.home_dir().join(".local/state"),
    };
    Ok(base.join("spacewm"))
}

pub fn init(level: &str) -> anyhow::Result<PathBuf> {
    // RUST_LOG wins if present; otherwise our level for spacewm and only warnings
    // from smithay (it is very chatty at info/debug). Two smithay modules log an ERROR
    // every time an Xwayland server ends, including when we stop an idle one on purpose;
    // spacewm logs real Xwayland crashes itself ("Xwayland connection lost").
    let filter = match EnvFilter::try_from_default_env() {
        Ok(filter) => filter,
        Err(_) => EnvFilter::try_new(format!(
            "spacewm={level},smithay=warn,smithay::utils::x11rb=off,smithay::xwayland::xserver=off"
        ))
        .with_context(|| format!("invalid log level '{level}'"))?,
    };

    let dir = state_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let log_path = dir.join("spacewm.log");
    if log_path.exists() {
        // Keep exactly one previous run: the interesting crash is usually the last one.
        fs::rename(&log_path, dir.join("spacewm.log.old"))
            .context("cannot rotate previous log file")?;
    }
    let file =
        File::create(&log_path).with_context(|| format!("cannot create {}", log_path.display()))?;

    // SPACEWM_SPAN_TIMES=1: log how long each frame phase took (span close events), for
    // profiling without tracy. Use with RUST_LOG=spacewm=trace.
    let span_events = if std::env::var_os("SPACEWM_SPAN_TIMES").is_some() {
        fmt::format::FmtSpan::CLOSE
    } else {
        fmt::format::FmtSpan::NONE
    };
    let registry = tracing_subscriber::registry();
    // `--features profile`: every span also goes to the tracy profiler.
    #[cfg(feature = "profile")]
    let registry = registry.with(tracing_tracy::TracyLayer::default());
    registry
        .with(filter)
        // Colors only on a real terminal; piped/redirected logs stay plain text.
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(std::io::stderr().is_terminal())
                .with_target(true)
                .with_span_events(span_events.clone()),
        )
        // A Mutex<File> is a valid writer: each event locks, writes a line, unlocks.
        .with(
            fmt::layer()
                .with_writer(Mutex::new(file))
                .with_ansi(false)
                .with_target(true)
                .with_span_events(span_events),
        )
        .try_init()
        .context("tracing subscriber already installed")?;

    install_panic_hook();
    Ok(log_path)
}

/// DRM device fds opened by the DRM backend. On a panic we drop DRM master on them
/// FIRST, so the VT / another session can take the display back even if unwinding goes
/// wrong.
pub static DRM_FDS: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());

/// DRM_IOCTL_DROP_MASTER = _IO('d', 0x1f).
const DRM_IOCTL_DROP_MASTER: libc::c_ulong = 0x641f;

/// Log panics through tracing so they land in the log file, then run the default
/// hook (which prints to stderr and handles RUST_BACKTRACE).
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_string());
        // Always with the call stack: a crash in a user's session must say where it came
        // from, without them having to know about RUST_BACKTRACE.
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(%location, "panic: {message}\n{backtrace}");
        if let Ok(fds) = DRM_FDS.try_lock() {
            for &fd in fds.iter() {
                // SAFETY: a plain ioctl on an fd we opened; failure is harmless here.
                unsafe { libc::ioctl(fd, DRM_IOCTL_DROP_MASTER) };
            }
        }
        default_hook(info);
    }));
}
