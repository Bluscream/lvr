//! Autostart tab: the table of managed apps and the entry editor.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};

use super::widgets::{self, BLUE, GREEN, GREY, ORANGE, RED};
use super::{EntryEditor, GraceMode, LvrApp};
use crate::config::{AutostartEntry, Trigger};
use crate::state::Command;

/// Pending structural change, applied after the table is drawn so we never
/// mutate the list we are iterating.
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
            if widgets::row_button(ui, "+ Add app", Some(GREEN), 150.0).clicked() {
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

    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(46.0))
        .column(Column::initial(190.0).at_least(120.0))
        .column(Column::initial(170.0).at_least(120.0))
        .column(Column::initial(130.0).at_least(90.0))
        .column(Column::remainder().at_least(160.0))
        .column(Column::exact(330.0))
        .header(28.0, |mut header| {
            for title in ["On", "Name", "Trigger", "Stops", "Status", ""] {
                header.col(|ui| {
                    ui.label(RichText::new(title).size(13.0).strong().color(GREY));
                });
            }
        })
        .body(|mut body| {
            for (index, entry) in entries.iter().enumerate() {
                let status = app.status.entry(&entry.id).cloned().unwrap_or_default();
                body.row(52.0, |mut row| {
                    row.col(|ui| {
                        let mut enabled = entry.enabled;
                        if ui.add(egui::Checkbox::without_text(&mut enabled)).changed() {
                            pending = Some(Pending::Toggle(entry.id.clone(), enabled));
                        }
                    });
                    row.col(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(entry.name_or_id()).size(15.0).strong());
                            if entry.console {
                                ui.label(RichText::new("console").size(11.0).color(GREY));
                            }
                        });
                    });
                    row.col(|ui| {
                        let color = if status.trigger_active { GREEN } else { GREY };
                        ui.label(
                            RichText::new(entry.trigger.to_string())
                                .size(13.0)
                                .color(color),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(widgets::format_grace(entry.grace_secs))
                                .size(13.0)
                                .color(if entry.keeps_running() { BLUE } else { GREY }),
                        );
                    });
                    row.col(|ui| {
                        let color = widgets::on_off(status.running);
                        ui.label(RichText::new("⏺").size(16.0).color(color));
                        ui.label(
                            RichText::new(super::dashboard::detail_line(&status))
                                .size(12.0)
                                .color(GREY),
                        );
                    });
                    row.col(|ui| {
                        if status.running {
                            if widgets::row_button(ui, "Stop", Some(ORANGE), 74.0).clicked() {
                                pending = Some(Pending::Stop(entry.id.clone()));
                            }
                        } else if widgets::row_button(ui, "Start", Some(GREEN), 74.0).clicked() {
                            pending = Some(Pending::Start(entry.id.clone()));
                        }
                        if widgets::row_button(ui, "Edit", None, 74.0).clicked() {
                            pending = Some(Pending::Edit(entry.id.clone()));
                        }
                        if ui
                            .add_enabled(
                                index > 0,
                                egui::Button::new("Up").min_size(egui::vec2(44.0, 38.0)),
                            )
                            .clicked()
                        {
                            pending = Some(Pending::MoveUp(index));
                        }
                        if ui
                            .add_enabled(
                                index + 1 < entries.len(),
                                egui::Button::new("Down").min_size(egui::vec2(54.0, 38.0)),
                            )
                            .clicked()
                        {
                            pending = Some(Pending::MoveDown(index));
                        }
                        if widgets::row_button(ui, "Delete", Some(RED), 82.0).clicked() {
                            pending = Some(Pending::Delete(entry.id.clone()));
                        }
                    });
                });
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
            let name = {
                let mut config = app.shared.config();
                let name = config.entry(&id).map(|e| e.name_or_id().to_string());
                config.autostart.retain(|e| e.id != id);
                name
            };
            if let Some(name) = name {
                app.shared.info(format!("Removed autostart entry {name}"));
            }
        }
        Pending::MoveUp(index) => {
            let mut config = app.shared.config();
            if index > 0 && index < config.autostart.len() {
                config.autostart.swap(index - 1, index);
            }
        }
        Pending::MoveDown(index) => {
            let mut config = app.shared.config();
            if index + 1 < config.autostart.len() {
                config.autostart.swap(index, index + 1);
            }
        }
        Pending::Start(id) => app.shared.send(Command::StartEntry(id)),
        Pending::Stop(id) => app.shared.send(Command::StopEntry(id)),
        Pending::Toggle(id, enabled) => {
            {
                let mut config = app.shared.config();
                if let Some(entry) = config.autostart.iter_mut().find(|e| e.id == id) {
                    entry.enabled = enabled;
                }
            }
            app.shared.send(Command::Poke);
        }
    }
}

