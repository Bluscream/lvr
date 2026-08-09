//! Default sink/source switching via `pactl` (PipeWire's PulseAudio shim).
//!
//! Everything shells out rather than linking libpipewire: it keeps the binary
//! free of C build dependencies and `pactl` is present on every PipeWire and
//! PulseAudio desktop.

use std::process::Stdio;

use anyhow::{Context, Result, bail};

use crate::procs::which;
use crate::state::AudioDevice;

/// Which node kind a lookup refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Sink,
    Source,
}

impl Kind {
    fn list_arg(self) -> &'static str {
        match self {
            Kind::Sink => "sinks",
            Kind::Source => "sources",
        }
    }

    fn get_default(self) -> &'static str {
        match self {
            Kind::Sink => "get-default-sink",
            Kind::Source => "get-default-source",
        }
    }

    fn set_default(self) -> &'static str {
        match self {
            Kind::Sink => "set-default-sink",
            Kind::Source => "set-default-source",
        }
    }

    fn streams_arg(self) -> &'static str {
        match self {
            Kind::Sink => "sink-inputs",
            Kind::Source => "source-outputs",
        }
    }

    fn move_stream(self) -> &'static str {
        match self {
            Kind::Sink => "move-sink-input",
            Kind::Source => "move-source-output",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Sink => "output",
            Kind::Source => "input",
        }
    }
}

/// Run `pactl` with a C locale so the human-readable output stays parseable.
async fn pactl(args: &[&str]) -> Result<String> {
    let program = which("pactl").context("`pactl` not found; is pipewire-pulse installed?")?;
    let output = tokio::process::Command::new(&program)
        .args(args)
        .env("LC_ALL", "C")
        .env("LANGUAGE", "C")
        .stdin(Stdio::null())
        .output()
        .await
        .with_context(|| format!("running `pactl {}`", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("`pactl {}` failed: {stderr}", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse `pactl list sinks|sources` into name/description pairs.
///
/// Only the top-level `Name:` / `Description:` of each block are taken; nested
/// `Properties:` lines are indented further and are ignored.
pub fn parse_devices(listing: &str) -> Vec<AudioDevice> {
    let mut devices = Vec::new();
    let mut current: Option<AudioDevice> = None;

    for line in listing.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("Sink #") || trimmed.starts_with("Source #") {
            if let Some(device) = current.take()
                && !device.name.is_empty()
            {
                devices.push(device);
            }
            current = Some(AudioDevice::default());
            continue;
        }
        let Some(device) = current.as_mut() else {
            continue;
        };
        if device.name.is_empty() {
            if let Some(rest) = trimmed.strip_prefix("Name: ") {
                device.name = rest.trim().to_string();
            }
        }
        if device.description.is_empty() {
            if let Some(rest) = trimmed.strip_prefix("Description: ") {
                device.description = rest.trim().to_string();
            }
        }
    }
    if let Some(device) = current
        && !device.name.is_empty()
    {
        devices.push(device);
    }
    devices
}

/// Stream ids from `pactl list short sink-inputs|source-outputs`.
pub fn parse_stream_ids(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|id| id.chars().all(|c| c.is_ascii_digit()))
        .map(|id| id.to_string())
        .collect()
}

pub async fn list_devices(kind: Kind) -> Result<Vec<AudioDevice>> {
    let listing = pactl(&["list", kind.list_arg()]).await?;
    let mut devices = parse_devices(&listing);
    if kind == Kind::Source {
        // Monitor sources are echoes of an output; nobody wants them as a mic.
        devices.retain(|d| !d.name.ends_with(".monitor"));
    }
    Ok(devices)
}

pub async fn get_default(kind: Kind) -> Result<String> {
    Ok(pactl(&[kind.get_default()]).await?.trim().to_string())
}

pub async fn set_default(kind: Kind, name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("no {} device name given", kind.label());
    }
    pactl(&[kind.set_default(), name]).await.map(|_| ())
}

/// Does a node with this exact name currently exist?
pub async fn device_exists(kind: Kind, name: &str) -> bool {
    match list_devices(kind).await {
        Ok(devices) => devices.iter().any(|d| d.name == name),
        Err(_) => false,
    }
}

/// Move every live stream of this kind onto `name`.
/// Returns how many streams moved; individual failures are not fatal because a
/// stream can disappear between listing and moving.
pub async fn move_streams(kind: Kind, name: &str) -> Result<usize> {
    let listing = pactl(&["list", "short", kind.streams_arg()]).await?;
    let mut moved = 0;
    for id in parse_stream_ids(&listing) {
        if pactl(&[kind.move_stream(), &id, name]).await.is_ok() {
            moved += 1;
        }
    }
    Ok(moved)
}

/// Set the default device and optionally drag existing streams along.
pub async fn switch_to(kind: Kind, name: &str, also_move_streams: bool) -> Result<()> {
    set_default(kind, name).await?;
    if also_move_streams {
        move_streams(kind, name).await?;
    }
    Ok(())
}

/// What a routing change actually did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RouteOutcome {
    /// Human-readable descriptions of the devices that changed.
    pub changes: Vec<String>,
    pub errors: Vec<String>,
}

