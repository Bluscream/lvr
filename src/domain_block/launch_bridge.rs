//! VRChat `launch.exe` Linux IPC bridge patch management.
//!
//! Replaces VRChat's `launch.exe` with a custom IPC bridge (`vrc-launch-bridge.exe`)
//! allowing VRCOSC and other Linux tools to pass launch parameters/arguments to VRChat.
//! Keeps a read-only backup (`launch.org.exe`) of the original executable.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Embedded byte payload for `vrc-launch-bridge.exe`.
pub const LAUNCH_BRIDGE_BYTES: &[u8] = include_bytes!("../../assets/vrc-launch-bridge.exe");

/// Status of the VRChat launch bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchBridgeStatus {
    /// VRChat game directory could not be located.
    NotDetected,
    /// `launch.exe` is present and matches the Linux IPC bridge binary.
    Patched,
    /// `launch.exe` is present but does not match the Linux IPC bridge binary (unpatched).
    Unpatched,
    /// `launch.exe` does not exist in the game directory.
    MissingLaunchExe,
}

/// Locate the VRChat game directory.
///
/// Can derive from an explicit prefix hint (e.g., `.../steamapps/compatdata/438100` -> `.../steamapps/common/VRChat`),
/// or by scanning Steam library roots for `steamapps/common/VRChat`.
pub fn detect_vrc_game_dir(prefix_hint: Option<&Path>) -> Option<PathBuf> {
    if let Some(prefix) = prefix_hint {
        // e.g. /path/to/steamapps/compatdata/438100 -> /path/to/steamapps/common/VRChat
        if let Some(compatdata) = prefix.parent()
            && let Some(steamapps) = compatdata.parent()
        {
            let candidate = steamapps.join("common/VRChat");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }

    for root in crate::steam::paths::library_roots() {
        let candidate = root.join("steamapps/common/VRChat");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    None
}

/// Check the current status of the VRChat launch bridge.
pub fn check_launch_bridge_status(game_dir: &Path) -> LaunchBridgeStatus {
    if !game_dir.is_dir() {
        return LaunchBridgeStatus::NotDetected;
    }
    let target = game_dir.join("launch.exe");
    if !target.is_file() {
        return LaunchBridgeStatus::MissingLaunchExe;
    }

    match fs::read(&target) {
        Ok(bytes) if bytes == LAUNCH_BRIDGE_BYTES => LaunchBridgeStatus::Patched,
        Ok(_) => LaunchBridgeStatus::Unpatched,
        Err(_) => LaunchBridgeStatus::Unpatched,
    }
}

/// Patch VRChat's `launch.exe` with the Linux IPC bridge executable.
///
/// Returns `Ok(true)` if the patch was applied, or `Ok(false)` if already up to date.
pub fn patch_launch_bridge(game_dir: &Path) -> Result<bool> {
    if !game_dir.is_dir() {
        bail!(
            "VRChat game directory does not exist: {}",
            game_dir.display()
        );
    }

    let target = game_dir.join("launch.exe");
    let backup = game_dir.join("launch.org.exe");

    // Check if already patched
    if target.is_file()
        && let Ok(current_bytes) = fs::read(&target)
        && current_bytes == LAUNCH_BRIDGE_BYTES
    {
        // Ensure correct permissions (0o555)
        let _ = fs::set_permissions(&target, fs::Permissions::from_mode(0o555));
        return Ok(false);
    }

    // If backup does not exist yet and target exists, create read-only backup (0o444)
    if !backup.exists() && target.is_file() {
        fs::copy(&target, &backup)
            .with_context(|| format!("Backing up {} to {}", target.display(), backup.display()))?;
        let _ = fs::set_permissions(&backup, fs::Permissions::from_mode(0o444));
    }

    // If target is read-only, remove or adjust permissions first
    if target.exists() {
        let _ = fs::set_permissions(&target, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_file(&target);
    }

    fs::write(&target, LAUNCH_BRIDGE_BYTES)
        .with_context(|| format!("Writing bridge to {}", target.display()))?;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o555))
        .with_context(|| format!("Setting 0o555 permissions on {}", target.display()))?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_bridge_status_and_patching() {
        let temp = tempfile::tempdir().unwrap();
        let game_dir = temp.path().join("VRChat");
        assert_eq!(
            check_launch_bridge_status(&game_dir),
            LaunchBridgeStatus::NotDetected
        );

        fs::create_dir_all(&game_dir).unwrap();
        assert_eq!(
            check_launch_bridge_status(&game_dir),
            LaunchBridgeStatus::MissingLaunchExe
        );

        let launch = game_dir.join("launch.exe");
        let backup = game_dir.join("launch.org.exe");
        fs::write(&launch, b"original-fake-launch-exe").unwrap();
        assert_eq!(
            check_launch_bridge_status(&game_dir),
            LaunchBridgeStatus::Unpatched
        );

        // First patch: should backup original and install bridge
        let patched = patch_launch_bridge(&game_dir).unwrap();
        assert!(patched);
        assert_eq!(
            check_launch_bridge_status(&game_dir),
            LaunchBridgeStatus::Patched
        );
        assert!(backup.is_file());
        assert_eq!(fs::read(&backup).unwrap(), b"original-fake-launch-exe");
        assert_eq!(fs::read(&launch).unwrap(), LAUNCH_BRIDGE_BYTES);

        // Second patch: should be idempotent and return false
        let re_patched = patch_launch_bridge(&game_dir).unwrap();
        assert!(!re_patched);
        assert_eq!(fs::read(&backup).unwrap(), b"original-fake-launch-exe");
    }
}
