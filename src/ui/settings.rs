//! Settings tab.

use egui::{RichText, Ui};

use super::LvrApp;
use super::widgets::{self, BLUE, GREEN, GREY, ORANGE};
use crate::procs;
use crate::state::Command;

pub fn show(app: &mut LvrApp, ui: &mut Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        general(app, ui);
        ui.add_space(12.0);
        wivrn(app, ui);
        ui.add_space(12.0);
        vrchat(app, ui);
        ui.add_space(12.0);
        vrc_files_and_tools(app, ui);
        ui.add_space(12.0);
        virtual_display(app, ui);
        ui.add_space(12.0);
        about(app, ui);
    });
}

fn general(app: &mut LvrApp, ui: &mut Ui) {
    widgets::heading(ui, "Interface");
    egui::Grid::new("settings-general")
        .num_columns(2)
        .spacing([14.0, 12.0])
        .min_col_width(190.0)
        .show(ui, |ui| {
            general_interface_rows(app, ui);
            general_runtime_rows(app, ui);
        });
}

fn general_interface_rows(app: &mut LvrApp, ui: &mut Ui) {
    ui.label("UI scale");
    {
        let mut config = app.shared.config();
        ui.add(
            egui::Slider::new(&mut config.general.ui_scale, 0.8..=2.5)
                .step_by(0.05)
                .text("bigger = easier to hit in VR"),
        );
    }
    ui.end_row();

    ui.label("Start hidden");
    {
        let mut config = app.shared.config();
        widgets::toggle(
            ui,
            &mut config.general.start_hidden,
            "start in the tray without opening the window",
        );
    }
    ui.end_row();

    ui.label("Close button");
    {
        let mut config = app.shared.config();
        widgets::toggle(
            ui,
            &mut config.general.close_to_tray,
            "hides to tray instead of quitting",
        );
    }
    ui.end_row();

    ui.label("Confirm stop-all");
    {
        let mut config = app.shared.config();
        widgets::toggle(
            ui,
            &mut config.general.confirm_stop_all,
            "ask before “Stop everything VR”",
        );
    }
    ui.end_row();

    ui.label("Show debug info");
    {
        let mut config = app.shared.config();
        widgets::toggle(
            ui,
            &mut config.general.show_debug_info,
            "show PIDs and process details in the status column",
        );
    }
    ui.end_row();
}

fn general_runtime_rows(app: &mut LvrApp, ui: &mut Ui) {
    ui.label("Poll interval");
    {
        let mut config = app.shared.config();
        ui.add(
            egui::DragValue::new(&mut config.general.poll_interval_ms)
                .range(200..=60_000)
                .speed(50.0)
                .suffix(" ms"),
        );
    }
    ui.end_row();

    ui.label("Relaunch debounce");
    {
        let mut config = app.shared.config();
        ui.add(
            egui::DragValue::new(&mut config.general.relaunch_debounce_secs)
                .range(1..=3600)
                .speed(1.0)
                .suffix(" s"),
        );
    }
    ui.end_row();

    ui.label("Stop grace (SIGTERM->SIGKILL)");
    {
        let mut config = app.shared.config();
        ui.add(
            egui::DragValue::new(&mut config.general.stop_grace_secs)
                .range(1..=300)
                .speed(1.0)
                .suffix(" s"),
        );
    }
    ui.end_row();

    ui.label("Log history");
    {
        let mut config = app.shared.config();
        ui.add(
            egui::DragValue::new(&mut config.general.log_capacity)
                .range(50..=100_000)
                .speed(10.0)
                .suffix(" lines"),
        );
    }
    ui.end_row();

    ui.label("Terminal command");
    ui.vertical(|ui| {
        {
            let mut config = app.shared.config();
            ui.add(
                egui::TextEdit::singleline(&mut config.general.terminal)
                    .desired_width(430.0)
                    .hint_text("auto-detect"),
            );
        }
        let detected = procs::detect_terminal().unwrap_or("none found");
        ui.label(
            RichText::new(format!(
                "{{cmd}} is replaced by the app. Detected: {detected}"
            ))
            .size(12.0)
            .color(GREY),
        );
    });
    ui.end_row();
}

