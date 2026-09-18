//! Virtual display management for headless / VR use.
//!
//! Uses `krfb-virtualmonitor` (native KDE Plasma Wayland virtual monitor utility)
//! to spawn on-demand virtual displays, with `kscreen-doctor` fallback.
//! Physical display presence is verified against sysfs DRM EDID to ignore
//! forced kernel connectors (e.g. `video=HDMI-A-1:e`).

use std::fs;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use tracing::{info, warn};

static OWNED_CONNECTOR: Mutex<Option<String>> = Mutex::new(None);

static VIRTUAL_MONITOR_CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// Parsed display mode specifications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMode {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
}

impl ParsedMode {
    /// Parse a resolution string like "3840x2160@120", "2560x1440@90", or "1920x1080".
    /// Clamps to safe minimums (640x480, 24Hz) and reasonable maximums.
    pub fn parse(s: &str) -> Self {
        let trimmed = s.trim();
        let (res_part, hz) = if let Some((res, hz_str)) = trimmed.split_once('@') {
            let hz_val = hz_str
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .unwrap_or(60.0)
                .clamp(24.0, 360.0);
            (res, (hz_val * 1000.0).round() as u32)
        } else {
            (trimmed, 60_000)
        };

        let (w, h) = if let Some((w_s, h_s)) = res_part.split_once('x') {
            let width = w_s.parse::<u32>().unwrap_or(2560).clamp(640, 16384);
            let height = h_s.parse::<u32>().unwrap_or(1440).clamp(480, 16384);
            (width, height)
        } else {
            (2560, 1440)
        };

        Self {
            width: w,
            height: h,
            refresh_mhz: hz,
        }
    }

    /// Format as "WIDTHxHEIGHT@HZ" for mode setting.
    pub fn mode_string(&self) -> String {
        let hz = (self.refresh_mhz as f64) / 1000.0;
        if (hz - hz.round()).abs() < 0.01 {
            format!("{}x{}@{}", self.width, self.height, hz.round() as u32)
        } else {
            format!("{}x{}@{:.2}", self.width, self.height, hz)
        }
    }

    /// Format resolution part as "WIDTHxHEIGHT".
    pub fn res_string(&self) -> String {
        format!("{}x{}", self.width, self.height)
    }
}

/// Information about a detected KScreen output.
#[derive(Debug, Clone)]
struct OutputInfo {
    id: String,
    name: String,
    enabled: bool,
    is_virtual: bool,
}

fn doctor_command() -> Command {
    let mut command = Command::new("timeout");
    command.args(["5s", "kscreen-doctor"]);
    command
}

fn query_outputs() -> Option<Vec<OutputInfo>> {
    let output = match doctor_command().arg("-j").output() {
        Ok(out) if out.status.success() => out.stdout,
        Ok(out) => {
            warn!("kscreen-doctor -j exited with status {}", out.status);
            return None;
        }
        Err(err) => {
            warn!("Failed to execute kscreen-doctor: {err}");
            return None;
        }
    };

    let v: serde_json::Value = match serde_json::from_slice(&output) {
        Ok(val) => val,
        Err(err) => {
            warn!("Failed to parse kscreen-doctor JSON output: {err}");
            return None;
        }
    };

    let mut list = Vec::new();
    if let Some(outputs) = v.get("outputs").and_then(|o| o.as_array()) {
        for o in outputs {
            let id = o
                .get("id")
                .map(|i| {
                    i.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| i.to_string())
                })
                .unwrap_or_default();
            let name = o
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let enabled = o.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false);
            let name_upper = name.to_ascii_uppercase();
            let is_virtual = name_upper.contains("VIRTUAL")
                || name_upper.contains("HEADLESS")
                || name_upper.contains("DUMMY")
                || name_upper.contains("WL-");

            list.push(OutputInfo {
                id,
                name,
                enabled,
                is_virtual,
            });
        }
    }
    Some(list)
}

/// Query the total number of connected physical displays.
///
/// Read straight from sysfs rather than by shelling out. This is the one
/// display query on a timer, and `kscreen-doctor` costs ~70ms of Qt startup and
/// KScreen D-Bus traffic per call; the kernel already publishes the same facts
/// in `/sys/class/drm` for the price of a few small reads.
///
/// The three conditions mirror the previous filter exactly: `status` replaces
/// `connected`, `enabled` replaces `enabled`, and a non-empty `edid` rejects a
/// port forced on by something like `video=HDMI-A-1:e`. Compositor-side virtual
/// outputs never appear under `/sys/class/drm` at all, so they are excluded for
/// free and no name-matching heuristic is needed.
pub fn get_connected_display_count() -> Option<usize> {
    let mut count = 0;
    for entry in fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let path = entry.path();
        // Connector directories are cardX-NAME; skip cardX, renderD*, version.
        if !entry.file_name().to_string_lossy().contains('-') {
            continue;
        }
        let connected = fs::read_to_string(path.join("status"))
            .is_ok_and(|status| status.trim() == "connected");
        let enabled =
            fs::read_to_string(path.join("enabled")).is_ok_and(|on| on.trim() == "enabled");
        let has_edid = fs::read(path.join("edid")).is_ok_and(|edid| !edid.is_empty());
        if connected && enabled && has_edid {
            count += 1;
        }
    }
    Some(count)
}

