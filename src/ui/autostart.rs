//! Autostart tab: list of managed apps and entry editor.

use egui::{RichText, Ui};

use super::widgets::{self, BLUE, GREEN, GREY, ORANGE, RED};
use super::{EntryEditor, GraceMode, LvrApp};
use crate::config::{AutostartEntry, Trigger};
use crate::state::Command;

/// Pending structural change, applied after drawing so we never mutate the
/// list while iterating.
enum Pending {
    Edit(String),
    Delete(String),
    MoveUp(usize),
    MoveDown(usize),
    Start(String),
    Stop(String),
    Toggle(String, bool),
}

pub fn show(app: &mut LvrApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        widgets::heading(ui, "Autostart");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::row_button(ui, "+ Add app", Some(GREEN), 130.0).clicked() {
                app.editor = Some(EntryEditor::new(
                    AutostartEntry {
                        name: "New app".into(),
                        trigger: Trigger::Vrchat,
                        grace_secs: 120,
                        ..Default::default()
                    },
                    true,
                ));
            }
        });
    });
    ui.label(
        RichText::new(
            "Apps listed here start automatically when their trigger appears, and stop again \
             once the grace period runs out. Grace \"keep running\" never stops them.",
        )
        .size(13.0)
        .color(GREY),
    );
    ui.add_space(8.0);

    let entries = app.shared.config_snapshot().autostart;
    if entries.is_empty() {
        ui.label(RichText::new("No entries yet. Use “+ Add app”.").color(GREY));
        return;
    }

    let mut pending: Option<Pending> = None;
    let show_debug = app.shared.config().general.show_debug_info;

    egui::ScrollArea::vertical().show(ui, |ui| {
        for (index, entry) in entries.iter().enumerate() {
            let status = app.status.entry(&entry.id).cloned().unwrap_or_default();

            egui::Frame::new()
                .fill(egui::Color32::from_black_alpha(35))
                .stroke((1.0, egui::Color32::from_white_alpha(15)))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Checkbox
                        let mut enabled = entry.enabled;
                        if ui.add(egui::Checkbox::without_text(&mut enabled)).changed() {
                            pending = Some(Pending::Toggle(entry.id.clone(), enabled));
                        }

                        // App Name & Subtext
                        ui.vertical(|ui| {
                            ui.label(RichText::new(entry.name_or_id()).size(14.0).strong());
                            if entry.console {
                                ui.label(RichText::new("console").size(10.0).color(GREY));
                            }
                        });

                        ui.add_space(10.0);

                        // Trigger
                        let trig_color = if status.trigger_active { GREEN } else { GREY };
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Trigger").size(10.0).color(GREY));
                            ui.label(
                                RichText::new(entry.trigger.to_string())
                                    .size(12.0)
                                    .color(trig_color),
                            );
                        });

                        ui.add_space(10.0);

                        // Stops / Grace
                        let grace_color = if entry.keeps_running() { BLUE } else { GREY };
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Stops").size(10.0).color(GREY));
                            ui.label(
                                RichText::new(widgets::format_grace(entry.grace_secs))
                                    .size(12.0)
                                    .color(grace_color),
                            );
                        });

                        ui.add_space(10.0);

                        // Status
                        let status_color = widgets::on_off(status.running);
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Status").size(10.0).color(GREY));
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("⏺").size(12.0).color(status_color));
                                ui.label(
                                    RichText::new(super::dashboard::detail_line(&status, show_debug))
                                        .size(11.0)
                                        .color(GREY),
                                );
                            });
                        });

                        // Action Buttons: laid out right-to-left so Delete is anchored to the right margin
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::compact_button(ui, "Delete", Some(RED), 0.0).clicked() {
                                pending = Some(Pending::Delete(entry.id.clone()));
                            }
                            if ui
                                .add_enabled(
                                    index + 1 < entries.len(),
                                    egui::Button::new(RichText::new("Down").size(11.0))
                                        .corner_radius(egui::CornerRadius::same(4)),
                                )
                                .clicked()
                            {
                                pending = Some(Pending::MoveDown(index));
                            }
                            if ui
                                .add_enabled(
                                    index > 0,
                                    egui::Button::new(RichText::new("Up").size(11.0))
                                        .corner_radius(egui::CornerRadius::same(4)),
                                )
                                .clicked()
                            {
                                pending = Some(Pending::MoveUp(index));
                            }
                            if widgets::compact_button(ui, "Edit", None, 0.0).clicked() {
                                pending = Some(Pending::Edit(entry.id.clone()));
                            }
                            if status.running {
                                if widgets::compact_button(ui, "Stop", Some(ORANGE), 0.0).clicked() {
                                    pending = Some(Pending::Stop(entry.id.clone()));
                                }
                            } else if widgets::compact_button(ui, "Start", Some(GREEN), 0.0).clicked() {
                                pending = Some(Pending::Start(entry.id.clone()));
                            }
                        });
                    });
                });
            ui.add_space(4.0);
        }
    });

    if let Some(action) = pending {
        apply(app, action);
    }
}

