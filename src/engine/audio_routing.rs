use std::time::{Duration, Instant};

use crate::audio::{self, Kind};
use crate::config::Config;
use crate::wivrn::WivrnState;

use super::Engine;

pub(super) const AUDIO_POLL_INTERVAL: Duration = Duration::from_secs(2);
pub(super) const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(15);

/// Audio routing bookkeeping.
#[derive(Debug, Default)]
pub(super) struct AudioRouting {
    pub(super) on_vr: bool,
    pub(super) saved_sink: Option<String>,
    pub(super) saved_source: Option<String>,
    pub(super) initialized: bool,
}

impl Engine {
    pub(super) async fn apply_audio(&mut self, config: &Config, wivrn: &WivrnState) {
        if !config.audio.enabled {
            self.audio.initialized = true;
            return;
        }

        if !self.audio.initialized {
            self.audio.initialized = true;
            self.audio.on_vr = wivrn.headset_connected;
            if wivrn.headset_connected {
                self.route_audio_to_vr(false).await;
            }
            return;
        }

        if wivrn.headset_connected && !self.audio.on_vr {
            self.route_audio_to_vr(false).await;
        } else if !wivrn.headset_connected && self.audio.on_vr {
            self.route_audio_to_desktop(false).await;
        }
    }

    pub(super) async fn route_audio_to_vr(&mut self, manual: bool) {
        let config = self.shared.config_snapshot();
        let vr_sink = config.audio.vr_sink.trim().to_string();
        let vr_source = config.audio.vr_source.trim().to_string();

        if let Ok(current) = audio::get_default(Kind::Sink).await
            && current != vr_sink
        {
            self.audio.saved_sink = Some(current);
        }
        if let Ok(current) = audio::get_default(Kind::Source).await
            && current != vr_source
        {
            self.audio.saved_source = Some(current);
        }

        let outcome =
            audio::route(Some(&vr_sink), Some(&vr_source), config.audio.move_streams).await;
        self.report_route(
            outcome,
            true,
            manual,
            "Could not route audio to VR — check the Audio tab",
        );
    }

    pub(super) async fn route_audio_to_desktop(&mut self, manual: bool) {
        let config = self.shared.config_snapshot();
        let sink = audio::desktop_target(
            Kind::Sink,
            &config.audio.desktop_sink,
            self.audio.saved_sink.as_deref(),
            &config.audio.vr_sink,
        )
        .await;
        let source = audio::desktop_target(
            Kind::Source,
            &config.audio.desktop_source,
            self.audio.saved_source.as_deref(),
            &config.audio.vr_source,
        )
        .await;

        let outcome = audio::route(
            sink.as_deref(),
            source.as_deref(),
            config.audio.move_streams,
        )
        .await;
        self.report_route(
            outcome,
            false,
            manual,
            "No desktop audio device to switch back to — pick one in the Audio tab",
        );
    }

    fn report_route(
        &mut self,
        outcome: audio::RouteOutcome,
        to_vr: bool,
        manual: bool,
        nothing_happened: &str,
    ) {
        for error in &outcome.errors {
            self.shared.warn(error.clone());
        }
        if !outcome.is_empty() {
            self.audio.on_vr = to_vr;
            let how = match (manual, to_vr) {
                (true, _) => "Audio (manual)",
                (false, true) => "Headset connected — audio",
                (false, false) => "Headset disconnected — audio",
            };
            self.shared.info(format!("{how}: {}", outcome.summary()));
        } else {
            if !to_vr {
                self.audio.on_vr = false;
            }
            if manual {
                self.shared.warn(nothing_happened.to_string());
            }
        }
        self.last_audio_poll = None;
    }

    pub(super) async fn refresh_audio_cache(&mut self, config: &Config) {
        let now = Instant::now();
        let due_defaults = self
            .last_audio_poll
            .is_none_or(|last| now.duration_since(last) >= AUDIO_POLL_INTERVAL);
        if due_defaults {
            self.last_audio_poll = Some(now);
            if let Ok(sink) = audio::get_default(Kind::Sink).await {
                self.cached_default_sink = sink;
            }
            if let Ok(source) = audio::get_default(Kind::Source).await {
                self.cached_default_source = source;
            }
            if config.audio.enabled {
                let vr_sink = config.audio.vr_sink.trim();
                if !vr_sink.is_empty() {
                    self.audio.on_vr = self.cached_default_sink == vr_sink;
                }
            }
        }

        let due_devices = self
            .last_device_poll
            .is_none_or(|last| now.duration_since(last) >= DEVICE_POLL_INTERVAL);
        if due_devices {
            self.last_device_poll = Some(now);
            if let Ok(sinks) = audio::list_devices(Kind::Sink).await {
                self.cached_sinks = sinks;
            }
            if let Ok(sources) = audio::list_devices(Kind::Source).await {
                self.cached_sources = sources;
            }
        }
    }
}
