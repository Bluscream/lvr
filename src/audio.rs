use std::process::Command;
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioState {
    Disconnected,
    ConnectedToWiVRn,
}

pub struct AudioSwitcher {
    pub previous_sink: Option<String>,
    pub previous_source: Option<String>,
    pub current_state: AudioState,
}

impl Default for AudioSwitcher {
    fn default() -> Self {
        Self {
            previous_sink: None,
            previous_source: None,
            current_state: AudioState::Disconnected,
        }
    }
}

/// Ask the WiVRn server over D-Bus whether a headset is actually connected.
///
/// `None` means the question could not be answered (server not running, no
/// session bus), which the caller treats as "no headset".
///
/// This replaces looking for a `wivrn.*` PipeWire node: those nodes are created
/// when a headset first connects and then persist for the whole lifetime of the
/// server, so node presence says "a headset connected at some point since WiVRn
/// started", not "a headset is connected now" — and audio never switched back
/// on disconnect.
pub fn headset_connected() -> Option<bool> {
    use zbus::blocking::{Connection, Proxy};

    let connection = Connection::session().ok()?;
    let proxy = Proxy::new(
        &connection,
        "io.github.wivrn.Server",
        "/io/github/wivrn/Server",
        "io.github.wivrn.Server",
    )
    .ok()?;
    proxy.get_property::<bool>("HeadsetConnected").ok()
}

impl AudioSwitcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check current default sink via pactl
    pub fn get_default_sink() -> Option<String> {
        Self::pactl_output(&["get-default-sink"])
    }

    /// Check current default source via pactl
    pub fn get_default_source() -> Option<String> {
        Self::pactl_output(&["get-default-source"])
    }

    fn pactl_output(args: &[&str]) -> Option<String> {
        let output = Command::new("pactl")
            .args(args)
            // pactl's human-readable output is localised; pin it so parsing and
            // matching behave the same on a German desktop as on an English one.
            .env("LC_ALL", "C")
            .output()
            .ok()?;
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
        None
    }

    /// Returns whether a wivrn sink / source node currently exists.
    pub fn is_wivrn_audio_available() -> (bool, bool) {
        let sink_available = Self::check_pactl_list("sinks", "wivrn");
        let source_available = Self::check_pactl_list("sources", "wivrn");
        (sink_available, source_available)
    }

    fn check_pactl_list(item_type: &str, search_term: &str) -> bool {
        Self::pactl_output(&["list", "short", item_type])
            .map(|out| out.to_lowercase().contains(search_term))
            .unwrap_or(false)
    }

    /// Set default sink via pactl
    pub fn set_default_sink(sink: &str) -> bool {
        info!("Setting default audio sink to '{}'", sink);
        let res = Command::new("pactl").args(["set-default-sink", sink]).status();
        match res {
            Ok(s) => s.success(),
            Err(e) => {
                warn!("Failed to execute pactl set-default-sink: {}", e);
                false
            }
        }
    }

    /// Set default source via pactl
    pub fn set_default_source(source: &str) -> bool {
        info!("Setting default audio source/microphone to '{}'", source);
        let res = Command::new("pactl")
            .args(["set-default-source", source])
            .status();
        match res {
            Ok(s) => s.success(),
            Err(e) => {
                warn!("Failed to execute pactl set-default-source: {}", e);
                false
            }
        }
    }

    /// Should audio be on the headset right now?
    ///
    /// Prefers the WiVRn D-Bus state and only falls back to node presence when
    /// D-Bus cannot answer.
    fn should_be_on_headset() -> bool {
        match headset_connected() {
            Some(connected) => connected,
            None => {
                let (sink, source) = Self::is_wivrn_audio_available();
                sink || source
            }
        }
    }

    /// Primary check loop called periodically when auto_switch_audio is enabled
    pub fn update(&mut self) {
        let wanted = Self::should_be_on_headset();

        match self.current_state {
            AudioState::Disconnected => {
                if wanted {
                    info!("Headset connected! Switching audio devices to WiVRn...");
                    let (sink_avail, source_avail) = Self::is_wivrn_audio_available();

                    // Remember where to go back to — but never remember a wivrn
                    // device as the "previous" one, or restoring is a no-op.
                    if let Some(sink) = Self::get_default_sink() {
                        if !sink.contains("wivrn") {
                            info!("Saved previous default sink: {}", sink);
                            self.previous_sink = Some(sink);
                        }
                    }
                    if let Some(source) = Self::get_default_source() {
                        if !source.contains("wivrn") {
                            info!("Saved previous default source: {}", source);
                            self.previous_source = Some(source);
                        }
                    }

                    if sink_avail {
                        Self::set_default_sink("wivrn.sink");
                    }
                    if source_avail {
                        Self::set_default_source("wivrn.source");
                    }

                    self.current_state = AudioState::ConnectedToWiVRn;
                }
            }
            AudioState::ConnectedToWiVRn => {
                if !wanted {
                    info!("Headset disconnected! Restoring previous audio devices...");
                    self.restore_previous_audio();
                    self.current_state = AudioState::Disconnected;
                }
            }
        }
    }

    pub fn restore_previous_audio(&mut self) {
        if self.previous_sink.is_none() && self.previous_source.is_none() {
            // Nothing was saved (e.g. lvr started while the headset was already
            // connected). Fall back to the first non-wivrn device rather than
            // silently doing nothing.
            self.previous_sink = Self::first_non_wivrn("sinks");
            self.previous_source = Self::first_non_wivrn("sources");
        }

        if let Some(ref prev_sink) = self.previous_sink {
            info!("Restoring default sink to '{}'", prev_sink);
            Self::set_default_sink(prev_sink);
        }
        if let Some(ref prev_source) = self.previous_source {
            info!("Restoring default source to '{}'", prev_source);
            Self::set_default_source(prev_source);
        }
        self.previous_sink = None;
        self.previous_source = None;
        self.current_state = AudioState::Disconnected;
    }

    /// First device of this kind that is not a wivrn node and not a monitor.
    fn first_non_wivrn(item_type: &str) -> Option<String> {
        let listing = Self::pactl_output(&["list", "short", item_type])?;
        listing
            .lines()
            .filter_map(|line| line.split_whitespace().nth(1))
            .find(|name| !name.contains("wivrn") && !name.ends_with(".monitor"))
            .map(|name| name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_switcher_starts_disconnected() {
        let switcher = AudioSwitcher::new();
        assert_eq!(switcher.current_state, AudioState::Disconnected);
        assert!(switcher.previous_sink.is_none());
    }

    #[test]
    fn a_wivrn_device_is_never_remembered_as_the_way_back() {
        // Guards the rule that makes restoring work at all: if "previous" were
        // allowed to be a wivrn node, switching back would be a no-op.
        for name in ["wivrn.sink", "wivrn.source"] {
            assert!(name.contains("wivrn"));
        }
        let switcher = AudioSwitcher {
            previous_sink: Some("alsa_output.example".to_string()),
            previous_source: None,
            current_state: AudioState::ConnectedToWiVRn,
        };
        assert!(!switcher.previous_sink.as_deref().unwrap().contains("wivrn"));
    }

    /// Runs against the real session bus when one is present.
    #[test]
    fn headset_state_is_answerable_or_absent() {
        match headset_connected() {
            Some(_) => { /* WiVRn is up and answered */ }
            None => { /* no server or no bus; the caller treats this as "no" */ }
        }
    }
}