fn apply(app: &mut LvrApp, action: Pending) {
    match action {
        Pending::Edit(id) => {
            if let Some(entry) = app.shared.config().entry(&id).cloned() {
                app.editor = Some(EntryEditor::new(entry, false));
            }
        }
        Pending::Delete(id) => {
            let mut config = app.shared.config();
            config.autostart.retain(|e| e.id != id);
            app.shared.info(format!("Deleted autostart entry `{id}`"));
            app.shared.send(Command::Poke);
        }
        Pending::MoveUp(index) => {
            if index > 0 {
                let mut config = app.shared.config();
                config.autostart.swap(index, index - 1);
                app.shared.send(Command::Poke);
            }
        }
        Pending::MoveDown(index) => {
            let mut config = app.shared.config();
            if index + 1 < config.autostart.len() {
                config.autostart.swap(index, index + 1);
                app.shared.send(Command::Poke);
            }
        }
        Pending::Start(id) => {
            app.shared.send(Command::StartEntry(id));
        }
        Pending::Stop(id) => {
            app.shared.send(Command::StopEntry(id));
        }
        Pending::Toggle(id, enabled) => {
            let mut config = app.shared.config();
            if let Some(entry) = config.autostart.iter_mut().find(|e| e.id == id) {
                entry.enabled = enabled;
                app.shared.send(Command::Poke);
            }
        }
    }
}

pub fn editor_body(ui: &mut Ui, editor: &mut EntryEditor) {
    ui.heading(if editor.is_new {
        "New autostart entry"
    } else {
        "Edit autostart entry"
    });
    ui.add_space(4.0);

    egui::Grid::new("entry-editor")
        .num_columns(2)
        .spacing([14.0, 10.0])
        .min_col_width(140.0)
        .show(ui, |ui| {
            ui.label("Name");
            ui.add(
                egui::TextEdit::singleline(&mut editor.entry.name)
                    .desired_width(360.0)
                    .hint_text("Display name"),
            );
            ui.end_row();

            ui.label("Enabled");
            ui.checkbox(&mut editor.entry.enabled, "start when triggered");
            ui.end_row();

            ui.label("Command");
            ui.vertical(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut editor.entry.command)
                        .desired_width(360.0)
                        .hint_text("/usr/bin/some-app --flag"),
                );
                ui.horizontal(|ui| {
                    ui.checkbox(&mut editor.entry.use_shell, "Run via sh -c");
                    ui.checkbox(&mut editor.entry.console, "Run in terminal");
                });
            });
            ui.end_row();

            ui.label("Working directory");
            ui.add(
                egui::TextEdit::singleline(&mut editor.entry.working_dir)
                    .desired_width(360.0)
                    .hint_text("Default: current directory"),
            );
            ui.end_row();

            ui.label("Start trigger");
            ui.vertical(|ui| {
                let kinds = ["WiVRn running", "Headset connected", "VRChat running", "Custom process", "Manual only"];
                egui::ComboBox::from_id_salt("editor-trigger-kind")
                    .selected_text(kinds.get(editor.trigger_kind).copied().unwrap_or("Manual only"))
                    .show_ui(ui, |ui| {
                        for (index, kind) in kinds.iter().enumerate() {
                            if ui.selectable_label(editor.trigger_kind == index, *kind).clicked() {
                                editor.trigger_kind = index;
                            }
                        }
                    });
                if editor.trigger_kind == 3 {
                    ui.add(
                        egui::TextEdit::singleline(&mut editor.trigger_process)
                            .desired_width(360.0)
                            .hint_text("executable substring, e.g. OBS.exe"),
                    );
                }
            });
            ui.end_row();

            ui.label("Start delay");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut editor.entry.start_delay_secs)
                        .range(0..=3600)
                        .suffix(" seconds"),
                );
                ui.label(RichText::new("wait this long after trigger appears").size(12.0).color(GREY));
            });
            ui.end_row();

            ui.label("Stop behavior");
            ui.vertical(|ui| {
                ui.radio_value(&mut editor.grace_mode, GraceMode::After, "Stop after grace period");
                if editor.grace_mode == GraceMode::After {
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);
                        ui.add(
                            egui::DragValue::new(&mut editor.grace_secs)
                                .range(1..=86400)
                                .suffix(" seconds"),
                        );
                    });
                }
                ui.radio_value(&mut editor.grace_mode, GraceMode::Immediately, "Stop immediately when trigger disappears");
                ui.radio_value(&mut editor.grace_mode, GraceMode::KeepRunning, "Keep running (never stops automatically)");
            });
            ui.end_row();

            ui.label("Match patterns");
            ui.vertical(|ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut editor.patterns_text)
                        .desired_rows(3)
                        .desired_width(360.0)
                        .font(egui::TextStyle::Monospace),
                );
                ui.label(
                    RichText::new(
                        "Substrings of process cmdlines used to detect if this app is running. \
                         Defaults to executable name if blank.",
                    )
                    .size(12.0)
                    .color(GREY),
                );
            });
            ui.end_row();
        });
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::state::Shared;
    use std::path::PathBuf;

    fn app_shared() -> Shared {
        Shared::new(
            Config::default(),
            PathBuf::from("/tmp/lvr-autostart-test.toml"),
        )
        .0
    }

    #[test]
    fn move_up_and_down_reorder_entries() {
        let shared = app_shared();
        let first = shared.config().autostart[0].id.clone();
        let second = shared.config().autostart[1].id.clone();

        {
            let mut config = shared.config();
            config.autostart.swap(0, 1);
        }
        assert_eq!(shared.config().autostart[0].id, second);
        assert_eq!(shared.config().autostart[1].id, first);
    }

    #[test]
    fn delete_removes_only_the_named_entry() {
        let shared = app_shared();
        let before = shared.config().autostart.len();
        let id = shared.config().autostart[2].id.clone();
        {
            let mut config = shared.config();
            config.autostart.retain(|e| e.id != id);
        }
        assert_eq!(shared.config().autostart.len(), before - 1);
        assert!(shared.config().entry(&id).is_none());
    }
}
