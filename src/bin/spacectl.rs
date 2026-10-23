//! spacectl: control a running spacewm over its IPC socket.

#[path = "../ipc/proto.rs"]
#[allow(dead_code)]
mod proto;

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use proto::{EventKind, InputAction, MouseButton, PressMode, Request, Response};

#[derive(Parser)]
#[command(name = "spacectl", version, about = "Control a running spacewm")]
struct Cli {
    /// Socket path (default: $SPACEWM_SOCKET, or the only spacewm socket in $XDG_RUNTIME_DIR).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Print raw JSON responses.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// spacewm version and backend.
    Version,
    /// List windows/panels.
    #[command(alias = "list-panels")]
    List,
    /// Focus a panel by id.
    Focus { id: u64 },
    /// Ask a panel's app to close.
    Close { id: u64 },
    /// Start a program inside spacewm: `spacectl spawn foot -e htop`. With --shell the words
    /// are one shell command line (pipes, &&), allowed only with general.ipc_allow_shell.
    Spawn {
        #[arg(long)]
        shell: bool,
        #[arg(trailing_var_arg = true, required = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Move a panel to X Y Z (metres), optionally turning it.
    Move {
        id: u64,
        #[arg(allow_hyphen_values = true)]
        x: f32,
        #[arg(allow_hyphen_values = true)]
        y: f32,
        #[arg(allow_hyphen_values = true)]
        z: f32,
        /// Rotation around Y in degrees (0 = facing +Z).
        #[arg(long, allow_hyphen_values = true)]
        yaw: Option<f32>,
    },
    /// Set a panel's scale in window pixels per metre (200..3000; bigger = smaller).
    Scale { id: u64, pixels_per_unit: f32 },
    /// Switch to walking, or into a window: `spacectl mode walk`, `spacectl mode window 3`.
    Mode {
        #[command(subcommand)]
        target: ModeCmd,
    },
    /// Re-read the config file now and report errors.
    ReloadConfig,
    /// Save a PNG of the composited screen (path inside $HOME).
    Screenshot {
        path: PathBuf,
        #[arg(long)]
        force: bool,
    },
    /// Quit spacewm.
    Quit,
    /// Move the camera to X Y Z (metres).
    Teleport {
        #[arg(allow_hyphen_values = true)]
        x: f32,
        #[arg(allow_hyphen_values = true)]
        y: f32,
        #[arg(allow_hyphen_values = true)]
        z: f32,
        /// Look direction around Y in degrees (0 = toward -Z, 90 = toward +X).
        #[arg(long, allow_hyphen_values = true)]
        yaw: Option<f32>,
        /// Up/down in degrees (-89..89).
        #[arg(long, allow_hyphen_values = true)]
        pitch: Option<f32>,
    },
    /// Set a block: X Y Z BLOCK (air, grass, dirt, stone, wood, leaves, glass, sand, water).
    SetBlock {
        #[arg(allow_hyphen_values = true)]
        x: i32,
        #[arg(allow_hyphen_values = true)]
        y: i32,
        #[arg(allow_hyphen_values = true)]
        z: i32,
        block: String,
    },
    /// Print the block at X Y Z.
    GetBlock {
        #[arg(allow_hyphen_values = true)]
        x: i32,
        #[arg(allow_hyphen_values = true)]
        y: i32,
        #[arg(allow_hyphen_values = true)]
        z: i32,
    },
    /// Print events as they happen.
    Watch,
    /// Synthetic input (needs spacewm --allow-input-injection).
    Input {
        #[command(subcommand)]
        action: InputCmd,
    },
}

#[derive(Subcommand)]
enum ModeCmd {
    Walk,
    Window { id: u64 },
}

#[derive(Subcommand)]
enum InputCmd {
    /// Press a key combo, e.g. "Alt+Return".
    Key {
        combo: String,
        #[arg(long, value_enum, default_value = "tap")]
        state: PressArg,
    },
    /// Type text.
    Type { text: String },
    /// Move the pointer to output-local coordinates.
    MoveTo { x: f64, y: f64 },
    /// Move the pointer relatively.
    MoveBy {
        #[arg(allow_hyphen_values = true)]
        dx: f64,
        #[arg(allow_hyphen_values = true)]
        dy: f64,
    },
    /// Click a mouse button.
    Click {
        #[arg(value_enum, default_value = "left")]
        button: ButtonArg,
        #[arg(long, value_enum, default_value = "tap")]
        state: PressArg,
    },
    /// Scroll by wheel notches (positive = down/right).
    Scroll {
        #[arg(allow_hyphen_values = true)]
        dy: f64,
        #[arg(allow_hyphen_values = true, default_value_t = 0.0)]
        dx: f64,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum PressArg {
    Press,
    Release,
    Tap,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ButtonArg {
    Left,
    Right,
    Middle,
}

fn find_socket(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    if let Some(p) = std::env::var_os("SPACEWM_SOCKET") {
        return Ok(PathBuf::from(p));
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?;
    let mut found: Vec<PathBuf> = std::fs::read_dir(&runtime)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("spacewm-") && n.ends_with(".sock"))
        })
        .collect();
    found.sort();
    match found.len() {
        0 => Err("no running spacewm found (no spacewm-*.sock in $XDG_RUNTIME_DIR)".into()),
        1 => Ok(found.remove(0)),
        _ => Err(format!(
            "several spacewm sockets found, pick one with --socket: {}",
            found.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        )),
    }
}

fn to_request(command: Command) -> Request {
    let press = |p: PressArg| match p {
        PressArg::Press => PressMode::Press,
        PressArg::Release => PressMode::Release,
        PressArg::Tap => PressMode::Tap,
    };
    match command {
        Command::Version => Request::Version,
        Command::List => Request::ListPanels,
        Command::Focus { id } => Request::FocusPanel { id },
        Command::Close { id } => Request::ClosePanel { id },
        Command::Spawn { shell, command } => Request::Spawn {
            // Without --shell, re-quote so the server's shell-words split restores the
            // same argv; with it, the words are one command line for sh -c.
            command: if shell { command.join(" ") } else { shell_words::join(command) },
            shell,
        },
        Command::Move { id, x, y, z, yaw } => {
            Request::MovePanel { id, position: [x, y, z], yaw_deg: yaw }
        }
        Command::Scale { id, pixels_per_unit } => Request::ScalePanel { id, pixels_per_unit },
        Command::Mode { target } => Request::SetMode {
            mode: match target {
                ModeCmd::Walk => proto::ModeTarget::Walk,
                ModeCmd::Window { id } => proto::ModeTarget::Window(id),
            },
        },
        Command::ReloadConfig => Request::ReloadConfig,
        Command::Screenshot { path, force } => {
            let path = std::path::absolute(&path).unwrap_or(path);
            Request::Screenshot { path: path.display().to_string(), force }
        }
        Command::Quit => Request::Quit,
        Command::SetBlock { x, y, z, block } => Request::SetBlock { pos: [x, y, z], block },
        Command::GetBlock { x, y, z } => Request::GetBlock { pos: [x, y, z] },
        Command::Teleport { x, y, z, yaw, pitch } => {
            Request::Teleport { position: [x, y, z], yaw_deg: yaw, pitch_deg: pitch }
        }
        Command::Watch => Request::Subscribe {
            events: vec![
                EventKind::PanelOpened,
                EventKind::PanelClosed,
                EventKind::FocusChanged,
                EventKind::ModeChanged,
                EventKind::ConfigReloaded,
            ],
        },
        Command::Input { action } => Request::Input {
            action: match action {
                InputCmd::Key { combo, state } => InputAction::Key { combo, state: press(state) },
                InputCmd::Type { text } => InputAction::Type { text },
                InputCmd::MoveTo { x, y } => InputAction::MoveTo { x, y },
                InputCmd::MoveBy { dx, dy } => InputAction::MoveBy { dx, dy },
                InputCmd::Click { button, state } => InputAction::Button {
                    button: match button {
                        ButtonArg::Left => MouseButton::Left,
                        ButtonArg::Right => MouseButton::Right,
                        ButtonArg::Middle => MouseButton::Middle,
                    },
                    state: press(state),
                },
                InputCmd::Scroll { dy, dx } => InputAction::Scroll { dx, dy },
            },
        },
    }
}

fn print_human(response: &Response) {
    match response {
        Response::Ok => {}
        Response::Error { message } => eprintln!("error: {message}"),
        Response::Version { version, backend } => println!("spacewm {version} ({backend})"),
        Response::Spawned { pid } => println!("started pid {pid}"),
        Response::Screenshot { path, width, height } => println!("{path} ({width}x{height})"),
        Response::Panels { panels } => {
            println!(
                "{:>4}  {:<7}  {:<3}  {:<22}  {:<11}  {:<20}  TITLE",
                "ID", "KIND", "FOC", "APP_ID", "SIZE", "POSITION"
            );
            for p in panels {
                println!(
                    "{:>4}  {:<7}  {:<3}  {:<22}  {:<11}  {:<20}  {}",
                    p.id,
                    p.kind,
                    if p.focused { "*" } else { "" },
                    p.app_id.as_deref().unwrap_or("-"),
                    format!("{}x{}", p.size_px[0], p.size_px[1]),
                    format!("{:.2} {:.2} {:.2}", p.position[0], p.position[1], p.position[2]),
                    p.title.as_deref().unwrap_or("")
                );
            }
        }
        Response::Event { event } => println!("{}", describe(event)),
        Response::Block { block } => println!("{block}"),
    }
}

fn describe(event: &proto::Event) -> String {
    use proto::Event;
    match event {
        Event::PanelOpened { id, app_id } => {
            format!("panel opened: {id} {}", app_id.as_deref().unwrap_or(""))
        }
        Event::PanelClosed { id } => format!("panel closed: {id}"),
        Event::FocusChanged { id: Some(id) } => format!("focus: {id}"),
        Event::FocusChanged { id: None } => "focus: none".into(),
        Event::ModeChanged { mode } => format!("mode: {mode}"),
        Event::ConfigReloaded => "config reloaded".into(),
    }
}

fn run() -> Result<bool, String> {
    let cli = Cli::parse();
    let path = find_socket(cli.socket)?;
    let watching = matches!(cli.command, Command::Watch);
    let request = to_request(cli.command);

    let mut stream = UnixStream::connect(&path)
        .map_err(|e| format!("cannot connect to {}: {e}", path.display()))?;
    let mut line = serde_json::to_string(&request).map_err(|e| e.to_string())?;
    line.push('\n');
    stream.write_all(line.as_bytes()).map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut ok = true;
    loop {
        let mut buf = String::new();
        let n = reader.read_line(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            if watching {
                return Ok(ok);
            }
            return Err("spacewm closed the connection".into());
        }
        let response: Response =
            serde_json::from_str(buf.trim()).map_err(|e| format!("bad response: {e}"))?;
        if matches!(response, Response::Error { .. }) {
            ok = false;
        }
        if cli.json {
            println!("{}", buf.trim());
        } else {
            print_human(&response);
        }
        // `watch`: the first response is the Ok for the subscription; keep reading events.
        if !watching || !ok {
            return Ok(ok);
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("spacectl: {e}");
            ExitCode::FAILURE
        }
    }
}