impl RouteOutcome {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub fn summary(&self) -> String {
        self.changes.join(", ")
    }
}

/// Point the default output and/or input at the given devices.
/// `None` (or an empty name) leaves that side alone.
pub async fn route(
    sink: Option<&str>,
    source: Option<&str>,
    also_move_streams: bool,
) -> RouteOutcome {
    let mut outcome = RouteOutcome::default();
    for (kind, target) in [(Kind::Sink, sink), (Kind::Source, source)] {
        let Some(name) = target.map(str::trim).filter(|name| !name.is_empty()) else {
            continue;
        };
        match switch_to(kind, name, also_move_streams).await {
            Ok(()) => outcome.changes.push(format!("{} → {name}", kind.label())),
            Err(err) => outcome
                .errors
                .push(format!("{} switch failed: {err:#}", kind.label())),
        }
    }
    outcome
}

/// Which device "back to desktop" means: the configured one, else the one that
/// was active before the headset took over, else the first device that is not
/// the VR one.
pub async fn desktop_target(
    kind: Kind,
    configured: &str,
    saved: Option<&str>,
    vr: &str,
) -> Option<String> {
    let configured = configured.trim();
    if !configured.is_empty() {
        return Some(configured.to_string());
    }
    let vr = vr.trim();
    if let Some(saved) = saved.map(str::trim)
        && !saved.is_empty()
        && saved != vr
        && device_exists(kind, saved).await
    {
        return Some(saved.to_string());
    }
    list_devices(kind)
        .await
        .ok()?
        .into_iter()
        .map(|device| device.name)
        .find(|name| name != vr)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINKS: &str = "Sink #63\n\
        \tState: RUNNING\n\
        \tName: alsa_output.usb-WG2.analog-stereo\n\
        \tDescription: WG2 Analog Stereo\n\
        \tProperties:\n\
        \t\tName: not-a-device\n\
        \t\tDescription: also not a device\n\
        Sink #341\n\
        \tState: RUNNING\n\
        \tName: wivrn.sink\n\
        \tDescription: WiVRn\n";

    #[test]
    fn parse_devices_reads_top_level_fields_only() {
        let devices = parse_devices(SINKS);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].name, "alsa_output.usb-WG2.analog-stereo");
        assert_eq!(devices[0].description, "WG2 Analog Stereo");
        assert_eq!(devices[1].name, "wivrn.sink");
        assert_eq!(devices[1].description, "WiVRn");
    }

    #[test]
    fn parse_devices_handles_empty_and_garbage_input() {
        assert!(parse_devices("").is_empty());
        assert!(parse_devices("no blocks here\nat all\n").is_empty());
    }

    #[test]
    fn parse_devices_reads_sources_too() {
        let listing = "Source #342\n\tName: wivrn.source\n\tDescription: WiVRn mic\n";
        let devices = parse_devices(listing);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "wivrn.source");
    }

    #[test]
    fn parse_stream_ids_takes_the_leading_column() {
        let listing = "34100\t63\t34099\tPipeWire\tfloat32le 2ch 48000Hz\n\
                       121\t112\t-\tPipeWire\tfloat32le 2ch 48000Hz\n";
        assert_eq!(parse_stream_ids(listing), vec!["34100", "121"]);
        assert!(parse_stream_ids("").is_empty());
        assert!(parse_stream_ids("garbage line\n").is_empty());
    }

    #[test]
    fn kind_labels_are_human_readable() {
        assert_eq!(Kind::Sink.label(), "output");
        assert_eq!(Kind::Source.label(), "input");
    }

    #[tokio::test]
    async fn set_default_rejects_blank_names() {
        assert!(set_default(Kind::Sink, "   ").await.is_err());
    }

    #[tokio::test]
    async fn route_ignores_blank_targets() {
        // Neither side is named, so nothing is attempted and nothing fails.
        let outcome = route(None, Some("  "), false).await;
        assert_eq!(outcome, RouteOutcome::default());
        assert!(outcome.is_empty());
    }

    #[tokio::test]
    async fn desktop_target_prefers_the_configured_device() {
        assert_eq!(
            desktop_target(Kind::Sink, "  explicit  ", Some("saved"), "wivrn.sink").await,
            Some("explicit".to_string())
        );
    }

    #[tokio::test]
    async fn desktop_target_never_returns_the_vr_device_from_the_saved_slot() {
        // The saved device is the VR one (e.g. lvr was restarted mid-session),
        // so it must not be chosen as the way back.
        let picked = desktop_target(Kind::Sink, "", Some("wivrn.sink"), "wivrn.sink").await;
        assert_ne!(picked.as_deref(), Some("wivrn.sink"));
    }

    #[test]
    fn route_outcome_summarises_changes() {
        let outcome = RouteOutcome {
            changes: vec!["output → a".into(), "input → b".into()],
            errors: Vec::new(),
        };
        assert_eq!(outcome.summary(), "output → a, input → b");
        assert!(!outcome.is_empty());
    }
}
