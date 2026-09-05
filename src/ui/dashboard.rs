//! Dashboard: Dynamic, fully responsive VR-first control center built from scratch.
//!
//! Layout Philosophy:
//! - Strictly dynamic column grids that adapt to ANY window width (1 to N columns).
//! - All buttons and cards compute their exact layout dynamically based on `ui.available_width()`,
//!   guaranteeing ZERO right or bottom overflow regardless of window dimensions.
//! - VR Ergonomics: Large, tactile click areas (minimum 48-52px height) with glowing borders
//!   and distinct visual state cues for laser pointers.
//! - Full vertical scroll container so content never clips at the bottom.

use egui::{
    Color32, CornerRadius, Frame, Margin, Response, RichText, ScrollArea, Stroke, Ui, Vec2,
};

use super::LvrApp;
use super::widgets::{self, BLUE, GREEN, GREY, ORANGE, RED};
use crate::state::Command;

/// Main Dashboard view renderer.
pub fn show(app: &mut LvrApp, ui: &mut Ui) {
    let max_w = ui.available_width();
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(max_w);
            render_telemetry_hud(app, ui);
            ui.add_space(6.0);
            render_domain_shield_card(app, ui);
            ui.add_space(6.0);
            render_quick_actions_card(app, ui);
            ui.add_space(6.0);
            render_managed_applications_card(app, ui);
        });
}

// ---------------------------------------------------------------------------
// 2. Telemetry HUD (Status Chips / Pills)
// ---------------------------------------------------------------------------

fn render_telemetry_hud(app: &LvrApp, ui: &mut Ui) {
    let status = &app.status;

    struct TelemetryItem<'a> {
        label: &'static str,
        value: &'a str,
        color: Color32,
    }

    let mut items: Vec<TelemetryItem> = Vec::with_capacity(8);
    items.push(TelemetryItem {
        label: "WiVRn Server",
        value: if status.wivrn_running { "Running" } else { "Stopped" },
        color: widgets::on_off(status.wivrn_running),
    });

    let headset_val = if status.headset_connected {
        if status.headset_name.is_empty() { "Connected" } else { &status.headset_name }
    } else {
        "Disconnected"
    };
    items.push(TelemetryItem {
        label: "Headset",
        value: headset_val,
        color: widgets::on_off(status.headset_connected),
    });

    items.push(TelemetryItem {
        label: "VRChat",
        value: if status.vrchat_running { "Running" } else { "Offline" },
        color: widgets::on_off(status.vrchat_running),
    });

    items.push(TelemetryItem {
        label: "Audio Output",
        value: if status.audio_on_vr { "VR Headset" } else { "Desktop" },
        color: if status.audio_on_vr { GREEN } else { BLUE },
    });

    let apps_val = format!("{}/{} active", status.running_entry_count(), status.entries.len());
    items.push(TelemetryItem {
        label: "Managed Apps",
        value: &apps_val,
        color: if status.running_entry_count() > 0 { GREEN } else { GREY },
    });

    if status.wivrn_running {
        items.push(TelemetryItem {
            label: "XR Session",
            value: if status.session_running { "Active" } else { "Idle" },
            color: if status.session_running { GREEN } else { GREY },
        });
    }

    if status.virtual_display_created {
        items.push(TelemetryItem {
            label: "Virtual Display",
            value: "Active",
            color: GREEN,
        });
    }

    if status.watchdog_paused {
        items.push(TelemetryItem {
            label: "Watchdog",
            value: "Paused",
            color: ORANGE,
        });
    }

    render_card(ui, |ui| {
        render_dynamic_grid(ui, items.len(), 140.0, 6, |ui, idx, col_w| {
            let item = &items[idx];
            widgets::pill_sized(ui, item.label, item.value, item.color, col_w);
        });
    });
}

// ---------------------------------------------------------------------------
// 3. VRChat Domain & Media Shield Card
// ---------------------------------------------------------------------------

