use egui::{RichText, Ui};

use crate::state::{Command, EntryStatus};
use crate::ui::widgets::{self, BIG_BUTTON_HEIGHT, BLUE, GREEN, GREY, ORANGE, RED, ROW_BUTTON_HEIGHT};
use crate::ui::LvrApp;

/// Formats the detail line for an entry, used by both the dashboard hover/status
/// and the autostart list.
pub fn detail_line(status: &EntryStatus, show_debug: bool) -> String {
    let mut parts = Vec::new();

    if let Some(err) = &status.last_error {
        parts.push(format!("error: {err}"));
    } else if let Some(secs) = status.start_in_secs {
        parts.push(format!("starting in {}", widgets::format_countdown(secs)));
    } else if let Some(secs) = status.stop_in_secs {
        parts.push(format!("stopping in {}", widgets::format_countdown(secs)));
    } else if status.suppressed {
        parts.push("stopped by hand (suppressed)".to_string());
    } else if status.running {
        parts.push("running".to_string());
    } else {
        parts.push("idle".to_string());
    }

    if show_debug && !status.pids.is_empty() {
        let pids: Vec<String> = status.pids.iter().map(|p| p.to_string()).collect();
        parts.push(format!("PID {}", pids.join(", ")));
    }

    parts.join(" · ")
}

/// Renders the main dashboard tab.
pub fn show(app: &mut LvrApp, ui: &mut Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(4.0);

        render_telemetry_hud(app, ui);

        ui.add_space(8.0);
        render_domain_shields(app, ui);

        ui.add_space(8.0);
        render_quick_actions(app, ui);

        ui.add_space(8.0);
        render_managed_apps(app, ui);

        ui.add_space(8.0);
    });
}

/// Section 2: Telemetry HUD (Live Status Chips)
fn render_telemetry_hud(app: &LvrApp, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        // WiVRn Server: Running (green) or Stopped (grey)
        let (wivrn_text, wivrn_color) = if app.status.wivrn_running {
            ("Running", GREEN)
        } else {
            ("Stopped", GREY)
        };
        if widgets::pill(ui, "WiVRn Server", wivrn_text, wivrn_color)
            .on_hover_text("Click to refresh status and recheck domain shields")
            .clicked()
        {
            app.send(Command::ReloadAll);
        }

        // Headset: Model name when connected, or Disconnected
        let (headset_text, headset_color) = if app.status.headset_connected {
            let name = if app.status.headset_name.is_empty() {
                "Connected"
            } else {
                &app.status.headset_name
            };
            (name, GREEN)
        } else {
            ("Disconnected", GREY)
        };
        widgets::pill(ui, "Headset", headset_text, headset_color);

        // VRChat: Running (green) or Offline (grey)
        let (vrc_text, vrc_color) = if app.status.vrchat_running {
            ("Running", GREEN)
        } else {
            ("Offline", GREY)
        };
        widgets::pill(ui, "VRChat", vrc_text, vrc_color);

        // Audio Output: Desktop or VR Headset
        let (audio_text, audio_color) = if app.status.audio_on_vr {
            ("VR Headset", BLUE)
        } else {
            ("Desktop", GREEN)
        };
        widgets::pill(ui, "Audio Output", audio_text, audio_color);

        // Managed Apps: Counter showing active / total
        let total_apps = app.status.entries.len();
        let running_apps = app.status.running_entry_count();
        let apps_text = format!("{running_apps}/{total_apps} active");
        let apps_color = if running_apps > 0 { GREEN } else { GREY };
        widgets::pill(ui, "Managed Apps", &apps_text, apps_color);

        // XR Session (appears when WiVRn is running): Active or Idle
        if app.status.wivrn_running {
            let (sess_text, sess_color) = if app.status.session_running {
                ("Active", GREEN)
            } else {
                ("Idle", GREY)
            };
            widgets::pill(ui, "XR Session", sess_text, sess_color);
        }

        // Virtual Display (appears when active)
        if app.status.virtual_display_created {
            let label = match &app.status.virtual_display_info {
                Some(info) => info.clone(),
                None => app.shared.config().virtual_display.resolution.clone(),
            };
            widgets::pill(ui, "Virtual Display", &label, BLUE);
        }

        // Watchdog (appears if paused)
        if app.status.watchdog_paused {
            widgets::pill(ui, "Watchdog", "Paused", ORANGE);
        }
    });
}