fn wivrn(app: &mut LvrApp, ui: &mut Ui) {
    widgets::heading(ui, "WiVRn");
    egui::Grid::new("settings-wivrn")
        .num_columns(2)
        .spacing([14.0, 12.0])
        .min_col_width(190.0)
        .show(ui, |ui| {
            ui.label("Watchdog");
            {
                let mut config = app.shared.config();
                widgets::toggle(
                    ui,
                    &mut config.wivrn.watchdog,
                    "restart WiVRn whenever it stops or crashes",
                );
            }
            ui.end_row();

            ui.label("Start command");
            {
                let mut config = app.shared.config();
                ui.add(
                    egui::TextEdit::singleline(&mut config.wivrn.start_command)
                        .desired_width(f32::INFINITY),
                );
            }
            ui.end_row();

            ui.label("Restart delay");
            {
                let mut config = app.shared.config();
                ui.add(
                    egui::DragValue::new(&mut config.wivrn.restart_delay_secs)
                        .range(1..=3600)
                        .speed(1.0)
                        .suffix(" s"),
                );
            }
            ui.end_row();

            ui.label("Give up after");
            {
                let mut config = app.shared.config();
                ui.add(
                    egui::DragValue::new(&mut config.wivrn.max_consecutive_failures)
                        .range(0..=100)
                        .speed(1.0)
                        .suffix(" failed tries (0 = never)"),
                );
            }
            ui.end_row();

            ui.label("Flatpak id");
            {
                let mut config = app.shared.config();
                ui.add(
                    egui::TextEdit::singleline(&mut config.wivrn.flatpak_id).desired_width(430.0),
                );
            }
            ui.end_row();
        });

    if app.status.watchdog_paused {
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Watchdog is currently paused because WiVRn was stopped on purpose. \
                 Press “Start WiVRn” to resume supervision.",
            )
            .size(13.0)
            .color(ORANGE),
        );
    }
    if app.status.wivrn_failures > 0 {
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!(
                "{} restart attempt(s) since WiVRn was last seen running.",
                app.status.wivrn_failures
            ))
            .size(13.0)
            .color(ORANGE),
        );
    }
}

fn vrchat(app: &mut LvrApp, ui: &mut Ui) {
    widgets::heading(ui, "VRChat detection");
    ui.label(
        RichText::new(
            "One pattern per line, matched case-insensitively against every process command line.",
        )
        .size(13.0)
        .color(GREY),
    );
    let mut text = app.shared.config().general.vrchat_match.join("\n");
    if ui
        .add(
            egui::TextEdit::multiline(&mut text)
                .desired_rows(3)
                .desired_width(430.0)
                .font(egui::TextStyle::Monospace),
        )
        .changed()
    {
        let patterns: Vec<String> = text
            .lines()
            .map(|line| line.trim().to_lowercase())
            .filter(|line| !line.is_empty())
            .collect();
        app.shared.config().general.vrchat_match = patterns;
    }
}