fn render_domain_shield_card(app: &mut LvrApp, ui: &mut Ui) {
    render_card(ui, |ui| {
        // Define the 4 items
        struct ShieldItem {
            label: &'static str,
            blocked: bool,
            category: crate::domain_block::BlockCategory,
            hover_blocked: &'static str,
            hover_allowed: &'static str,
        }

        let items = [
            ShieldItem {
                label: "Videos",
                blocked: app.status.block_state.video_blocked,
                category: crate::domain_block::BlockCategory::Video,
                hover_blocked: "Video playback is BLOCKED (CDNs redirected + yt-dlp locked).\nClick to allow.",
                hover_allowed: "Video playback is ALLOWED.\nClick to block video streams & players.",
            },
            ShieldItem {
                label: "Images",
                blocked: app.status.block_state.images_blocked,
                category: crate::domain_block::BlockCategory::Images,
                hover_blocked: "Image loading is BLOCKED.\nClick to allow.",
                hover_allowed: "Image loading is ALLOWED.\nClick to block pure image domains.",
            },
            ShieldItem {
                label: "Strings",
                blocked: app.status.block_state.strings_blocked,
                category: crate::domain_block::BlockCategory::Strings,
                hover_blocked: "String loading is BLOCKED.\nClick to allow.",
                hover_allowed: "String loading is ALLOWED.\nClick to block pure string domains.",
            },
            ShieldItem {
                label: "Rest",
                blocked: app.status.block_state.rest_blocked,
                category: crate::domain_block::BlockCategory::Rest,
                hover_blocked: "Multi-list domains are BLOCKED (VRChat core assets stay protected).\nClick to allow.",
                hover_allowed: "Multi-list domains are ALLOWED.\nClick to block multi-list domains (*.github.io, ciel.topaz.chat).",
            },
        ];

        render_dynamic_grid(ui, items.len(), 170.0, 4, |ui, idx, col_w| {
            let item = &items[idx];
            let (btn_title, tint) = if item.blocked {
                (format!("{} Blocked", item.label), RED)
            } else {
                (format!("{} Allowed", item.label), GREEN)
            };
            let resp = render_vr_button(ui, &btn_title, tint, col_w);
            let resp = resp.on_hover_text(if item.blocked { item.hover_blocked } else { item.hover_allowed });
            if resp.clicked() {
                app.shared.send(Command::ToggleBlockCategory(item.category));
            }
        });
    });
}

// ---------------------------------------------------------------------------
// 4. Quick Actions Card
// ---------------------------------------------------------------------------

