//! Shared widgets, sized for VR controllers and couch distance.

use egui::{Color32, CornerRadius, Response, RichText, Ui, Vec2};

/// Height of the primary action buttons. Deliberately chunky: these get hit
/// with a laser pointer from a Quest controller.
pub const BIG_BUTTON_HEIGHT: f32 = 56.0;
/// Height of secondary (row-level) buttons.
pub const ROW_BUTTON_HEIGHT: f32 = 38.0;

pub const GREEN: Color32 = Color32::from_rgb(0x3e, 0xcf, 0x6d);
pub const BLUE: Color32 = Color32::from_rgb(0x4c, 0x8d, 0xff);
pub const RED: Color32 = Color32::from_rgb(0xe0, 0x53, 0x53);
pub const ORANGE: Color32 = Color32::from_rgb(0xf0, 0x9d, 0x3c);
pub const GREY: Color32 = Color32::from_rgb(0x8a, 0x90, 0x99);

/// A large primary button. `width` of `0.0` means "fill the available width".
pub fn big_button(ui: &mut Ui, label: &str, tint: Option<Color32>, width: f32) -> Response {
    let width = if width > 0.0 {
        width
    } else {
        ui.available_width()
    };
    let mut button = egui::Button::new(RichText::new(label).size(17.0).strong())
        .corner_radius(CornerRadius::same(10));
    if let Some(tint) = tint {
        button = button.fill(tint.gamma_multiply(0.22)).stroke((1.5, tint));
    }
    ui.add_sized(Vec2::new(width, BIG_BUTTON_HEIGHT), button)
}

/// A smaller button for table rows; still comfortably clickable.
pub fn row_button(ui: &mut Ui, label: &str, tint: Option<Color32>, width: f32) -> Response {
    let mut button =
        egui::Button::new(RichText::new(label).size(14.0)).corner_radius(CornerRadius::same(8));
    if let Some(tint) = tint {
        button = button.fill(tint.gamma_multiply(0.20)).stroke((1.0, tint));
    }
    ui.add_sized(Vec2::new(width, ROW_BUTTON_HEIGHT), button)
}

/// A compact button for tight table rows.
pub fn compact_button(ui: &mut Ui, label: &str, tint: Option<Color32>, width: f32) -> Response {
    let mut button =
        egui::Button::new(RichText::new(label).size(12.0)).corner_radius(CornerRadius::same(5));
    if let Some(tint) = tint {
        button = button.fill(tint.gamma_multiply(0.20)).stroke((1.0, tint));
    }
    ui.add_sized(Vec2::new(width, 22.0), button)
}

/// Coloured status chip, e.g. "WiVRn: running".
pub fn pill(ui: &mut Ui, label: &str, value: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .stroke((1.0, color.gamma_multiply(0.8)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).size(12.0).color(GREY));
                ui.label(RichText::new(value).size(17.0).strong().color(color));
            });
        });
}

pub fn on_off(value: bool) -> Color32 {
    if value { GREEN } else { GREY }
}

/// Section heading with a little breathing room above it.
pub fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).size(19.0).strong());
    ui.add_space(4.0);
}

/// Big toggle switch with a label, returns true when it changed.
pub fn toggle(ui: &mut Ui, value: &mut bool, label: &str) -> bool {
    ui.add(egui::Checkbox::new(value, RichText::new(label).size(15.0)))
        .changed()
}

/// Format a countdown as `m:ss` (or `<n>s` under a minute).
pub fn format_countdown(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

/// Human description of a grace period.
pub fn format_grace(grace_secs: i64) -> String {
    if grace_secs < 0 {
        "keep running".to_string()
    } else if grace_secs == 0 {
        "immediately".to_string()
    } else {
        format!("after {}", format_countdown(grace_secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_formats_minutes_and_seconds() {
        assert_eq!(format_countdown(0), "0s");
        assert_eq!(format_countdown(59), "59s");
        assert_eq!(format_countdown(60), "1:00");
        assert_eq!(format_countdown(125), "2:05");
        assert_eq!(format_countdown(3600), "60:00");
    }

    #[test]
    fn grace_is_described_in_words() {
        assert_eq!(format_grace(-1), "keep running");
        assert_eq!(format_grace(0), "immediately");
        assert_eq!(format_grace(120), "after 2:00");
    }

    #[test]
    fn on_off_picks_distinct_colors() {
        assert_eq!(on_off(true), GREEN);
        assert_eq!(on_off(false), GREY);
    }
}
