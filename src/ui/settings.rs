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

            ui.label("Stop grace (SIGTERM→SIGKILL)");
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
        });
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