fn render_quick_actions_card(app: &mut LvrApp, ui: &mut Ui) {
    render_card(ui, |ui| {
        enum ActionKind {
            RestartWivrn,
            DisconnectHeadset,
            ToggleAudio(bool),
            StartAll,
            ToggleDisplay(bool),
            StopAll,
        }

        struct ActionItem {
            title: &'static str,
            tint: Color32,
            hover: &'static str,
            kind: ActionKind,
        }

        let mut actions = Vec::new();
        actions.push(ActionItem {
            title: "Restart WiVRn",
            tint: BLUE,
            hover: "Restart the WiVRn OpenXR streaming service",
            kind: ActionKind::RestartWivrn,
        });
        actions.push(ActionItem {
            title: "Disconnect Headset",
            tint: GREY,
            hover: "Disconnect active WiVRn client connection",
            kind: ActionKind::DisconnectHeadset,
        });
        if app.status.audio_on_vr {
            actions.push(ActionItem {
                title: "Audio -> Desktop",
                tint: BLUE,
                hover: "Route audio back to desktop speakers/headphones",
                kind: ActionKind::ToggleAudio(false),
            });
        } else {
            actions.push(ActionItem {
                title: "Audio -> VR",
                tint: BLUE,
                hover: "Route audio to WiVRn headset virtual sink",
                kind: ActionKind::ToggleAudio(true),
            });
        }
        actions.push(ActionItem {
            title: "Start All Triggered",
            tint: GREEN,
            hover: "Launch all enabled autostart apps that are not currently running",
            kind: ActionKind::StartAll,
        });
        if app.status.virtual_display_created {
            actions.push(ActionItem {
                title: "Remove Display",
                tint: ORANGE,
                hover: "Disable and remove the virtual display output",
                kind: ActionKind::ToggleDisplay(false),
            });
        } else {
            actions.push(ActionItem {
                title: "Create Display",
                tint: BLUE,
                hover: "Create virtual display output using configured resolution",
                kind: ActionKind::ToggleDisplay(true),
            });
        }
        actions.push(ActionItem {
            title: "Stop Everything VR",
            tint: RED,
            hover: "Safely terminate all running managed apps and VR processes",
            kind: ActionKind::StopAll,
        });

        render_dynamic_grid(ui, actions.len(), 170.0, 4, |ui, idx, col_w| {
            let action = &actions[idx];
            let resp = render_vr_button(ui, action.title, action.tint, col_w)
                .on_hover_text(action.hover);
            if resp.clicked() {
                match action.kind {
                    ActionKind::RestartWivrn => app.shared.send(Command::RestartWivrn),
                    ActionKind::DisconnectHeadset => app.shared.send(Command::DisconnectHeadset),
                    ActionKind::ToggleAudio(vr) => app.shared.send(Command::SetAudioVr(vr)),
                    ActionKind::StartAll => {
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
                    ActionKind::ToggleDisplay(create) => {
                        if create {
                            app.shared.send(Command::CreateVirtualDisplay);
                        } else {
                            app.shared.send(Command::RemoveVirtualDisplay);
                        }
                    }
                    ActionKind::StopAll => app.request_stop_all(),
                }
            }
        });
    });
}

// ---------------------------------------------------------------------------
// 5. Managed Applications Card
// ---------------------------------------------------------------------------

fn render_managed_applications_card(app: &mut LvrApp, ui: &mut Ui) {
    render_card(ui, |ui| {
        if app.status.entries.is_empty() {
            ui.label(
                RichText::new("No applications configured yet. Add apps in the Autostart tab.")
                    .color(GREY),
            );
            return;
        }

        let entries = app.status.entries.clone();
        render_dynamic_grid(ui, entries.len(), 170.0, 4, |ui, idx, col_w| {
            let entry = &entries[idx];
            let tint = if entry.running { GREEN } else { GREY };
            let status_text = detail_line(entry, app.shared.config().general.show_debug_info);
            let label = if entry.running {
                format!("● {}", entry.name)
            } else {
                format!("○ {}", entry.name)
            };
            let resp = render_vr_button(ui, &label, tint, col_w);
            let resp = resp.on_hover_text(format!(
                "{} ({})",
                if entry.running { "Click to stop" } else { "Click to start" },
                status_text
            ));
            if resp.clicked() {
                if entry.running {
                    app.shared.send(Command::StopEntry(entry.id.clone()));
                } else {
                    app.shared.send(Command::StartEntry(entry.id.clone()));
                }
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Dynamic Grid & VR Touch Helpers
// ---------------------------------------------------------------------------

/// Renders items into a dynamic grid that automatically calculates the number
/// of columns based on `ui.available_width()`, perfectly dividing space so
/// that items NEVER overflow horizontally or clip on the right edge.
fn render_dynamic_grid(
    ui: &mut Ui,
    total_items: usize,
    target_item_w: f32,
    max_cols: usize,
    mut render_item: impl FnMut(&mut Ui, usize, f32),
) {
    // ui.available_width() inside a Frame accounts for inner_margin.
    // Subtract a small 2.0px buffer to prevent floating-point rounding or scrollbar edges from overflowing.
    let total_w = (ui.available_width() - 2.0).max(100.0);
    let spacing = ui.spacing().item_spacing.x;

    // Calculate how many columns can fit without shrinking below target_item_w
    let mut num_cols = 1;
    for c in (2..=max_cols).rev() {
        let needed_spacing = spacing * (c as f32 - 1.0);
        if total_w > needed_spacing {
            let col_w = (total_w - needed_spacing) / (c as f32);
            if col_w >= target_item_w {
                num_cols = c;
                break;
            }
        }
    }

    // Now compute the exact column width so columns fill available width without overflowing
    let needed_spacing = spacing * (num_cols as f32 - 1.0);
    let col_w = ((total_w - needed_spacing) / (num_cols as f32)).floor().max(50.0);

    // Render items in rows of `num_cols`
    for row_start in (0..total_items).step_by(num_cols) {
        ui.horizontal(|ui| {
            for i in 0..num_cols {
                let item_idx = row_start + i;
                if item_idx < total_items {
                    render_item(ui, item_idx, col_w);
                }
            }
        });
    }
}

/// Renders a section card with a dark glass container and subtle border (zero header clutter).
fn render_card<R>(
    ui: &mut Ui,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    Frame::group(ui.style())
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(8))
        .fill(Color32::from_rgb(0x19, 0x1b, 0x20))
        .stroke(Stroke::new(1.0, Color32::from_rgb(0x2c, 0x30, 0x38)))
        .show(ui, add_contents)
        .inner
}

/// A tactile button sized for VR laser pointer interaction (50px height),
/// with prominent typography, smooth 10px corners, and glowing tint borders.
fn render_vr_button(ui: &mut Ui, label: &str, tint: Color32, width: f32) -> Response {
    let button = egui::Button::new(RichText::new(label).size(14.0).strong())
        .corner_radius(CornerRadius::same(10))
        .fill(tint.gamma_multiply(0.20))
        .stroke(Stroke::new(1.5, tint));

    ui.add_sized(Vec2::new(width, 50.0), button)
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
