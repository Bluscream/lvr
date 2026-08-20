//! Virtual 4K display management for headless / VR use.
//!
//! Uses `kscreen-doctor` to query connected displays and manage virtual
//! screen creation/enabling on Wayland / KWin.

use std::process::Command;
use tracing::{info, warn, error};

/// Query the total number of connected displays using `kscreen-doctor -j`.
pub fn get_connected_display_count() -> usize {
    let output = match Command::new("kscreen-doctor").arg("-j").output() {
        Ok(out) if out.status.success() => out.stdout,
        Ok(out) => {
            warn!("kscreen-doctor -j exited with status {}", out.status);
            return 0;
        }
        Err(err) => {
            warn!("Failed to execute kscreen-doctor: {err}");
            return 0;
        }
    };

    let text = match std::str::from_utf8(&output) {
        Ok(t) => t,
        Err(_) => return 0,
    };

    let mut count = 0;
    // Simple robust count of connected outputs from kscreen-doctor JSON output
    for block in text.split('{') {
        if block.contains("\"connected\": true") && block.contains("\"enabled\": true") {
            count += 1;
        }
    }
    count
}

/// Create/enable a virtual 4K display using kscreen-doctor.
pub fn create_virtual_4k_display(resolution: &str) -> bool {
    let mode_str = if resolution.trim().is_empty() {
        "3840x2160@120"
    } else {
        resolution.trim()
    };

    info!("Creating virtual 4K display: {mode_str}");

    // First attempt: kscreen-doctor addCustomMode or output mode setting
    // Try adding mode or enabling virtual screen output if present
    let status = Command::new("kscreen-doctor")
        .arg(format!("output.VIRTUAL-1.mode.{}", mode_str))
        .arg("output.VIRTUAL-1.enable")
        .status();

    match status {
        Ok(s) if s.success() => {
            info!("Successfully enabled virtual display VIRTUAL-1 with mode {mode_str}");
            true
        }
        _ => {
            // Fallback attempt: addCustomMode on first connected or active output if needed
            let fallback = Command::new("kscreen-doctor")
                .arg(format!("output.1.addCustomMode.3840.2160.60000.full"))
                .status();
            match fallback {
                Ok(s) if s.success() => {
                    info!("Successfully added 4K custom mode via kscreen-doctor");
                    true
                }
                Err(e) => {
                    error!("Failed to execute kscreen-doctor for virtual display: {e}");
                    false
                }
                Ok(s) => {
                    warn!("kscreen-doctor returned status {s} when attempting virtual display creation");
                    false
                }
            }
        }
    }
}

/// Disable/remove virtual display output using kscreen-doctor.
pub fn remove_virtual_display() -> bool {
    info!("Disabling virtual display output");
    let status = Command::new("kscreen-doctor")
        .arg("output.VIRTUAL-1.disable")
        .status();

    match status {
        Ok(s) if s.success() => {
            info!("Successfully disabled virtual display VIRTUAL-1");
            true
        }
        _ => {
            warn!("Could not disable VIRTUAL-1 output via kscreen-doctor");
            false
        }
    }
}
