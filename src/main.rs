//! lvr / LinuxVR — a tray app and GUI that keeps a Linux VR session together:
//! it supervises the WiVRn server, follows headset connect/disconnect with the
//! audio routing, and starts/stops companion apps around VRChat.

mod audio;
mod config;
mod display;
mod engine;
mod icon;
mod ipc;
mod procs;
mod state;
mod tray;
mod ui;
mod domain_block;
mod wivrn;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::state::{Command, Shared};

const HELP: &str = "\
lvr — LinuxVR companion (WiVRn supervisor, audio switcher, autostart manager)

USAGE:
    lvr [OPTIONS]

OPTIONS:
    -c, --config <PATH>   Use a specific config file
                          (default: $XDG_CONFIG_HOME/lvr/config.toml, or $LVR_CONFIG)
        --hidden          Start in the tray without showing the window
        --show            Show the window on start (overrides general.start_hidden)
        --no-tray         Do not create a tray icon
        --check           Load the config, print a summary and exit
        --status          Print the current VR status (read-only) and exit
        --tab <NAME>      Open on a specific tab: dashboard, autostart, audio,
                          settings or logs
        --audio <WHERE>   Route audio to `vr` or `desktop` and exit
                          (bindable to a keyboard shortcut)
    -h, --help            Print this help
    -V, --version         Print the version

ENVIRONMENT:
    LVR_CONFIG            Config file path
    LVR_LOG / RUST_LOG    Log filter, e.g. `lvr=debug`
";

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    config: Option<PathBuf>,
    hidden: Option<bool>,
    no_tray: bool,
    check: bool,
    status: bool,
    tab: Option<String>,
    audio: Option<String>,
    help: bool,
    version: bool,
}

impl Args {
    fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self> {
        let mut parsed = Args::default();
        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "-h" | "--help" => parsed.help = true,
                "-V" | "--version" => parsed.version = true,
                "--hidden" => parsed.hidden = Some(true),
                "--show" => parsed.hidden = Some(false),
                "--no-tray" => parsed.no_tray = true,
                "--check" => parsed.check = true,
                "--status" => parsed.status = true,
                "--tab" => {
                    parsed.tab = Some(iter.next().context("--tab needs a name")?);
                }
                other if other.starts_with("--tab=") => {
                    parsed.tab = Some(other["--tab=".len()..].to_string());
                }
                "--audio" => {
                    parsed.audio = Some(iter.next().context("--audio needs vr or desktop")?);
                }
                other if other.starts_with("--audio=") => {
                    parsed.audio = Some(other["--audio=".len()..].to_string());
                }
                "-c" | "--config" => {
                    let value = iter.next().context("--config needs a path")?;
                    parsed.config = Some(PathBuf::from(value));
                }
                other if other.starts_with("--config=") => {
                    parsed.config = Some(PathBuf::from(&other["--config=".len()..]));
                }
                other => anyhow::bail!("unknown argument `{other}` (try --help)"),
            }
        }
        Ok(parsed)
    }
}