fn vrc_files_and_tools(app: &mut LvrApp, ui: &mut Ui) {
    widgets::heading(ui, "VRChat & Domain Shield");
    ui.label(
        RichText::new(
            "Configure media blocking domain shields, community blocklist integration, \
             and access VRChat Proton prefix files.",
        )
        .size(13.0)
        .color(GREY),
    );
    ui.add_space(6.0);

    egui::Grid::new("settings-vrc-domain-block")
        .num_columns(2)
        .spacing([14.0, 12.0])
        .min_col_width(190.0)
        .show(ui, |ui| {
            ui.label("Load Community Blocklists");
            {
                let mut config = app.shared.config();
                if widgets::toggle(
                    ui,
                    &mut config.domain_block.load_community_blocklists,
                    "merge community video, image, and string domains into active shield",
                ) {
                    app.send(Command::ReloadDomainLists);
                    app.send(Command::SaveConfig);
                }
            }
            ui.end_row();

            ui.label("DNS Shield (Steam)");
            ui.horizontal(|ui| {
                let shield_active = app.status.steam_shield_active;
                let (status_text, color) = if shield_active {
                    ("Installed in Steam", GREEN)
                } else {
                    ("Not in Steam Launch Options", ORANGE)
                };

                let opts_summary = if app.status.steam_launch_options.is_empty() {
                    "No launch options set"
                } else {
                    &app.status.steam_launch_options
                };
                ui.label(RichText::new(status_text).size(12.0).color(color))
                    .on_hover_text(format!("Current Steam LaunchOptions:\n{opts_summary}"));

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
            ui.end_row();
        });

    let lists = crate::domain_block::active_domains();
    ui.add_space(6.0);
    ui.label(RichText::new("Domains summary:").strong());
    ui.add_space(3.0);
    render_domains_blocked_grid(ui, &lists, &app.status.block_state);
    ui.add_space(8.0);

    let prefix_opt = crate::domain_block::detect_vrc_prefix("");
    render_vrc_prefix_section(ui, prefix_opt.as_deref());
}

fn render_domains_blocked_grid(
    ui: &mut Ui,
    lists: &crate::domain_block::DomainLists,
    block_state: &crate::domain_block::BlockState,
) {
    let categories = lists.all_categories();
    let num_columns = 1 + categories.len() + 1;

    egui::Grid::new("settings-domains-blocked-grid")
        .striped(true)
        .num_columns(num_columns)
        .spacing([24.0, 6.0])
        .min_col_width(60.0)
        .show(ui, |ui| {
            // Header row
            ui.label(RichText::new("Status").strong());
            for cat in &categories {
                ui.label(RichText::new(cat.short_label()).strong());
            }
            ui.label(RichText::new("Total").strong());
            ui.end_row();

            // Row 1: Total Loaded / Available domains
            ui.label(RichText::new("Loaded").strong());
            for cat in &categories {
                ui.label(lists.count_for_category(cat).to_string());
            }
            ui.label(RichText::new(lists.total_count().to_string()).strong().color(BLUE));
            ui.end_row();

            // Row 2: Actually Blocked domains right now
            ui.label(RichText::new("Blocked").strong());
            let mut total_blocked = 0usize;
            for cat in &categories {
                let is_blocked = block_state.is_blocked(cat);
                let count = if is_blocked {
                    lists.count_for_category(cat)
                } else {
                    0
                };
                total_blocked += count;

                let (text, color) = if is_blocked {
                    (count.to_string(), super::widgets::RED)
                } else {
                    ("0".to_string(), GREY)
                };
                ui.label(RichText::new(text).color(color));
            }

            let total_color = if total_blocked > 0 { super::widgets::RED } else { GREY };
            ui.label(RichText::new(total_blocked.to_string()).strong().color(total_color));
            ui.end_row();
        });
}


fn render_vrc_prefix_section(ui: &mut Ui, prefix_opt: Option<&std::path::Path>) {
    // Open shield_rules.txt button (available regardless of prefix detection)
    let shield_rules_paths = crate::domain_block::dns_shield::shield_rules_paths();
    let shield_rules_file = shield_rules_paths.first().cloned()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/lvr_shield_rules.txt"));

    ui.horizontal_wrapped(|ui| {
        if widgets::row_button(ui, "📄 Open shield_rules.txt", Some(BLUE), 200.0).clicked() {
            if let Some(parent) = shield_rules_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if !shield_rules_file.exists() {
                let _ = std::fs::write(&shield_rules_file, "# LVR DNS Shield Rules (autogenerated)\n");
            }
            let _ = std::process::Command::new("xdg-open")
                .arg(&shield_rules_file)
                .spawn();
        }

        if let Some(prefix) = prefix_opt {
            let tools_dir = crate::domain_block::vrc_tools_dir(prefix);
            if widgets::row_button(ui, "🛠 Open Tools Folder", Some(GREEN), 180.0).clicked() {
                let _ = std::fs::create_dir_all(&tools_dir);
                let _ = std::process::Command::new("xdg-open")
                    .arg(&tools_dir)
                    .spawn();
            }
        }
    });

    if let Some(prefix) = prefix_opt {
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("Prefix: {}", prefix.display()))
                .size(11.0)
                .color(GREY),
        );
    } else {
        ui.add_space(4.0);
        ui.label(
            RichText::new("VRChat Proton prefix not detected automatically. Start VRChat once or check Steam compatdata.")
                .size(13.0)
                .color(ORANGE),
        );
    }
}

fn virtual_display(app: &mut LvrApp, ui: &mut Ui) {
    widgets::heading(ui, "Virtual Display");
    egui::Grid::new("settings-virtual-display")
        .num_columns(2)
        .spacing([14.0, 12.0])
        .min_col_width(190.0)
        .show(ui, |ui| {
            ui.label("Create on startup");
            {
                let mut config = app.shared.config();
                widgets::toggle(
                    ui,
                    &mut config.virtual_display.create_on_startup,
                    "automatically create a virtual display when lvr starts if no physical display is connected",
                );
            }
            ui.end_row();

            ui.label("Create on last display unplugged");
            {
                let mut config = app.shared.config();
                widgets::toggle(
                    ui,
                    &mut config.virtual_display.create_on_last_display_unplugged,
                    "automatically create a virtual display when the last monitor is unplugged",
                );
            }
            ui.end_row();

            ui.label("Debounce delay");
            {
                let mut config = app.shared.config();
                ui.add(
                    egui::DragValue::new(&mut config.virtual_display.debounce_secs)
                        .range(0..=60)
                        .speed(1.0)
                        .suffix(" s"),
                );
            }
            ui.end_row();

            ui.label("Resolution mode");
            {
                let mut config = app.shared.config();
                render_virtual_display_resolution(ui, &mut config);
            }
            ui.end_row();
        });
}