/// The body of the entry editor window.
pub fn editor_body(ui: &mut Ui, editor: &mut EntryEditor) {
    egui::Grid::new("entry-editor")
        .num_columns(2)
        .spacing([14.0, 12.0])
        .min_col_width(120.0)
        .show(ui, |ui| {
            ui.label("Name");
            ui.add(
                egui::TextEdit::singleline(&mut editor.entry.name)
                    .desired_width(f32::INFINITY)
                    .hint_text("VRCVideoCacher"),
            );
            ui.end_row();

            ui.label("Enabled");
            ui.checkbox(&mut editor.entry.enabled, "auto-start this app");
            ui.end_row();

            ui.label("Trigger");
            ui.vertical(|ui| {
                egui::ComboBox::from_id_salt("trigger-kind")
                    .width(260.0)
                    .selected_text(Trigger::KINDS[editor.trigger_kind.min(4)])
                    .show_ui(ui, |ui| {
                        for (index, label) in Trigger::KINDS.iter().enumerate() {
                            ui.selectable_value(&mut editor.trigger_kind, index, *label);
                        }
                    });
                if editor.trigger_kind == 3 {
                    ui.add(
                        egui::TextEdit::singleline(&mut editor.trigger_process)
                            .desired_width(260.0)
                            .hint_text("part of the process command line"),
                    );
                }
            });
            ui.end_row();

            ui.label("Command");
            ui.add(
                egui::TextEdit::singleline(&mut editor.entry.command)
                    .desired_width(f32::INFINITY)
                    .hint_text("/path/to/app --flag"),
            );
            ui.end_row();

            ui.label("Working dir");
            ui.add(
                egui::TextEdit::singleline(&mut editor.entry.working_dir)
                    .desired_width(f32::INFINITY)
                    .hint_text("optional"),
            );
            ui.end_row();

            ui.label("Console");
            ui.checkbox(
                &mut editor.entry.console,
                "run inside a terminal window (keeps its output visible)",
            );
            ui.end_row();

            ui.label("Shell");
            ui.checkbox(
                &mut editor.entry.use_shell,
                "run through `sh -c` (needed for pipes, && and globs)",
            );
            ui.end_row();

            ui.label("Stop when trigger ends");
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut editor.grace_mode,
                        GraceMode::KeepRunning,
                        "keep running",
                    );
                    ui.selectable_value(
                        &mut editor.grace_mode,
                        GraceMode::Immediately,
                        "immediately",
                    );
                    ui.selectable_value(&mut editor.grace_mode, GraceMode::After, "after…");
                });
                if editor.grace_mode == GraceMode::After {
                    ui.add(
                        egui::DragValue::new(&mut editor.grace_secs)
                            .range(1..=86_400)
                            .speed(5.0)
                            .suffix(" s"),
                    );
                }
            });
            ui.end_row();

            ui.label("Start delay");
            ui.add(
                egui::DragValue::new(&mut editor.entry.start_delay_secs)
                    .range(0..=3600)
                    .speed(1.0)
                    .suffix(" s"),
            );
            ui.end_row();

            ui.label("Restart on exit");
            ui.checkbox(
                &mut editor.entry.restart_on_exit,
                "relaunch if it quits while the trigger is still active",
            );
            ui.end_row();

            ui.label("Match patterns");
            ui.vertical(|ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut editor.patterns_text)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .hint_text("one per line; matched against process command lines"),
                );
                ui.label(
                    RichText::new(
                        "Used to detect whether the app is already running and to stop it. \
                         Leave empty to use the executable's file name.",
                    )
                    .size(12.0)
                    .color(GREY),
                );
            });
            ui.end_row();

            ui.label("Stop command");
            ui.add(
                egui::TextEdit::singleline(&mut editor.entry.stop_command)
                    .desired_width(f32::INFINITY)
                    .hint_text("optional, e.g. flatpak kill dev.slimevr.SlimeVR"),
            );
            ui.end_row();

            ui.label("Stop-all");
            ui.checkbox(
                &mut editor.entry.include_in_stop_all,
                "include in “Stop everything VR”",
            );
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
