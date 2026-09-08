//! Steam process management (status check, graceful shutdown, restart).

use std::time::Duration;
use anyhow::{Result, bail};

use crate::procs;

/// Checks if any Steam process is currently running on the system.
pub fn steam_running() -> bool {
    let mut scanner = procs::ProcessScanner::new();
    let snapshot = scanner.scan();
    snapshot.any_matching(
        &["steam.sh".into(), "/steam ".into(), "steamwebhelper".into()],
        &[],
    )
}

/// Request a clean shutdown of Steam and wait until the processes terminate.
pub async fn shutdown_steam() -> Result<()> {
    if !steam_running() {
        return Ok(());
    }
    let _ = procs::run_command_line("steam -shutdown").await;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if !steam_running() {
            tokio::time::sleep(Duration::from_millis(1000)).await;
            return Ok(());
        }
    }
    bail!("Steam is still running after shutdown command");
}

/// Starts the Steam client using available launcher commands.
pub async fn start_steam() -> Result<()> {
    for cmd in ["bazzite-steam", "steam"] {
        if procs::which(cmd).is_some() {
            let _ = procs::spawn_command_line(cmd);
            return Ok(());
        }
    }
    procs::spawn_command_line("steam").map(|_| ())
}
