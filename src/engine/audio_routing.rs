use std::time::{Duration, Instant};

use crate::audio::{self, Kind};
use crate::config::Config;
use crate::wivrn::WivrnState;

use super::Engine;

/// Minimum spacing between automatic routing retries after a partial failure.
/// This is a retry backoff, not a poll, and must stay short to be useful.
pub(super) const AUDIO_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Fallback refresh for the default sink/source. Each one costs a `pactl` fork,
/// and `pactl subscribe` reports every change as a `server` event, so this only
/// has to cover a missed event or a dropped subscription.
pub(super) const DEFAULTS_POLL_INTERVAL: Duration = Duration::from_secs(60);
/// Fallback refresh for the device list; events normally trigger it first.
pub(super) const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Audio routing bookkeeping.
#[derive(Debug, Default)]
pub(super) struct AudioRouting {
    pub(super) on_vr: bool,
    pub(super) saved_sink: Option<String>,
    pub(super) saved_source: Option<String>,
    last_headset: Option<bool>,
    pending: Option<bool>,
    last_attempt: Option<Instant>,
}

impl AudioRouting {
    fn auto_target(&mut self, enabled: bool, connected: bool, now: Instant) -> Option<bool> {
        if !enabled {
            self.last_headset = None;
            self.pending = None;
            return None;
        }
        if self.last_headset != Some(connected) {
            if self.last_headset.is_some() || connected {
                self.pending = Some(connected);
            }
            self.last_headset = Some(connected);
            self.last_attempt = None;
        }
        let target = self.pending?;
        if self
            .last_attempt
            .is_some_and(|last| now.duration_since(last) < AUDIO_POLL_INTERVAL)
        {
            return None;
        }
        self.last_attempt = Some(now);
        Some(target)
    }
}

impl Engine {
    pub(super) async fn apply_audio(&mut self, config: &Config, wivrn: &WivrnState) {
        match self.audio.auto_target(
            config.audio.enabled,
            wivrn.headset_connected,
            Instant::now(),
        ) {
            Some(true) => self.route_audio_to_vr(false).await,
            Some(false) => self.route_audio_to_desktop(false).await,
            None => {}
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
        self.audio.pending = if manual || outcome.errors.is_empty() {
            None
        } else {
            Some(to_vr)
        };
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
        // `pactl subscribe` tells us when anything actually changed; the
        // intervals are now only a safety net for a missed or dropped event.
        let due_defaults = self.audio_events.take_defaults_changed()
            || self
                .last_audio_poll
                .is_none_or(|last| now.duration_since(last) >= DEFAULTS_POLL_INTERVAL);
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

        let due_devices = self.audio_events.take_devices_changed()
            || self
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_routing_is_preserved_until_headset_transition() {
        let now = Instant::now();
        let mut routing = AudioRouting::default();
        assert_eq!(routing.auto_target(true, true, now), Some(true));
        routing.pending = None;
        routing.on_vr = false;
        assert_eq!(
            routing.auto_target(true, true, now + Duration::from_secs(3)),
            None
        );
        assert_eq!(
            routing.auto_target(true, false, now + Duration::from_secs(4)),
            Some(false)
        );
    }
    #[test]
    fn retries_failed_partial_routes_without_poll_spam() {
        let now = Instant::now();
        let mut routing = AudioRouting::default();
        assert_eq!(routing.auto_target(true, true, now), Some(true));
        assert_eq!(
            routing.auto_target(true, true, now + Duration::from_millis(200)),
            None
        );
        assert_eq!(
            routing.auto_target(true, true, now + Duration::from_secs(2)),
            Some(true)
        );
        assert_eq!(
            routing.auto_target(false, true, now + Duration::from_secs(3)),
            None
        );
        assert_eq!(
            routing.auto_target(true, true, now + Duration::from_secs(4)),
            Some(true)
        );
    }
}
