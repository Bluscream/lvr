//! Dashboard: status at a glance plus the buttons you actually press in VR.

use egui::{RichText, Ui};

use super::LvrApp;
use super::widgets::{self, BLUE, GREEN, GREY, ORANGE, RED};
use crate::state::Command;

pub fn show(app: &mut LvrApp, ui: &mut Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(4.0);
        status_banner(app, ui);
        ui.add_space(14.0);
        media_blocking_card(app, ui);
        ui.add_space(14.0);
        quick_actions_card(app, ui);
        ui.add_space(14.0);
        managed_apps(app, ui);
        ui.add_space(12.0);
    });
}

fn status_banner(app: &LvrApp, ui: &mut Ui) {
    let status = &app.status;

    // Header title row with last supervisor tick timestamp
    ui.horizontal(|ui| {
        ui.label(RichText::new("System Overview").size(18.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let updated = match status.last_tick {
                Some(at) => format!("Refreshed: {}", at.format("%H:%M:%S")),
                None => "Waiting for supervisor…".to_string(),
            };
            ui.label(RichText::new(updated).size(12.0).color(GREY));
        });
    });
    ui.add_space(6.0);

    // Primary status indicators in a unified grid/card container
    egui::Frame::group(ui.style())
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                widgets::pill_sized(
                    ui,
                    "WiVRn Server",
                    if status.wivrn_running { "Running" } else { "Stopped" },
                    widgets::on_off(status.wivrn_running),
                    115.0,
                );

                widgets::pill_sized(
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
                    115.0,
                );

                widgets::pill_sized(
                    ui,
                    "VRChat",
                    if status.vrchat_running { "Running" } else { "Not running" },
                    widgets::on_off(status.vrchat_running),
                    115.0,
                );

                widgets::pill_sized(
                    ui,
                    "Audio Output",
                    if status.audio_on_vr { "VR Headset" } else { "Desktop" },
                    if status.audio_on_vr { GREEN } else { BLUE },
                    115.0,
                );

                widgets::pill_sized(
                    ui,
                    "Managed Apps",
                    &format!(
                        "{}/{} active",
                        status.running_entry_count(),
                        status.entries.len()
                    ),
                    if status.running_entry_count() > 0 { GREEN } else { BLUE },
                    115.0,
                );

                if status.wivrn_running {
                    widgets::pill_sized(
                        ui,
                        "XR Session",
                        if status.session_running { "Active" } else { "Idle" },
                        widgets::on_off(status.session_running),
                        115.0,
                    );
                }

                widgets::pill_sized(
                    ui,
                    "Virtual Displays",
                    &format!("{} active", status.display_count),
                    if status.display_count > 0 { GREEN } else { GREY },
                    115.0,
                );

                if status.watchdog_paused {
                    widgets::pill_sized(ui, "Watchdog", "Paused", ORANGE, 115.0);
                }
            });
        });
}

