//! Dashboard: status at a glance plus the buttons you actually press in VR.

use egui::{RichText, Ui};

use super::LvrApp;
use super::widgets::{self, BLUE, GREEN, GREY, ORANGE, RED};
use crate::state::Command;

pub fn show(app: &mut LvrApp, ui: &mut Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        status_row(app, ui);
        ui.add_space(10.0);
        actions(app, ui);
        ui.add_space(10.0);
        managed_apps(app, ui);
    });
}

fn status_row(app: &LvrApp, ui: &mut Ui) {
    let status = &app.status;
    ui.horizontal_wrapped(|ui| {
        widgets::pill(
            ui,
            "WiVRn server",
            if status.wivrn_running {
                "Running"
            } else {
                "Stopped"
            },
            widgets::on_off(status.wivrn_running),
        );
        widgets::pill(
            ui,
            "Headset",
            &if status.headset_connected {
                if status.headset_name.is_empty() {
                    "Connected".to_string()
                } else {
                    status.headset_name.clone()
                }
            } else {
                "Disconnected".to_string()
            },
            widgets::on_off(status.headset_connected),
        );
        widgets::pill(
            ui,
            "VRChat",
            if status.vrchat_running {
                "Running"
            } else {
                "Not running"
            },
            widgets::on_off(status.vrchat_running),
        );
        widgets::pill(
            ui,
            "Audio",
            if status.audio_on_vr { "VR" } else { "Desktop" },
            if status.audio_on_vr { GREEN } else { BLUE },
        );
        widgets::pill(
            ui,
            "Managed apps",
            &format!(
                "{} / {}",
                status.running_entry_count(),
                status.entries.len()
            ),
            BLUE,
        );
        if status.wivrn_running {
            widgets::pill(
                ui,
                "XR session",
                if status.session_running {
                    "Active"
                } else {
                    "Idle"
                },
                widgets::on_off(status.session_running),
            );
        }
        if let Some(profile) = &status.steam_profile {
            widgets::pill(ui, "VRC Proton", profile, BLUE);
        } else if !status.steam_compat_tool.is_empty() {
            widgets::pill(ui, "VRC Proton", &status.steam_compat_tool, ORANGE);
        }
        if status.watchdog_paused {
            widgets::pill(ui, "Watchdog", "Paused", ORANGE);
        }
    });

    ui.add_space(4.0);
    let updated = match status.last_tick {
        Some(at) => format!("last update {}", at.format("%H:%M:%S")),
        None => "waiting for the first supervisor pass…".to_string(),
    };
    ui.label(RichText::new(updated).size(12.0).color(GREY));
}

fn actions(app: &mut LvrApp, ui: &mut Ui) {
    widgets::heading(ui, "Actions");

    let full = ui.available_width();
    let button_width = ((full - 30.0) / 3.0).max(150.0);

    ui.horizontal_wrapped(|ui| {
        if widgets::big_button(ui, "Restart WiVRn", Some(BLUE), button_width).clicked() {
            app.shared.send(Command::RestartWivrn);
        }
        steam_profile_button(app, ui, button_width);
        if widgets::big_button(ui, "Disconnect headset", None, button_width).clicked() {
            app.shared.send(Command::DisconnectHeadset);
        }
    });

    ui.horizontal_wrapped(|ui| {
        if app.status.audio_on_vr {
            if widgets::big_button(ui, "Audio -> Desktop", Some(BLUE), button_width).clicked() {
                app.shared.send(Command::SetAudioVr(false));
            }
        } else if widgets::big_button(ui, "Audio -> VR", Some(BLUE), button_width).clicked() {
            app.shared.send(Command::SetAudioVr(true));
        }
        if widgets::big_button(ui, "Start all triggered", Some(GREEN), button_width).clicked() {
            let ids: Vec<String> = app
                .status
                .entries
                .iter()
                .filter(|e| !e.running)
                .map(|e| e.id.clone())
                .collect();
            for id in ids {
                app.shared.send(Command::StartEntry(id));
            }
        }
        if widgets::big_button(ui, "Stop everything VR", Some(RED), button_width).clicked() {
            app.request_stop_all();
        }
    });
}

/// The Proton profile toggle: shows the profile it would switch *to*.
fn steam_profile_button(app: &mut LvrApp, ui: &mut Ui, width: f32) {
    let steam = app.shared.config().steam.clone();
    if !steam.enabled {
        return;
    }

    if app.status.steam_switching {
        let _ = widgets::big_button(ui, "Switching…", Some(GREY), width);
        return;
    }

    let active = app.status.steam_profile.clone();
    let Some(next) = steam.next_profile(active.as_deref()) else {
        return;
    };
    let tint = if active.is_none() { ORANGE } else { BLUE };
    let response = widgets::big_button(ui, &next.name, Some(tint), width);
    let response = response.on_hover_text(match &active {
        Some(name) => format!("VRChat is on \"{name}\" — switch to \"{}\"", next.name),
        None => format!(
            "VRChat is on an unknown compat tool ({}) — switch to \"{}\"",
            if app.status.steam_compat_tool.is_empty() {
                "none"
            } else {
                &app.status.steam_compat_tool
            },
            next.name
        ),
    });
    if response.clicked() {
        app.request_steam_switch(next.name.clone());
    }
}

