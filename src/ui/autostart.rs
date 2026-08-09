//! Autostart tab: the table of managed apps and the entry editor.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};

use super::widgets::{self, BLUE, GREEN, GREY, ORANGE, RED};
use super::{EntryEditor, GraceMode, LvrApp};
use crate::config::{AutostartEntry, Trigger};
use crate::state::Command;

const ON_WIDTH: f32 = 36.0;
const NAME_WIDTH: f32 = 120.0;
const TRIGGER_WIDTH: f32 = 140.0;
const STOPS_WIDTH: f32 = 110.0;
/// Status never shrinks below this; past that point the table scrolls instead.
const MIN_STATUS_WIDTH: f32 = 90.0;
/// Slack for the cell's own padding and the scrollbar.
const CELL_PADDING: f32 = 16.0;
const STATUS_TEXT_SIZE: f32 = 11.0;
/// Room for the status dot that sits left of the wrapped text.
const STATUS_DOT_WIDTH: f32 = 20.0;
/// Floor for rows: larger to fit name and console subtext comfortably.
const MIN_ROW_HEIGHT: f32 = 36.0;

/// Compact widths of the action buttons, in the order they are drawn.
const ACTION_BUTTON_WIDTHS: [f32; 5] = [48.0, 44.0, 30.0, 40.0, 52.0];

/// Exact width the action column needs for every button plus the gaps.
fn actions_column_width(item_spacing: f32) -> f32 {
    let buttons: f32 = ACTION_BUTTON_WIDTHS.iter().sum();
    let gaps = item_spacing * (ACTION_BUTTON_WIDTHS.len() - 1) as f32;
    buttons + gaps
}

/// How tall a row must be for its status text to fit at `status_width`.
fn row_height(ui: &egui::Ui, status_text: &str, status_width: f32) -> f32 {
    let wrap_width = (status_width - STATUS_DOT_WIDTH).max(40.0);
    let galley = ui.painter().layout(
        status_text.to_owned(),
        egui::FontId::proportional(STATUS_TEXT_SIZE),
        egui::Color32::PLACEHOLDER,
        wrap_width,
    );
    (galley.size().y + 8.0).max(MIN_ROW_HEIGHT)
}

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

    let spacing = ui.spacing().item_spacing.x;
    let actions_width = actions_column_width(spacing);
    // Everything except Status is fixed, so Status is what has to give: it
    // takes the leftover width and wraps onto as many lines as it needs, and
    // the row grows to match. That keeps the action buttons on screen no
    // matter how long a pid list gets.
    let status_width = (ui.available_width()
        - (ON_WIDTH + NAME_WIDTH + TRIGGER_WIDTH + STOPS_WIDTH + actions_width)
        - spacing * 5.0
        - CELL_PADDING)
        .max(MIN_STATUS_WIDTH);

    let show_debug = app.shared.config().general.show_debug_info;
    let row_heights: Vec<f32> = entries
        .iter()
        .map(|entry| {
            let status = app.status.entry(&entry.id).cloned().unwrap_or_default();
            let text = super::dashboard::detail_line(&status, show_debug);
            row_height(ui, &text, status_width)
        })
        .collect();

    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(ON_WIDTH))
        .column(Column::exact(NAME_WIDTH))
        .column(Column::exact(TRIGGER_WIDTH))
        .column(Column::exact(STOPS_WIDTH))
        .column(Column::remainder().at_least(MIN_STATUS_WIDTH))
        .column(Column::exact(actions_width))
        .header(24.0, |mut header| {
            for title in ["On", "Name", "Trigger", "Stops", "Status", ""] {
                header.col(|ui| {
                    ui.label(RichText::new(title).size(12.0).strong().color(GREY));
                });
            }
        })
        .body(|mut body| {
            for (index, entry) in entries.iter().enumerate() {
                let status = app.status.entry(&entry.id).cloned().unwrap_or_default();
                body.row(row_heights[index], |mut row| {
                    row.col(|ui| {
                        let mut enabled = entry.enabled;
                        if ui.add(egui::Checkbox::without_text(&mut enabled)).changed() {
                            pending = Some(Pending::Toggle(entry.id.clone(), enabled));
                        }
                    });
                    row.col(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(entry.name_or_id()).size(13.0).strong());
                            if entry.console {
                                ui.label(RichText::new("console").size(10.0).color(GREY));
                            }
                        });
                    });
                    row.col(|ui| {
                        let color = if status.trigger_active { GREEN } else { GREY };
                        ui.label(
                            RichText::new(entry.trigger.to_string())
                                .size(12.0)
                                .color(color),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(widgets::format_grace(entry.grace_secs))
                                .size(12.0)
                                .color(if entry.keeps_running() { BLUE } else { GREY }),
                        );
                    });
                    row.col(|ui| {
                        let color = widgets::on_off(status.running);
                        ui.set_max_width(status_width);
                        ui.horizontal_top(|ui| {
                            ui.label(RichText::new("⏺").size(13.0).color(color));
                            ui.add(
                                egui::Label::new(
                                    RichText::new(super::dashboard::detail_line(&status, show_debug))
                                        .size(STATUS_TEXT_SIZE)
                                        .color(GREY),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                        });
                    });
                    row.col(|ui| {
                        if status.running {
                            if widgets::compact_button(ui, "Stop", Some(ORANGE), 48.0).clicked() {
                                pending = Some(Pending::Stop(entry.id.clone()));
                            }
                        } else if widgets::compact_button(ui, "Start", Some(GREEN), 48.0).clicked() {
                            pending = Some(Pending::Start(entry.id.clone()));
                        }
                        if widgets::compact_button(ui, "Edit", None, 44.0).clicked() {
                            pending = Some(Pending::Edit(entry.id.clone()));
                        }
                        if ui
                            .add_enabled(
                                index > 0,
                                egui::Button::new(RichText::new("Up").size(11.0))
                                    .min_size(egui::vec2(30.0, 17.0)),
                            )
                            .clicked()
                        {
                            pending = Some(Pending::MoveUp(index));
                        }
                        if ui
                            .add_enabled(
                                index + 1 < entries.len(),
                                egui::Button::new(RichText::new("Down").size(11.0))
                                    .min_size(egui::vec2(40.0, 17.0)),
                            )
                            .clicked()
                        {
                            pending = Some(Pending::MoveDown(index));
                        }
                        if widgets::compact_button(ui, "Delete", Some(RED), 52.0).clicked() {
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
    use super::{ACTION_BUTTON_WIDTHS, actions_column_width};
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
    fn the_action_column_is_wide_enough_for_every_button() {
        // The regression this replaces: the column was a hard-coded 330.0 while
        // the buttons needed 368.0, so "Delete" was clipped off the window edge.
        let spacing = 10.0;
        let width = actions_column_width(spacing);
        let needed: f32 = ACTION_BUTTON_WIDTHS.iter().sum::<f32>()
            + spacing * (ACTION_BUTTON_WIDTHS.len() - 1) as f32;
        assert!(
            width >= needed,
            "action column {width} cannot hold {needed} of buttons"
        );
        assert_eq!(width, 254.0);
    }

    #[test]
    fn a_wider_gap_between_buttons_widens_the_column() {
        assert!(actions_column_width(20.0) > actions_column_width(10.0));
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