fn media_blocking_card(app: &mut LvrApp, ui: &mut Ui) {
    egui::Frame::group(ui.style())
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            let full = ui.available_width();
            let spacing = ui.spacing().item_spacing.x;
            // Adaptive columns: 4 columns on large widths, 3 on medium, 2 on smaller, 1 on narrow.
            // Buttons are ~75% width compared to previous 3-column layout.
            let cols = if full >= 860.0 {
                4.0
            } else if full >= 640.0 {
                3.0
            } else if full >= 420.0 {
                2.0
            } else {
                1.0
            };
            let col_w = ((full - spacing * (cols - 1.0)) / cols).floor().max(180.0);

            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("🛡 VRChat Media & Domain Controls").size(16.0).strong());
                ui.label(RichText::new("• Zero-lag prefix hosts & yt-dlp stubbing").size(12.0).color(GREY));
            });
            ui.add_space(8.0);

            ui.horizontal_wrapped(|ui| {
                // Video Control
                let video_blocked = app.status.block_state.video_blocked;
                let (btn_label, tint) = if video_blocked {
                    ("🎬 Videos: Blocked", RED)
                } else {
                    ("🎬 Videos: Allowed", GREEN)
                };
                let resp = widgets::big_button(ui, btn_label, Some(tint), col_w);
                let resp = resp.on_hover_text(if video_blocked {
                    "Video playback is currently BLOCKED (CDNs redirected + yt-dlp locked).\nClick to allow video playback."
                } else {
                    "Video playback is ALLOWED.\nClick to block video streams & players (AVPro/UnityVideo CDNs + yt-dlp)."
                });
                if resp.clicked() {
                    app.shared.send(Command::ToggleBlockCategory(crate::domain_block::BlockCategory::Video));
                }

                // Image Control
                let img_blocked = app.status.block_state.images_blocked;
                let (btn_label, tint) = if img_blocked {
                    ("🖼 Images: Blocked", RED)
                } else {
                    ("🖼 Images: Allowed", GREEN)
                };
                let resp = widgets::big_button(ui, btn_label, Some(tint), col_w);
                let resp = resp.on_hover_text(if img_blocked {
                    "Image loading (imageHostUrlList) is BLOCKED.\nClick to allow image loading."
                } else {
                    "Image loading is ALLOWED.\nClick to block pure image domains (multi-list domains remain in Rest category)."
                });
                if resp.clicked() {
                    app.shared.send(Command::ToggleBlockCategory(crate::domain_block::BlockCategory::Images));
                }

                // String Control
                let str_blocked = app.status.block_state.strings_blocked;
                let (btn_label, tint) = if str_blocked {
                    ("📜 Strings: Blocked", RED)
                } else {
                    ("📜 Strings: Allowed", GREEN)
                };
                let resp = widgets::big_button(ui, btn_label, Some(tint), col_w);
                let resp = resp.on_hover_text(if str_blocked {
                    "String loading (stringHostUrlList) is BLOCKED.\nClick to allow string loading."
                } else {
                    "String loading is ALLOWED.\nClick to block pure string domains (multi-list domains remain in Rest category)."
                });
                if resp.clicked() {
                    app.shared.send(Command::ToggleBlockCategory(crate::domain_block::BlockCategory::Strings));
                }

                // Rest Control (domains in multiple lists, excluding VRChat internal & whiteListedAssetUrls)
                let rest_blocked = app.status.block_state.rest_blocked;
                let (btn_label, tint) = if rest_blocked {
                    ("🌐 Rest: Blocked", RED)
                } else {
                    ("🌐 Rest: Allowed", GREEN)
                };
                let resp = widgets::big_button(ui, btn_label, Some(tint), col_w);
                let resp = resp.on_hover_text(if rest_blocked {
                    "Rest domains (domains appearing in multiple lists) are BLOCKED.\nVRChat internal domains & whiteListedAssetUrls remain strictly protected.\nClick to allow."
                } else {
                    "Rest domains are ALLOWED.\nClick to block multi-list domains (*.github.io, ciel.topaz.chat, etc.) while keeping VRChat core assets protected."
                });
                if resp.clicked() {
                    app.shared.send(Command::ToggleBlockCategory(crate::domain_block::BlockCategory::Rest));
                }
            });
        });
}

fn quick_actions_card(app: &mut LvrApp, ui: &mut Ui) {
    egui::Frame::group(ui.style())
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            let full = ui.available_width();
            let spacing = ui.spacing().item_spacing.x;
            let cols = if full >= 860.0 {
                4.0
            } else if full >= 640.0 {
                3.0
            } else if full >= 420.0 {
                2.0
            } else {
                1.0
            };
            let col_w = ((full - spacing * (cols - 1.0)) / cols).floor().max(180.0);

            ui.label(RichText::new("⚡ Quick Actions").size(16.0).strong());
            ui.add_space(8.0);

            ui.horizontal_wrapped(|ui| {
                if widgets::big_button(ui, "🔄 Restart WiVRn", Some(BLUE), col_w).clicked() {
                    app.shared.send(Command::RestartWivrn);
                }

                if widgets::big_button(ui, "🔌 Disconnect Headset", None, col_w).clicked() {
                    app.shared.send(Command::DisconnectHeadset);
                }

                if app.status.audio_on_vr {
                    if widgets::big_button(ui, "🔊 Audio -> Desktop", Some(BLUE), col_w).clicked() {
                        app.shared.send(Command::SetAudioVr(false));
                    }
                } else if widgets::big_button(ui, "🎧 Audio -> VR", Some(BLUE), col_w).clicked() {
                    app.shared.send(Command::SetAudioVr(true));
                }

                if widgets::big_button(ui, "▶ Start All Triggered", Some(GREEN), col_w).clicked() {
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

                if app.status.virtual_display_created {
                    if widgets::big_button(ui, "🖥 Remove Display", Some(ORANGE), col_w).clicked() {
                        app.shared.send(Command::RemoveVirtualDisplay);
                    }
                } else if widgets::big_button(ui, "🖥 Create Display", Some(BLUE), col_w).clicked() {
                    app.shared.send(Command::CreateVirtualDisplay);
                }

                if widgets::big_button(ui, "🛑 Stop Everything VR", Some(RED), col_w).clicked() {
                    app.request_stop_all();
                }
            });
        });
}

fn managed_apps(app: &mut LvrApp, ui: &mut Ui) {
    egui::Frame::group(ui.style())
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("🚀 Managed Applications").size(16.0).strong());
                ui.label(
                    RichText::new(format!(
                        "• {}/{} running",
                        app.status.running_entry_count(),
                        app.status.entries.len()
                    ))
                    .size(12.0)
                    .color(GREY),
                );
            });
            ui.add_space(8.0);

            if app.status.entries.is_empty() {
                ui.label(
                    RichText::new("No applications configured yet. You can add and configure apps in the Autostart tab.")
                        .color(GREY),
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
                    let tint = if entry.running { GREEN } else { GREY };
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