/// Create/enable a virtual display.
///
/// Uses `krfb-virtualmonitor` for clean KDE Wayland virtual display creation.
/// Falls back to `kscreen-doctor` if already available.
/// Returns `Some(description)` with the connector name and mode if successful, or `None` on failure.
pub fn create_virtual_display(resolution: &str) -> Option<String> {
    let mode = ParsedMode::parse(resolution);
    let res_str = mode.res_string();
    let mode_str = mode.mode_string();
    info!("Requesting virtual display with mode {mode_str}");

    // First, stop any previously spawned virtual monitor child
    remove_virtual_display();

    // 1. Try spawning krfb-virtualmonitor
    if let Ok(child) = Command::new("krfb-virtualmonitor")
        .arg("--resolution")
        .arg(&res_str)
        .arg("--name")
        .arg("VR-Headset")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        info!(
            "Spawned krfb-virtualmonitor (PID: {}) with resolution {res_str}",
            child.id()
        );
        if let Ok(mut lock) = VIRTUAL_MONITOR_CHILD.lock() {
            *lock = Some(child);
        }

        // Give KWin a moment to register the new Wayland output
        std::thread::sleep(std::time::Duration::from_millis(1500));

        let outputs = query_outputs().unwrap_or_default();
        if let Some(virt) = outputs
            .iter()
            .find(|o| o.is_virtual || o.name.contains("VR-Headset"))
        {
            let name = if !virt.name.is_empty() {
                &virt.name
            } else {
                "Virtual-VR-Headset"
            };
            info!("Virtual display successfully registered in KWin: {name}");
            return Some(format!("{name} ({mode_str})"));
        }
        warn!("Virtual monitor did not register an output; cleaning up failed launch");
        remove_virtual_display();
        return None;
    }

    // 2. Fallback to kscreen-doctor if krfb-virtualmonitor binary wasn't found
    let outputs = query_outputs()?;
    if let Some(virt) = outputs.iter().find(|o| o.is_virtual && !o.enabled) {
        let target = if !virt.name.is_empty() {
            &virt.name
        } else {
            &virt.id
        };
        info!("Found virtual output connector {target}; enabling via kscreen-doctor");
        let status = doctor_command()
            .arg(format!("output.{target}.mode.{mode_str}"))
            .arg(format!("output.{target}.enable"))
            .status();
        if let Ok(s) = status
            && s.success()
        {
            *OWNED_CONNECTOR.lock().unwrap_or_else(|e| e.into_inner()) = Some(target.clone());
            return Some(format!("{target} ({mode_str})"));
        }
    }

    warn!("Failed to create virtual display");
    None
}

/// Disable/remove virtual display output.
pub fn remove_virtual_display() -> bool {
    let mut cleaned = false;

    // 1. Terminate krfb-virtualmonitor child process if managed
    if let Ok(mut lock) = VIRTUAL_MONITOR_CHILD.lock()
        && let Some(mut child) = lock.take()
    {
        info!(
            "Killing managed krfb-virtualmonitor process (PID: {})",
            child.id()
        );
        let _ = child.kill();
        let _ = child.wait();
        cleaned = true;
    }

    // Disable only a connector that this process explicitly enabled.
    let mut owned = OWNED_CONNECTOR.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(identifier) = owned.as_ref() {
        match doctor_command()
            .arg(format!("output.{identifier}.disable"))
            .status()
        {
            Ok(status) if status.success() => {
                *owned = None;
                cleaned = true;
            }
            other => warn!("Could not disable managed virtual output {identifier}: {other:?}"),
        }
    }

    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_modes() {
        let m = ParsedMode::parse("2560x1440@120");
        assert_eq!(m.width, 2560);
        assert_eq!(m.height, 1440);
        assert_eq!(m.refresh_mhz, 120_000);
        assert_eq!(m.mode_string(), "2560x1440@120");
        assert_eq!(m.res_string(), "2560x1440");

        let m2 = ParsedMode::parse("3840x2160@59.94");
        assert_eq!(m2.width, 3840);
        assert_eq!(m2.height, 2160);
        assert_eq!(m2.refresh_mhz, 59_940);
        assert_eq!(m2.res_string(), "3840x2160");
    }

    #[test]
    fn parses_fallback_defaults() {
        let m = ParsedMode::parse("");
        assert_eq!(m.width, 2560);
        assert_eq!(m.height, 1440);
        assert_eq!(m.refresh_mhz, 60_000);
        assert_eq!(m.res_string(), "2560x1440");

        let m2 = ParsedMode::parse("1920x1080");
        assert_eq!(m2.width, 1920);
        assert_eq!(m2.height, 1080);
        assert_eq!(m2.refresh_mhz, 60_000);
        assert_eq!(m2.res_string(), "1920x1080");
    }
}