fn managed_apps(app: &mut LvrApp, ui: &mut Ui) {
    widgets::heading(ui, "Managed apps");

    if app.status.entries.is_empty() {
        ui.label(
            RichText::new("Nothing configured yet — add apps in the Autostart tab.").color(GREY),
        );
        return;
    }

    let full = ui.available_width();
    let spacing = ui.spacing().item_spacing.x;
    let cols = 4.0;
    let button_width = ((full - spacing * (cols - 1.0)) / cols).max(110.0);

    let entries = app.status.entries.clone();
    ui.horizontal_wrapped(|ui| {
        for entry in entries {
            let tint = if entry.running { ORANGE } else { GREEN };
            let status_text = detail_line(&entry, app.shared.config().general.show_debug_info);
            let response = widgets::big_button(ui, &entry.name, Some(tint), button_width);
            let response = response.on_hover_text(format!(
                "{} ({})",
                if entry.running { "Click to stop" } else { "Click to start" },
                status_text
            ));
            if response.clicked() {
                if entry.running {
                    app.shared.send(Command::StopEntry(entry.id.clone()));
                } else {
                    app.shared.send(Command::StartEntry(entry.id.clone()));
                }
            }
        }
    });
}

/// One line of context under an app's name.
pub fn detail_line(entry: &crate::state::EntryStatus, show_debug: bool) -> String {
    if let Some(error) = &entry.last_error {
        if show_debug {
            return format!("error: {error} (pids: {:?})", entry.pids);
        } else {
            return format!("error: {error}");
        }
    }
    if let Some(secs) = entry.start_in_secs {
        if show_debug {
            return format!(
                "starting in {} [trig_active: {}, suppressed: {}]",
                widgets::format_countdown(secs),
                entry.trigger_active,
                entry.suppressed
            );
        } else {
            return format!("starting in {}", widgets::format_countdown(secs));
        }
    }
    if let Some(secs) = entry.stop_in_secs {
        let pids = entry
            .pids
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        if show_debug {
            return format!(
                "stopping in {} [pid {}, trig_active: {}]",
                widgets::format_countdown(secs),
                if pids.is_empty() { "none" } else { &pids },
                entry.trigger_active
            );
        } else {
            return format!("stopping in {}", widgets::format_countdown(secs));
        }
    }
    if entry.running {
        if show_debug {
            let pids = entry
                .pids
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return format!(
                "running — pid {} [trig_active: {}, count: {}]",
                if pids.is_empty() { "none" } else { &pids },
                entry.trigger_active,
                entry.pids.len()
            );
        } else {
            return "running".to_string();
        }
    }
    if entry.suppressed {
        if show_debug {
            return format!(
                "stopped by you [suppressed: true, trig_active: {}, pids: {:?}]",
                entry.trigger_active, entry.pids
            );
        } else {
            return "stopped by you".to_string();
        }
    }
    if entry.trigger_active {
        if show_debug {
            return format!("trigger active [running: false, pids: {:?}]", entry.pids);
        } else {
            return "trigger active".to_string();
        }
    }
    if show_debug {
        format!("idle [trig_active: false, pids: {:?}]", entry.pids)
    } else {
        "idle".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::EntryStatus;

    #[test]
    fn detail_line_prefers_errors_then_timers() {
        let mut entry = EntryStatus {
            running: true,
            pids: vec![42],
            ..Default::default()
        };
        assert_eq!(detail_line(&entry, false), "running");
        assert_eq!(detail_line(&entry, true), "running — pid 42 [trig_active: false, count: 1]");

        entry.stop_in_secs = Some(90);
        assert_eq!(detail_line(&entry, false), "stopping in 1:30");
        assert_eq!(detail_line(&entry, true), "stopping in 1:30 [pid 42, trig_active: false]");

        entry.start_in_secs = Some(5);
        assert_eq!(detail_line(&entry, false), "starting in 5s");

        entry.last_error = Some("boom".into());
        assert_eq!(detail_line(&entry, false), "error: boom");
    }

    #[test]
    fn detail_line_describes_idle_states() {
        let mut entry = EntryStatus::default();
        assert_eq!(detail_line(&entry, false), "idle");
        assert_eq!(detail_line(&entry, true), "idle [trig_active: false, pids: []]");
        entry.trigger_active = true;
        assert_eq!(detail_line(&entry, false), "trigger active");
        entry.suppressed = true;
        assert_eq!(detail_line(&entry, false), "stopped by you");
    }
}