/// Section 3: VRChat Domain & Media Shield Controls
fn render_domain_shields(app: &mut LvrApp, ui: &mut Ui) {
    let lists = crate::domain_block::active_domains();
    let categories = lists.all_categories();

    let full = ui.available_width();
    let spacing = ui.spacing().item_spacing.x;
    let count = categories.len().max(1) as f32;
    let button_w = ((full - spacing * (count - 1.0)) / count).floor().max(110.0);

    ui.horizontal_wrapped(|ui| {
        for category in categories {
            let is_blocked = app.status.block_state.is_blocked(&category);
            let tint = if is_blocked { RED } else { GREEN };
            let display_name = category.label();

            let button = egui::Button::new(RichText::new(display_name).size(14.0).strong())
                .corner_radius(egui::CornerRadius::same(10))
                .fill(tint.gamma_multiply(0.20))
                .stroke((1.5, tint));

            if ui.add_sized(egui::Vec2::new(button_w, BIG_BUTTON_HEIGHT), button).clicked() {
                app.send(Command::ToggleBlockCategory(category));
            }
        }
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let shield_active = app.status.steam_shield_active;
        let (status_text, color) = if shield_active {
            ("DNS Shield: Installed in Steam", GREEN)
        } else {
            ("DNS Shield: Not in Steam Launch Options", ORANGE)
        };

        let opts_summary = if app.status.steam_launch_options.is_empty() {
            "No launch options set"
        } else {
            &app.status.steam_launch_options
        };
        ui.label(RichText::new(status_text).size(12.0).color(color))
            .on_hover_text(format!("Current Steam LaunchOptions:\n{opts_summary}"));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let btn_label = if shield_active {
                "Remove from Steam"
            } else {
                "Enable in Steam"
            };
            let btn_color = if shield_active { GREY } else { BLUE };
            let btn = egui::Button::new(RichText::new(btn_label).size(11.0).strong())
                .corner_radius(egui::CornerRadius::same(6))
                .fill(btn_color.gamma_multiply(0.15))
                .stroke((1.0, btn_color.gamma_multiply(0.5)));

            let tooltip = if shield_active {
                "Click to remove LD_PRELOAD DNS shield from VRChat's Steam launch options"
            } else {
                "Click to automatically add LD_PRELOAD DNS shield to VRChat in Steam (localconfig.vdf)"
            };

            if ui.add(btn).on_hover_text(tooltip).clicked() {
                app.send(Command::SetSteamShieldEnabled(!shield_active));
            }
        });
    });
}

/// Section 4: Quick Action Controls
fn render_quick_actions(app: &mut LvrApp, ui: &mut Ui) {
    let full = ui.available_width();
    let spacing = ui.spacing().item_spacing.x;
    let col_w = ((full - spacing * 2.0) / 3.0).floor().max(140.0);

    // Row 1: WiVRn & Audio controls
    ui.horizontal(|ui| {
        // Restart WiVRn
        if widgets::big_button(ui, "Restart WiVRn", Some(BLUE), col_w).clicked() {
            app.send(Command::RestartWivrn);
        }

        // Disconnect Headset
        let disconnect_tint = if app.status.headset_connected {
            Some(ORANGE)
        } else {
            None
        };
        if widgets::big_button(ui, "Disconnect Headset", disconnect_tint, col_w).clicked() {
            app.send(Command::DisconnectHeadset);
        }

        // Audio -> VR / Audio -> Desktop
        let (audio_lbl, audio_tint) = if app.status.audio_on_vr {
            ("Audio -> Desktop", Some(GREEN))
        } else {
            ("Audio -> VR", Some(BLUE))
        };
        if widgets::big_button(ui, audio_lbl, audio_tint, col_w).clicked() {
            app.send(Command::SetAudioVr(!app.status.audio_on_vr));
        }
    });

    ui.add_space(4.0);

    // Row 2: Companion trigger, Virtual Display, Stop Everything
    ui.horizontal(|ui| {
        // Start All Triggered: immediately launch companion apps whose triggers are active
        if widgets::big_button(ui, "Start All Triggered", Some(GREEN), col_w).clicked() {
            for entry in &app.status.entries {
                if entry.trigger_active && !entry.running {
                    app.send(Command::StartEntry(entry.id.clone()));
                }
            }
        }

        // Create Display / Remove Display
        let (disp_lbl, disp_tint) = if app.status.virtual_display_created {
            ("Remove Display", Some(ORANGE))
        } else {
            ("Create Display", Some(BLUE))
        };
        if widgets::big_button(ui, disp_lbl, disp_tint, col_w).clicked() {
            if app.status.virtual_display_created {
                app.send(Command::RemoveVirtualDisplay);
            } else {
                app.send(Command::CreateVirtualDisplay);
            }
        }

        // Stop Everything VR: prompts confirmation dialog via LvrApp
        if widgets::big_button(ui, "Stop Everything VR", Some(RED), col_w).clicked() {
            app.request_stop_all();
        }
    });
}

/// Section 5: Managed Companion Applications
fn render_managed_apps(app: &mut LvrApp, ui: &mut Ui) {
    if app.status.entries.is_empty() {
        ui.label(RichText::new("No companion applications configured.").color(GREY).size(14.0));
        return;
    }

    let show_debug = app.shared.config().general.show_debug_info;
    let full = ui.available_width();
    let spacing = ui.spacing().item_spacing.x;
    let card_w = ((full - spacing) / 2.0).floor().max(160.0);

    ui.horizontal_wrapped(|ui| {
        for entry in &app.status.entries {
            let is_running = entry.running;
            let (circle, status_color) = if is_running {
                ("●", GREEN)
            } else {
                ("○", GREY)
            };

            let label_text = format!("{circle}  {}", entry.name);
            let hover_text = detail_line(entry, show_debug);

            let mut button = egui::Button::new(RichText::new(label_text).size(15.0).strong())
                .corner_radius(egui::CornerRadius::same(8));

            if is_running {
                button = button
                    .fill(GREEN.gamma_multiply(0.18))
                    .stroke((1.2, GREEN));
            } else {
                button = button
                    .fill(status_color.gamma_multiply(0.08))
                    .stroke((1.0, GREY.gamma_multiply(0.5)));
            }

            let resp = ui.add_sized(egui::Vec2::new(card_w, ROW_BUTTON_HEIGHT), button)
                .on_hover_text(hover_text);

            if resp.clicked() {
                if is_running {
                    app.send(Command::StopEntry(entry.id.clone()));
                } else {
                    app.send(Command::StartEntry(entry.id.clone()));
                }
            }
        }
    });
}