fn render_virtual_display_resolution(ui: &mut Ui, config: &mut crate::config::Config) {
    let current_raw = config.virtual_display.resolution.clone();
    let (mut current_res, mut current_hz) = if let Some((r, h)) = current_raw.split_once('@') {
        (r.to_string(), h.to_string())
    } else {
        (current_raw.clone(), "60".to_string())
    };

    let common_resolutions = [
        ("1280x720", "1280x720 (720p HD)"),
        ("1920x1080", "1920x1080 (1080p FHD)"),
        ("2560x1440", "2560x1440 (1440p QHD)"),
        ("3440x1440", "3440x1440 (UWQHD Ultrawide)"),
        ("3840x2160", "3840x2160 (4K UHD)"),
        ("5120x1440", "5120x1440 (Dual QHD 32:9)"),
        ("7680x4320", "7680x4320 (8K UHD)"),
        // VR headset-specific single-eye & combined panel targets
        ("1832x1920", "1832x1920 (Quest 2 native per-eye)"),
        ("2064x2208", "2064x2208 (Quest 3 native per-eye)"),
        ("4128x2208", "4128x2208 (Quest 3 combined panel)"),
        ("2448x2448", "2448x2448 (Vive Pro 2 native per-eye)"),
        ("2880x2720", "2880x2720 (Bigscreen Beyond per-eye)"),
        ("3552x3840", "3552x3840 (Apple Vision Pro per-eye)"),
        ("3840x3552", "3840x3552 (Somnium VR1 per-eye)"),
        ("5120x2160", "5120x2160 (5K2K Ultrawide)"),
    ];

    let refresh_rates = [
        "30", "45", "60", "72", "75", "80", "90", "100", "120", "144", "165", "180", "207", "240",
    ];

    let mut changed = false;

    ui.horizontal(|ui| {
        let res_label = common_resolutions
            .iter()
            .find(|(res, _)| *res == current_res)
            .map(|(_, desc)| *desc)
            .unwrap_or(&current_res);

        egui::ComboBox::from_id_salt("vd-resolution-select")
            .width(260.0)
            .selected_text(res_label)
            .show_ui(ui, |ui| {
                for (res, desc) in common_resolutions {
                    if ui.selectable_label(current_res == res, desc).clicked() {
                        current_res = res.to_string();
                        changed = true;
                    }
                }
            });

        let hz_label = format!("{current_hz} Hz");
        egui::ComboBox::from_id_salt("vd-refresh-select")
            .width(90.0)
            .selected_text(hz_label)
            .show_ui(ui, |ui| {
                for rate in refresh_rates {
                    let label = format!("{rate} Hz");
                    if ui.selectable_label(current_hz == rate, label).clicked() {
                        current_hz = rate.to_string();
                        changed = true;
                    }
                }
            });
    });

    if changed {
        config.virtual_display.resolution = format!("{current_res}@{current_hz}");
    }
}

fn about(app: &mut LvrApp, ui: &mut Ui) {
    widgets::heading(ui, "Config");
    let path = app.shared.config_path().display().to_string();
    ui.label(RichText::new(&path).size(13.0).color(GREY));
    ui.add_space(6.0);

    let width = ((ui.available_width() - 20.0) / 3.0).max(150.0);
    ui.horizontal_wrapped(|ui| {
        if widgets::big_button(ui, "Save now", Some(GREEN), width).clicked() {
            app.shared.send(Command::SaveConfig);
        }
        if widgets::big_button(ui, "Reload from disk", Some(BLUE), width).clicked() {
            reload(app);
        }
        if widgets::big_button(ui, "Open folder", None, width).clicked() {
            open_config_folder(app);
        }
    });

    ui.add_space(10.0);
    ui.label(
        RichText::new(format!(
            "{} {} — tray + GUI supervisor for WiVRn on Linux",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION")
        ))
        .size(13.0)
        .color(GREY),
    );
}

fn reload(app: &mut LvrApp) {
    let path = app.shared.config_path().clone();
    match crate::config::Config::load_or_create(&path) {
        Ok(config) => {
            *app.shared.config() = config.clone();
            app.saved_config = config;
            app.shared.info("Reloaded config from disk");
            app.shared.send(Command::Poke);
        }
        Err(err) => app
            .shared
            .error(format!("Reloading config failed: {err:#}")),
    }
}

fn open_config_folder(app: &LvrApp) {
    let Some(dir) = app.shared.config_path().parent().map(|p| p.to_path_buf()) else {
        return;
    };
    let Some(opener) = procs::which("xdg-open") else {
        app.shared.warn("xdg-open is not available");
        return;
    };
    match std::process::Command::new(opener)
        .arg(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        // Reap it off-thread so the file manager never lingers as a zombie.
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(err) => app
            .shared
            .warn(format!("Could not open {}: {err}", dir.display())),
    }
}