fn main() -> Result<()> {
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("lvr: {err:#}");
            std::process::exit(2);
        }
    };

    if args.help {
        print!("{HELP}");
        return Ok(());
    }
    if args.version {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    init_logging();

    let config_path = args.config.clone().unwrap_or_else(Config::default_path);
    let mut config = Config::load_or_create(&config_path)
        .with_context(|| format!("loading {}", config_path.display()))?;
    config.normalize();

    if handle_cli_command(&args, &config, &config_path)? {
        return Ok(());
    }

    let tab = match args.tab.as_deref() {
        Some(name) => ui::Tab::from_name(name)
            .with_context(|| format!("unknown tab `{name}` (try --help)"))?,
        None => ui::Tab::Dashboard,
    };

    // A second supervisor would fight the first one over every process, so a
    // second launch just raises the window of the one already running.
    let listener = match ipc::acquire(&ipc::show_request(args.tab.as_deref())) {
        ipc::Acquired::Listener(listener) => Some(listener),
        ipc::Acquired::AlreadyRunning => {
            println!("lvr is already running — raising its window.");
            return Ok(());
        }
        ipc::Acquired::Unavailable(err) => {
            tracing::warn!("single-instance socket unavailable: {err:#}");
            None
        }
    };

    ensure_display_environment();

    let start_hidden = args.hidden.unwrap_or(config.general.start_hidden);
    let (shared, rx) = Shared::new(config, config_path.clone());
    shared.info(format!("Config: {}", config_path.display()));

    if let Some(listener) = listener {
        ipc::serve(listener, shared.clone());
    }

    let worker = spawn_worker(shared.clone(), rx, !args.no_tray)?;
    let native_options = gui_native_options(start_hidden);

    let gui_shared = shared.clone();
    let result = eframe::run_native(
        "lvr",
        native_options,
        Box::new(move |cc| Ok(Box::new(ui::LvrApp::new(cc, gui_shared, tab)))),
    );

    shared.set_quitting();
    shared.send(Command::Quit);
    if let Err(err) = shared.save_config() {
        tracing::error!("saving config failed: {err:#}");
    }

    // Give the supervisor a moment to unwind; do not hang the exit on it.
    let _ = worker.join_timeout(Duration::from_secs(3));

    if let Err(err) = result {
        anyhow::bail!("GUI failed: {err}");
    }
    ipc::cleanup();
    tracing::info!("bye");
    // The tray's D-Bus task can still be parked; exit decisively.
    std::process::exit(0);
}

fn handle_cli_command(args: &Args, config: &Config, config_path: &Path) -> Result<bool> {
    if args.check {
        print_check(config, config_path);
        return Ok(true);
    }

    if args.status {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("starting the tokio runtime")?;
        let status = runtime.block_on(engine::probe(config));
        print_status(&status, config.general.show_debug_info);
        return Ok(true);
    }

    if let Some(where_to) = args.audio.as_deref() {
        let to_vr = match where_to.trim().to_ascii_lowercase().as_str() {
            "vr" | "headset" => true,
            "desktop" | "pc" => false,
            other => anyhow::bail!("--audio expects `vr` or `desktop`, got `{other}`"),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("starting the tokio runtime")?;
        let outcome = runtime.block_on(engine::route_audio(config, to_vr));
        for error in &outcome.errors {
            eprintln!("lvr: {error}");
        }
        if outcome.is_empty() {
            anyhow::bail!("nothing to switch — check the audio devices in your config");
        }
        println!("{}", outcome.summary());
        return Ok(true);
    }

    Ok(false)
}

fn gui_native_options(start_hidden: bool) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LinuxVR")
            .with_app_id("lvr")
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([400.0, 320.0])
            .with_icon(icon::window_icon())
            .with_visible(!start_hidden),
        persist_window: false,
        ..Default::default()
    }
}

/// Handle for the background runtime thread.
struct Worker {
    handle: std::thread::JoinHandle<()>,
    done: std::sync::mpsc::Receiver<()>,
}

impl Worker {
    fn join_timeout(self, timeout: Duration) -> bool {
        let finished = self.done.recv_timeout(timeout).is_ok();
        if finished {
            let _ = self.handle.join();
        }
        finished
    }
}

fn spawn_worker(
    shared: Shared,
    rx: tokio::sync::mpsc::UnboundedReceiver<Command>,
    with_tray: bool,
) -> Result<Worker> {
    let (done_tx, done) = std::sync::mpsc::channel();
    let handle = std::thread::Builder::new()
        .name("lvr-supervisor".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    tracing::error!("could not start the tokio runtime: {err}");
                    let _ = done_tx.send(());
                    return;
                }
            };
            runtime.block_on(async {
                let tray_task = with_tray.then(|| tokio::spawn(tray::run(shared.clone())));
                engine::Engine::new(shared, rx).run().await;
                if let Some(task) = tray_task {
                    task.abort();
                }
            });
            // Do not wait for lingering D-Bus tasks on shutdown.
            runtime.shutdown_timeout(Duration::from_millis(500));
            let _ = done_tx.send(());
        })
        .context("spawning the supervisor thread")?;
    Ok(Worker { handle, done })
}

