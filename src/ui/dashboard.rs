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
        if app.status.wivrn_running {
            if widgets::big_button(ui, "Stop WiVRn", Some(ORANGE), button_width).clicked() {
                app.shared.send(Command::StopWivrn);
            }
        } else if widgets::big_button(ui, "Start WiVRn", Some(GREEN), button_width).clicked() {
            app.shared.send(Command::StartWivrn);
        }
        if widgets::big_button(ui, "Disconnect headset", None, button_width).clicked() {
            app.shared.send(Command::DisconnectHeadset);
        }
    });

    ui.horizontal_wrapped(|ui| {
        if app.status.audio_on_vr {
            if widgets::big_button(ui, "Audio → Desktop", Some(BLUE), button_width).clicked() {
                app.shared.send(Command::SetAudioVr(false));
            }
        } else if widgets::big_button(ui, "Audio → VR", Some(BLUE), button_width).clicked() {
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

fn managed_apps(app: &mut LvrApp, ui: &mut Ui) {
    widgets::heading(ui, "Managed apps");

    if app.status.entries.is_empty() {
        ui.label(
            RichText::new("Nothing configured yet — add apps in the Autostart tab.").color(GREY),
        );
        return;
    }

    let entries = app.status.entries.clone();
    for entry in entries {
        egui::Frame::group(ui.style())
            .corner_radius(egui::CornerRadius::same(10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let color = widgets::on_off(entry.running);
                    ui.label(RichText::new("⏺").size(20.0).color(color));
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&entry.name).size(17.0).strong());
                        ui.label(RichText::new(detail_line(&entry)).size(13.0).color(GREY));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if entry.running {
                            if widgets::row_button(ui, "Stop", Some(ORANGE), 110.0).clicked() {
                                app.shared.send(Command::StopEntry(entry.id.clone()));
                            }
                        } else if widgets::row_button(ui, "Start", Some(GREEN), 110.0).clicked() {
                            app.shared.send(Command::StartEntry(entry.id.clone()));
                        }
                    });
                });
            });
    }
}

/// One line of context under an app's name.
pub fn detail_line(entry: &crate::state::EntryStatus) -> String {
    if let Some(error) = &entry.last_error {
        return format!("error: {error}");
    }
    if let Some(secs) = entry.start_in_secs {
        return format!("starting in {}", widgets::format_countdown(secs));
    }
    if let Some(secs) = entry.stop_in_secs {
        return format!("stopping in {}", widgets::format_countdown(secs));
    }
    if entry.running {
        let pids = entry
            .pids
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return format!("running — pid {pids}");
    }
    if entry.suppressed {
        return "stopped by you — will not auto-start until the trigger cycles".to_string();
    }
    if entry.trigger_active {
        return "trigger active".to_string();
    }
    "idle".to_string()
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
        assert_eq!(detail_line(&entry), "running — pid 42");

        entry.stop_in_secs = Some(90);
        assert_eq!(detail_line(&entry), "stopping in 1:30");

        entry.start_in_secs = Some(5);
        assert_eq!(detail_line(&entry), "starting in 5s");

        entry.last_error = Some("boom".into());
        assert_eq!(detail_line(&entry), "error: boom");
    }

    #[test]
    fn detail_line_describes_idle_states() {
        let mut entry = EntryStatus::default();
        assert_eq!(detail_line(&entry), "idle");
        entry.trigger_active = true;
        assert_eq!(detail_line(&entry), "trigger active");
        entry.suppressed = true;
        assert!(detail_line(&entry).starts_with("stopped by you"));
    }
}
