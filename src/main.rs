mod audio;
mod config;
mod gui;
mod process;
mod service;
mod tray;

use config::Config;
use gui::LinuxVrGui;
use ksni::TrayMethods;
use service::{AppState, ServiceWorker};
use std::sync::{Arc, Mutex};
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

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    info!("Initializing LinuxVR (lvr)...");

    let config = Config::load();
    let state = AppState::new(config);

    let nuke_trigger = Arc::new(Mutex::new(false));
    let show_gui_trigger = Arc::new(Mutex::new(false));

    // Spawn Background Tokio Service Worker & Tray Icon
    let state_clone = state.clone();
    let nuke_trigger_clone = nuke_trigger.clone();
    let show_gui_trigger_clone = show_gui_trigger.clone();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime");

        rt.block_on(async move {
            // Spawn Tray Item
            let tray_item = tray::LinuxVrTray::new(
                state_clone.clone(),
                nuke_trigger_clone.clone(),
                show_gui_trigger_clone.clone(),
            );
            tokio::spawn(async move {
                if let Err(e) = tray_item.spawn().await {
                    tracing::warn!("System tray icon could not be registered: {}", e);
                }
            });

            // Watchdog loop with nuke check
            let state_inner = state_clone.clone();
            let nuke_trg = nuke_trigger_clone.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    let should_nuke = {
                        let mut trg = nuke_trg.lock().unwrap();
                        if *trg {
                            *trg = false;
                            true
                        } else {
                            false
                        }
                    };
                    if should_nuke {
                        let mut w = ServiceWorker::new(state_inner.clone());
                        w.nuke_vr();
                    }
                }
            });

            let worker = ServiceWorker::new(state_clone);
            worker.start().await;
        });
    });

    // Launch GUI app
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("LinuxVR - Companion & Runtime Manager")
            .with_inner_size([880.0, 620.0])
            .with_min_inner_size([640.0, 480.0]),
        ..Default::default()
    };

    let app_state = state.clone();
    let app_nuke = nuke_trigger.clone();

    eframe::run_native(
        "LinuxVR",
        options,
        Box::new(move |_cc| Ok(Box::new(LinuxVrGui::new(app_state, app_nuke)))),
    )
}
