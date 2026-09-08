//! Virtual display management for headless / VR use.
//!
//! Uses `krfb-virtualmonitor` (native KDE Plasma Wayland virtual monitor utility)
//! to spawn on-demand virtual displays, with `kscreen-doctor` fallback.
//! Physical display presence is verified against sysfs DRM EDID to ignore
//! forced kernel connectors (e.g. `video=HDMI-A-1:e`).

use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use tracing::{info, warn};

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
            let hz_val = hz_str.parse::<f64>().unwrap_or(60.0).clamp(24.0, 360.0);
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
    connected: bool,
    enabled: bool,
    is_virtual: bool,
}

fn query_outputs() -> Vec<OutputInfo> {
    let output = match Command::new("kscreen-doctor").arg("-j").output() {
        Ok(out) if out.status.success() => out.stdout,
        Ok(out) => {
            warn!("kscreen-doctor -j exited with status {}", out.status);
            return Vec::new();
        }
        Err(err) => {
            warn!("Failed to execute kscreen-doctor: {err}");
            return Vec::new();
        }
    };

    let v: serde_json::Value = match serde_json::from_slice(&output) {
        Ok(val) => val,
        Err(err) => {
            warn!("Failed to parse kscreen-doctor JSON output: {err}");
            return Vec::new();
        }
    };

    let mut list = Vec::new();
    if let Some(outputs) = v.get("outputs").and_then(|o| o.as_array()) {
        for o in outputs {
            let id = o.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
            let name = o.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
            let connected = o.get("connected").and_then(|c| c.as_bool()).unwrap_or(false);
            let enabled = o.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false);
            let name_upper = name.to_ascii_uppercase();
            let is_virtual = name_upper.contains("VIRTUAL")
                || name_upper.contains("HEADLESS")
                || name_upper.contains("DUMMY")
                || name_upper.contains("WL-");

            list.push(OutputInfo {
                id,
                name,
                connected,
                enabled,
                is_virtual,
            });
        }
    }
    list
}

/// Check if a display connector actually has an active physical monitor with valid EDID.
/// This prevents kernel cmdline overrides like `video=HDMI-A-1:e` from tricking the system
/// into thinking a real monitor is attached when it's just a disconnected port forced enabled.
fn connector_has_physical_edid(name: &str) -> bool {
    let drm_dir = Path::new("/sys/class/drm");
    if let Ok(entries) = fs::read_dir(drm_dir) {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let s = file_name.to_string_lossy();
            // Match cardX-NAME (e.g. card1-DP-3 or card1-HDMI-A-1)
            if s.ends_with(name) {
                let status_path = entry.path().join("status");
                let edid_path = entry.path().join("edid");
                if let Ok(status) = fs::read_to_string(&status_path)
                    && status.trim() == "connected"
                        && let Ok(edid) = fs::read(&edid_path) {
                            return !edid.is_empty();
                        }
            }
        }
    }
    // Fallback: If sysfs DRM can't be read, trust kscreen-doctor's connected status
    true
}

/// Query the total number of connected physical displays.
pub fn get_connected_display_count() -> usize {
    query_outputs()
        .into_iter()
        .filter(|o| o.connected && o.enabled && !o.is_virtual && connector_has_physical_edid(&o.name))
        .count()
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
        info!("Spawned krfb-virtualmonitor (PID: {}) with resolution {res_str}", child.id());
        if let Ok(mut lock) = VIRTUAL_MONITOR_CHILD.lock() {
            *lock = Some(child);
        }

        // Give KWin a moment to register the new Wayland output
        std::thread::sleep(std::time::Duration::from_millis(1500));

        let outputs = query_outputs();
        if let Some(virt) = outputs.iter().find(|o| o.is_virtual || o.name.contains("VR-Headset")) {
            let name = if !virt.name.is_empty() { &virt.name } else { "Virtual-VR-Headset" };
            info!("Virtual display successfully registered in KWin: {name}");
            return Some(format!("{name} ({mode_str})"));
        }
        return Some(format!("Virtual-VR-Headset ({mode_str})"));
    }

    // 2. Fallback to kscreen-doctor if krfb-virtualmonitor binary wasn't found
    let outputs = query_outputs();
    if let Some(virt) = outputs.iter().find(|o| o.is_virtual) {
        let target = if !virt.name.is_empty() { &virt.name } else { &virt.id };
        info!("Found virtual output connector {target}; enabling via kscreen-doctor");
        let status = Command::new("kscreen-doctor")
            .arg(format!("output.{target}.mode.{mode_str}"))
            .arg(format!("output.{target}.enable"))
            .status();
        if let Ok(s) = status && s.success() {
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
        && let Some(mut child) = lock.take() {
            info!("Killing managed krfb-virtualmonitor process (PID: {})", child.id());
            let _ = child.kill();
            let _ = child.wait();
            cleaned = true;
        }

    // Also pkill any lingering krfb-virtualmonitor processes named VR-Headset
    let _ = Command::new("pkill")
        .arg("-f")
        .arg("krfb-virtualmonitor.*VR-Headset")
        .status();

    // 2. Disable via kscreen-doctor if any virtual connector remains enabled
    let outputs = query_outputs();
    for virt in outputs.iter().filter(|o| o.is_virtual && o.enabled) {
        let identifier = if !virt.name.is_empty() { &virt.name } else { &virt.id };
        let status = Command::new("kscreen-doctor")
            .arg(format!("output.{identifier}.disable"))
            .status();
        if let Ok(s) = status && s.success() {
            info!("Disabled virtual output {identifier} via kscreen-doctor");
            cleaned = true;
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
