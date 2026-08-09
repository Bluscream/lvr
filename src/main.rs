// The binary drives the library crate rather than re-declaring every module,
// which used to compile the whole app twice and let the two copies drift.
use ksni::TrayMethods;
use lvr::config::Config;
use lvr::gui::LinuxVrGui;
use lvr::ipc;
use lvr::service::{AppState, Command, ServiceWorker};
use lvr::tray;
use tracing::info;
use tracing_subscriber::EnvFilter;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("LinuxVR (lvr) - VR Companion & Runtime Watchdog Manager");
        println!("\nUsage: lvr [OPTIONS]");
        println!("\nOptions:");
        println!("  -h, --help       Print help information");
        println!("  -v, --version    Print version information");
        println!("      --hidden     Start in the tray without showing the window");
        println!("\nFeatures:");
        println!("  • WiVRn Watchdog Service (Auto-restart on crash)");
        println!("  • PipeWire/PulseAudio Auto-Switcher (VR headset audio routing)");
        println!("  • Autostart & Grace Period Engine (VRChat & WiVRn companion apps)");
        println!("  • One-Click Nuke VR & WiVRn Restart");
        println!("  • VR-Optimized GUI & System Tray Icon");
        return Ok(());
    }

    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("lvr version {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let start_hidden = args.iter().any(|a| a == "--hidden");

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    info!("Initializing LinuxVR (lvr)...");

    // A second supervisor would fight the first one over WiVRn and over every
    // managed app, so a second launch just raises the first one's window.
    let listener = match ipc::acquire() {
        ipc::Acquired::Listener(listener) => Some(listener),
        ipc::Acquired::AlreadyRunning => {
            println!("LinuxVR is already running — raising its window.");
            return Ok(());
        }
        ipc::Acquired::Unavailable(reason) => {
            tracing::warn!("single-instance guard unavailable: {}", reason);
            None
        }
    };

    let config = Config::load();
    let (state, commands) = AppState::new(config);

    if let Some(listener) = listener {
        ipc::serve(listener, state.clone());
    }

    // The supervisor is blocking work (scanning /proc, running pactl, waiting
    // for processes to exit), so it gets a plain OS thread rather than a tokio
    // task.
    let worker_state = state.clone();
    let worker = std::thread::Builder::new()
        .name("lvr-supervisor".to_string())
        .spawn(move || ServiceWorker::new(worker_state).run(commands))
        .expect("failed to spawn the supervisor thread");

    // Tokio exists only for the tray's D-Bus connection.
    let tray_state = state.clone();
    std::thread::Builder::new()
        .name("lvr-tray".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("could not start the tokio runtime: {}", e);
                    return;
                }
            };
            rt.block_on(async move {
                // A tray app that autostarts gets SIGTERM at logout; without
                // this it dies mid-tick, skipping the config save and leaving
                // its runtime socket behind.
                let signal_state = tray_state.clone();
                tokio::spawn(async move {
                    use tokio::signal::unix::{signal, SignalKind};
                    let mut term = match signal(SignalKind::terminate()) {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    let mut interrupt = match signal(SignalKind::interrupt()) {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    tokio::select! {
                        _ = term.recv() => {}
                        _ = interrupt.recv() => {}
                    }
                    signal_state.add_log("Received shutdown signal, stopping.");
                    signal_state.set_quitting();
                    signal_state.send(Command::Quit);
                });

                let tray = tray::LinuxVrTray::new(tray_state.clone());
                match tray.spawn().await {
                    Ok(handle) => {
                        // Keep the tray in sync with the supervisor's status.
                        let mut previous = tray::TraySnapshot::of(&tray_state);
                        loop {
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            if tray_state.is_quitting() {
                                handle.shutdown().await;
                                return;
                            }
                            let current = tray::TraySnapshot::of(&tray_state);
                            if current != previous {
                                previous = current.clone();
                                if handle
                                    .update(move |t: &mut tray::LinuxVrTray| t.snapshot = current)
                                    .await
                                    .is_none()
                                {
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("System tray icon could not be registered: {}", e);
                    }
                }
            });
        })
        .expect("failed to spawn the tray thread");

    // Launch GUI app
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("LinuxVR - Companion & Runtime Manager")
            .with_app_id("lvr")
            .with_inner_size([880.0, 620.0])
            .with_min_inner_size([640.0, 480.0])
            .with_visible(!start_hidden),
        ..Default::default()
    };

    let app_state = state.clone();
    let result = eframe::run_native(
        "LinuxVR",
        options,
        Box::new(move |cc| Ok(Box::new(LinuxVrGui::new(cc, app_state)))),
    );

    state.set_quitting();
    state.send(Command::Quit);
    let _ = worker.join();
    ipc::cleanup();

    result
}