/// Ensure WAYLAND_DISPLAY and/or DISPLAY are set and available before initializing UI.
/// If started early by systemd/autostart before the compositor socket is published or exported,
/// this probes the runtime directories and waits briefly for the display socket to become ready.
fn ensure_display_environment() {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        let uid = unsafe { libc::getuid() };
        let default_path = format!("/run/user/{uid}");
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", &default_path);
        }
        default_path
    });

    let runtime_path = std::path::PathBuf::from(&runtime_dir);

    // Wait up to 15 seconds (75 iterations * 200ms) for display socket to appear
    for _ in 0..75 {
        // If WAYLAND_DISPLAY is already set, check if the socket exists
        if let Ok(wayland_display) = std::env::var("WAYLAND_DISPLAY")
            && runtime_path.join(&wayland_display).exists() {
                return;
            }

        // If DISPLAY is already set, check if the X11 socket exists
        if let Ok(display_var) = std::env::var("DISPLAY") {
            let num = display_var.trim_start_matches(':').split('.').next().unwrap_or("0");
            if std::path::Path::new(&format!("/tmp/.X11-unix/X{num}")).exists() {
                return;
            }
        }

        // Check for wayland sockets in XDG_RUNTIME_DIR
        for candidate in ["wayland-0", "wayland-1", "wayland-2"] {
            let socket_path = runtime_path.join(candidate);
            if socket_path.exists() {
                unsafe {
                    std::env::set_var("WAYLAND_DISPLAY", candidate);
                }
                tracing::info!("Discovered and set WAYLAND_DISPLAY={candidate}");
                return;
            }
        }

        // Check for X11 sockets in /tmp/.X11-unix/
        for display_num in [0, 1, 2] {
            let x11_socket = format!("/tmp/.X11-unix/X{display_num}");
            if std::path::Path::new(&x11_socket).exists() {
                let disp = format!(":{display_num}");
                unsafe {
                    std::env::set_var("DISPLAY", &disp);
                }
                tracing::info!("Discovered and set DISPLAY={disp}");
                return;
            }
        }

        std::thread::sleep(Duration::from_millis(200));
    }

    // Fallback: set defaults anyway if nothing found after timeout
    if std::env::var("WAYLAND_DISPLAY").is_err() && std::env::var("DISPLAY").is_err() {
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", "wayland-0");
            std::env::set_var("DISPLAY", ":0");
        }
        tracing::warn!("Display socket not detected; defaulted WAYLAND_DISPLAY=wayland-0, DISPLAY=:0");
    }
}

