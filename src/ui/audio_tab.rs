//! Audio tab: which devices VR and desktop mean, and manual routing.

use egui::{RichText, Ui};

use super::LvrApp;
use super::widgets::{self, BLUE, GREEN, GREY};
use crate::state::{AudioDevice, Command};

const REMEMBER_LAST: &str = "(remember what was active)";

pub fn show(app: &mut LvrApp, ui: &mut Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        widgets::heading(ui, "Audio routing");
        ui.label(
            RichText::new(
                "When a headset connects to WiVRn, the default output and microphone switch to \
                 the VR devices — and switch back when it disconnects.",
            )
            .size(13.0)
            .color(GREY),
        );
        ui.add_space(8.0);

        current_state(app, ui);
        ui.add_space(10.0);
        settings(app, ui);
        ui.add_space(10.0);
        manual_controls(app, ui);
    });
}

fn current_state(app: &LvrApp, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        widgets::pill(
            ui,
            "Routing",
            if app.status.audio_on_vr {
                "VR devices"
            } else {
                "Desktop devices"
            },
            if app.status.audio_on_vr { GREEN } else { BLUE },
        );
        widgets::pill(
            ui,
            "Default output",
            &app.status.friendly_sink_label(),
            GREY,
        );
        widgets::pill(
            ui,
            "Default input",
            &app.status.friendly_source_label(),
            GREY,
        );
    });
}

fn settings(app: &mut LvrApp, ui: &mut Ui) {
    let sinks = app.status.sinks.clone();
    let sources = app.status.sources.clone();
    let mut changed = false;

    egui::Grid::new("audio-settings")
        .num_columns(2)
        .spacing([14.0, 12.0])
        .min_col_width(150.0)
        .show(ui, |ui| {
            ui.label("Automatic switching");
            {
                let mut config = app.shared.config();
                changed |= widgets::toggle(
                    ui,
                    &mut config.audio.enabled,
                    "follow the headset connection",
                );
            }
            ui.end_row();

            ui.label("Move existing streams");
            {
                let mut config = app.shared.config();
                changed |= widgets::toggle(
                    ui,
                    &mut config.audio.move_streams,
                    "also move apps that are already playing/recording",
                );
            }
            ui.end_row();

            ui.label("VR output");
            changed |= device_combo(ui, app, "vr-sink", &sinks, Field::VrSink, false);
            ui.end_row();

            ui.label("VR microphone");
            changed |= device_combo(ui, app, "vr-source", &sources, Field::VrSource, false);
            ui.end_row();

            ui.label("Desktop output");
            changed |= device_combo(ui, app, "desk-sink", &sinks, Field::DesktopSink, true);
            ui.end_row();

            ui.label("Desktop microphone");
            changed |= device_combo(ui, app, "desk-source", &sources, Field::DesktopSource, true);
            ui.end_row();
        });

    if changed {
        app.shared.send(Command::Poke);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    VrSink,
    VrSource,
    DesktopSink,
    DesktopSource,
}

fn read(app: &LvrApp, field: Field) -> String {
    let config = app.shared.config();
    match field {
        Field::VrSink => config.audio.vr_sink.clone(),
        Field::VrSource => config.audio.vr_source.clone(),
        Field::DesktopSink => config.audio.desktop_sink.clone(),
        Field::DesktopSource => config.audio.desktop_source.clone(),
    }
}

fn write(app: &LvrApp, field: Field, value: String) {
    let mut config = app.shared.config();
    match field {
        Field::VrSink => config.audio.vr_sink = value,
        Field::VrSource => config.audio.vr_source = value,
        Field::DesktopSink => config.audio.desktop_sink = value,
        Field::DesktopSource => config.audio.desktop_source = value,
    }
}

/// Device picker plus a free-text field, because a device can be configured
/// while it is not currently present (the WiVRn nodes only exist while the
/// server runs).
fn device_combo(
    ui: &mut Ui,
    app: &LvrApp,
    id: &str,
    devices: &[AudioDevice],
    field: Field,
    allow_empty: bool,
) -> bool {
    let mut current = read(app, field);
    let mut changed = false;

    ui.vertical(|ui| {
        let selected_label = if current.is_empty() {
            REMEMBER_LAST.to_string()
        } else {
            devices
                .iter()
                .find(|d| d.name == current)
                .map(|d| d.label().to_string())
                .unwrap_or_else(|| current.clone())
        };
        egui::ComboBox::from_id_salt(id)
            .width(360.0)
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                if allow_empty
                    && ui
                        .selectable_label(current.is_empty(), REMEMBER_LAST)
                        .clicked()
                {
                    current = String::new();
                    changed = true;
                }
                for device in devices {
                    if ui
                        .selectable_label(current == device.name, device.label())
                        .clicked()
                    {
                        current = device.name.clone();
                        changed = true;
                    }
                }
                if devices.is_empty() {
                    ui.label(RichText::new("no devices found").color(GREY));
                }
            });
        if ui
            .add(
                egui::TextEdit::singleline(&mut current)
                    .desired_width(360.0)
                    .hint_text("PipeWire node name"),
            )
            .changed()
        {
            changed = true;
        }
    });

    if changed {
        write(app, field, current);
    }
    changed
}

fn manual_controls(app: &mut LvrApp, ui: &mut Ui) {
    widgets::heading(ui, "Manual");
    let width = ((ui.available_width() - 20.0) / 3.0).max(150.0);
    ui.horizontal_wrapped(|ui| {
        if widgets::big_button(ui, "Route to VR", Some(GREEN), width).clicked() {
            app.shared.send(Command::SetAudioVr(true));
        }
        if widgets::big_button(ui, "Route to desktop", Some(BLUE), width).clicked() {
            app.shared.send(Command::SetAudioVr(false));
        }
        if widgets::big_button(ui, "Refresh devices", None, width).clicked() {
            app.shared.send(Command::RefreshAudioDevices);
        }
    });
}

#[cfg(test)]
mod tests {
    use crate::state::friendly_label;

    #[test]
    fn friendly_label_handles_empty_and_wivrn_names() {
        assert_eq!(friendly_label("", &[]), "unknown");
        assert_eq!(friendly_label("wivrn.sink", &[]), "WiVRn Sink");
    }
}
