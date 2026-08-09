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

impl AudioSwitcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check current default sink via pactl
    pub fn get_default_sink() -> Option<String> {
        let output = Command::new("pactl")
            .arg("get-default-sink")
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

    /// Check current default source via pactl
    pub fn get_default_source() -> Option<String> {
        let output = Command::new("pactl")
            .arg("get-default-source")
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

    /// Returns true if wivrn sink or source exists in PipeWire / PulseAudio
    pub fn is_wivrn_audio_available() -> (bool, bool) {
        let sink_available = Self::check_pactl_list("sinks", "wivrn");
        let source_available = Self::check_pactl_list("sources", "wivrn");
        (sink_available, source_available)
    }

    fn check_pactl_list(item_type: &str, search_term: &str) -> bool {
        let output = Command::new("pactl")
            .args(["list", item_type, "short"])
            .output();
        if let Ok(out) = output {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                return stdout.to_lowercase().contains(search_term);
            }
        }
        false
    }

    /// Set default sink via pactl
    pub fn set_default_sink(sink: &str) -> bool {
        info!("Setting default audio sink to '{}'", sink);
        let res = Command::new("pactl")
            .args(["set-default-sink", sink])
            .status();
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

    /// Primary check loop called periodically when auto_switch_audio is enabled
    pub fn update(&mut self) {
        let (sink_avail, source_avail) = Self::is_wivrn_audio_available();
        let wivrn_available = sink_avail || source_avail;

        match self.current_state {
            AudioState::Disconnected => {
                if wivrn_available {
                    info!("WiVRn Audio detected! Switching audio devices to WiVRn...");
                    let cur_sink = Self::get_default_sink();
                    let cur_source = Self::get_default_source();

                    if let Some(ref sink) = cur_sink {
                        if !sink.contains("wivrn") {
                            self.previous_sink = cur_sink.clone();
                            info!("Saved previous default sink: {:?}", self.previous_sink);
                        }
                    }

                    if let Some(ref source) = cur_source {
                        if !source.contains("wivrn") {
                            self.previous_source = cur_source.clone();
                            info!("Saved previous default source: {:?}", self.previous_source);
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
                if !wivrn_available {
                    info!("WiVRn Audio disconnected! Restoring previous audio devices...");
                    self.restore_previous_audio();
                    self.current_state = AudioState::Disconnected;
                }
            }
        }
    }

    pub fn restore_previous_audio(&mut self) {
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
    }
}