fn init_logging() {
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = std::env::var("LVR_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .unwrap_or_else(|_| "info".to_string());
    let filter = EnvFilter::try_new(&filter).unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

fn print_check(config: &Config, path: &std::path::Path) {
    println!("config: {}", path.display());
    println!("  ui scale         : {}", config.general.ui_scale);
    println!(
        "  poll interval    : {} ms",
        config.general.poll_interval_ms
    );
    println!("  wivrn watchdog   : {}", config.wivrn.watchdog);
    println!("  wivrn command    : {}", config.wivrn.start_command);
    println!("  audio switching  : {}", config.audio.enabled);
    println!(
        "  audio VR devices : {} / {}",
        config.audio.vr_sink, config.audio.vr_source
    );
    println!(
        "  terminal         : {}",
        if config.general.terminal.trim().is_empty() {
            procs::detect_terminal().unwrap_or("none found").to_string()
        } else {
            config.general.terminal.clone()
        }
    );
    println!("  autostart entries: {}", config.autostart.len());
    for entry in &config.autostart {
        println!(
            "    [{}] {:<16} trigger={:<20} stop={:<14} cmd={}",
            if entry.enabled { "x" } else { " " },
            entry.name_or_id(),
            entry.trigger.to_string(),
            ui::widgets::format_grace(entry.grace_secs),
            entry.command
        );
    }
}

fn print_status(status: &state::Status, show_debug: bool) {
    let yes_no = |value: bool| if value { "yes" } else { "no" };
    println!("wivrn running   : {}", yes_no(status.wivrn_running));
    println!("headset         : {}", yes_no(status.headset_connected));
    if !status.headset_name.is_empty() {
        println!("headset name    : {}", status.headset_name);
    }
    println!("xr session      : {}", yes_no(status.session_running));
    println!("vrchat running  : {}", yes_no(status.vrchat_running));
    println!("default output  : {}", status.friendly_sink_label());
    println!("default input   : {}", status.friendly_source_label());
    println!("audio on vr     : {}", yes_no(status.audio_on_vr));
    println!(
        "audio devices   : {} outputs, {} inputs",
        status.sinks.len(),
        status.sources.len()
    );
    println!("managed apps    :");
    for entry in &status.entries {
        if show_debug {
            println!(
                "  {:<16} running={:<4} trigger={:<4} pids={:?}",
                entry.name,
                yes_no(entry.running),
                yes_no(entry.trigger_active),
                entry.pids
            );
        } else {
            println!(
                "  {:<16} running={:<4} trigger={:<4}",
                entry.name,
                yes_no(entry.running),
                yes_no(entry.trigger_active)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args> {
        Args::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn no_arguments_is_the_default() {
        assert_eq!(parse(&[]).unwrap(), Args::default());
    }

    #[test]
    fn flags_are_recognised() {
        let args = parse(&["--hidden", "--no-tray", "--check", "--status"]).unwrap();
        assert_eq!(args.hidden, Some(true));
        assert!(args.no_tray);
        assert!(args.check);
        assert!(args.status);

        assert_eq!(parse(&["--show"]).unwrap().hidden, Some(false));
        assert!(parse(&["-h"]).unwrap().help);
        assert!(parse(&["--version"]).unwrap().version);
    }

    #[test]
    fn config_accepts_both_spellings() {
        assert_eq!(
            parse(&["--config", "/tmp/a.toml"]).unwrap().config,
            Some(PathBuf::from("/tmp/a.toml"))
        );
        assert_eq!(
            parse(&["--config=/tmp/b.toml"]).unwrap().config,
            Some(PathBuf::from("/tmp/b.toml"))
        );
        assert_eq!(
            parse(&["-c", "/tmp/c.toml"]).unwrap().config,
            Some(PathBuf::from("/tmp/c.toml"))
        );
    }

    #[test]
    fn tab_accepts_both_spellings() {
        assert_eq!(
            parse(&["--tab", "logs"]).unwrap().tab.as_deref(),
            Some("logs")
        );
        assert_eq!(
            parse(&["--tab=audio"]).unwrap().tab.as_deref(),
            Some("audio")
        );
    }

    #[test]
    fn audio_accepts_both_spellings() {
        assert_eq!(
            parse(&["--audio", "vr"]).unwrap().audio.as_deref(),
            Some("vr")
        );
        assert_eq!(
            parse(&["--audio=desktop"]).unwrap().audio.as_deref(),
            Some("desktop")
        );
    }

    #[test]
    fn bad_arguments_are_rejected() {
        assert!(parse(&["--nope"]).is_err());
        assert!(parse(&["--config"]).is_err());
        assert!(parse(&["--tab"]).is_err());
        assert!(parse(&["--audio"]).is_err());
    }

    #[test]
    fn later_flags_win() {
        assert_eq!(parse(&["--hidden", "--show"]).unwrap().hidden, Some(false));
    }

    #[test]
    fn help_text_documents_every_flag() {
        for flag in [
            "--config",
            "--hidden",
            "--show",
            "--no-tray",
            "--check",
            "--status",
            "--tab",
            "--audio",
            "--help",
            "--version",
        ] {
            assert!(HELP.contains(flag), "{flag} missing from --help");
        }
    }

    #[test]
    fn source_files_under_1000_lines() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let src_dir = manifest_dir.join("src");

        fn check_dir(dir: &std::path::Path) {
            for entry in std::fs::read_dir(dir).expect("read src directory") {
                let entry = entry.expect("valid directory entry");
                let path = entry.path();
                if path.is_dir() {
                    check_dir(&path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let content = std::fs::read_to_string(&path)
                        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
                    let line_count = content.lines().count();
                    assert!(
                        line_count <= 1000,
                        "Source file {} has {} lines, exceeding 1000 lines max!",
                        path.display(),
                        line_count
                    );
                }
            }
        }

        check_dir(&src_dir);
    }
}

