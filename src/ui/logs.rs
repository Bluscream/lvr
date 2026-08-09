//! Logs tab.

use egui::{Color32, RichText, Ui};

use super::LvrApp;
use super::widgets::{self, BLUE, GREY, ORANGE, RED};
use crate::state::{LogLevel, LogLine};

const LEVELS: [LogLevel; 4] = [
    LogLevel::Debug,
    LogLevel::Info,
    LogLevel::Warn,
    LogLevel::Error,
];

pub fn show(app: &mut LvrApp, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        widgets::heading(ui, "Logs");
        for (index, level) in LEVELS.iter().enumerate() {
            ui.checkbox(&mut app.log_levels[index], level.label());
        }
        ui.checkbox(&mut app.log_wrap, "wrap");
        ui.add(
            egui::TextEdit::singleline(&mut app.log_filter)
                .desired_width(200.0)
                .hint_text("filter…"),
        );
        if widgets::row_button(ui, "Copy", Some(BLUE), 90.0).clicked() {
            let text = visible_lines(app)
                .iter()
                .map(LogLine::formatted)
                .collect::<Vec<_>>()
                .join("\n");
            ui.ctx().copy_text(text);
        }
        if widgets::row_button(ui, "Clear", Some(ORANGE), 90.0).clicked() {
            app.shared.clear_logs();
        }
    });
    ui.add_space(6.0);

    let lines = visible_lines(app);
    let wrap = app.log_wrap;
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if lines.is_empty() {
                ui.label(RichText::new("Nothing logged yet.").color(GREY));
                return;
            }
            for line in &lines {
                let text = RichText::new(line.formatted())
                    .monospace()
                    .color(color_for(line.level));
                let label = egui::Label::new(text);
                ui.add(if wrap {
                    label
                } else {
                    label.wrap_mode(egui::TextWrapMode::Extend)
                });
            }
        });
}

fn visible_lines(app: &LvrApp) -> Vec<LogLine> {
    let filter = app.log_filter.trim().to_lowercase();
    let enabled: Vec<LogLevel> = LEVELS
        .iter()
        .enumerate()
        .filter(|(index, _)| app.log_levels[*index])
        .map(|(_, level)| *level)
        .collect();
    app.shared
        .logs()
        .into_iter()
        .filter(|line| enabled.contains(&line.level))
        .filter(|line| filter.is_empty() || line.message.to_lowercase().contains(&filter))
        .collect()
}

fn color_for(level: LogLevel) -> Color32 {
    match level {
        LogLevel::Debug => GREY,
        LogLevel::Info => Color32::from_rgb(0xd0, 0xd4, 0xda),
        LogLevel::Warn => ORANGE,
        LogLevel::Error => RED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_level_has_a_distinct_colour() {
        let colors: Vec<Color32> = LEVELS.iter().map(|l| color_for(*l)).collect();
        for (i, a) in colors.iter().enumerate() {
            for b in colors.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn levels_are_ordered_from_quiet_to_loud() {
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
    }
}
