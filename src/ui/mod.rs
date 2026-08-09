//! The egui front-end.

mod audio_tab;
mod autostart;
mod dashboard;
mod logs;
mod settings;
pub mod widgets;

use std::time::{Duration, Instant};

use egui::{RichText, ViewportCommand};

use crate::config::{AutostartEntry, Config, Trigger};
use crate::state::{Command, Shared, Status};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Autostart,
    Audio,
    Settings,
    Logs,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Dashboard,
        Tab::Autostart,
        Tab::Audio,
        Tab::Settings,
        Tab::Logs,
    ];

    /// Parse a tab name for `--tab`.
    pub fn from_name(name: &str) -> Option<Tab> {
        Tab::ALL
            .into_iter()
            .find(|tab| tab.label().eq_ignore_ascii_case(name.trim()))
    }

    fn label(self) -> &'static str {
        match self {
            Tab::Dashboard => "Dashboard",
            Tab::Autostart => "Autostart",
            Tab::Audio => "Audio",
            Tab::Settings => "Settings",
            Tab::Logs => "Logs",
        }
    }
}

/// Edit buffer for one autostart entry.
pub struct EntryEditor {
    pub entry: AutostartEntry,
    pub patterns_text: String,
    pub trigger_kind: usize,
    pub trigger_process: String,
    pub grace_mode: GraceMode,
    pub grace_secs: i64,
    pub is_new: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraceMode {
    KeepRunning,
    Immediately,
    After,
}

impl EntryEditor {
    pub fn new(entry: AutostartEntry, is_new: bool) -> Self {
        let trigger_kind = entry.trigger.kind_index();
        let trigger_process = match &entry.trigger {
            Trigger::Process(p) => p.clone(),
            _ => String::new(),
        };
        let (grace_mode, grace_secs) = match entry.grace_secs {
            g if g < 0 => (GraceMode::KeepRunning, 120),
            0 => (GraceMode::Immediately, 120),
            g => (GraceMode::After, g),
        };
        Self {
            patterns_text: entry.match_patterns.join("\n"),
            entry,
            trigger_kind,
            trigger_process,
            grace_mode,
            grace_secs,
            is_new,
            error: None,
        }
    }

    /// Fold the editor's scratch fields back into the entry.
    pub fn build(&self) -> AutostartEntry {
        let mut entry = self.entry.clone();
        entry.trigger = Trigger::from_kind_index(self.trigger_kind, self.trigger_process.trim());
        entry.grace_secs = match self.grace_mode {
            GraceMode::KeepRunning => -1,
            GraceMode::Immediately => 0,
            GraceMode::After => self.grace_secs.max(1),
        };
        entry.match_patterns = self
            .patterns_text
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect();
        entry
    }

    /// Reject entries that could never work.
    pub fn validate(&self) -> Result<AutostartEntry, String> {
        let entry = self.build();
        if entry.name.trim().is_empty() {
            return Err("Give the entry a name.".into());
        }
        if entry.command.trim().is_empty() {
            return Err("Give the entry a command to run.".into());
        }
        if let Trigger::Process(pattern) = &entry.trigger
            && pattern.trim().is_empty()
        {
            return Err("A custom process trigger needs a process name to look for.".into());
        }
        if !entry.use_shell
            && !entry.console
            && let Err(err) = shell_words::split(&entry.command)
        {
            return Err(format!("Command cannot be parsed: {err}"));
        }
        if !entry.working_dir.trim().is_empty()
            && !std::path::Path::new(entry.working_dir.trim()).is_dir()
        {
            return Err("Working directory does not exist.".into());
        }
        Ok(entry)
    }
}

pub struct LvrApp {
    shared: Shared,
    status: Status,
    tab: Tab,
    editor: Option<EntryEditor>,
    confirming_stop_all: bool,
    saved_config: Config,
    dirty_since: Option<Instant>,
    applied_zoom: f32,
    pub(crate) log_levels: [bool; 4],
    pub(crate) log_filter: String,
    pub(crate) log_wrap: bool,
}

const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(700);

impl LvrApp {
    pub fn new(cc: &eframe::CreationContext<'_>, shared: Shared, tab: Tab) -> Self {
        shared.set_repaint_ctx(cc.egui_ctx.clone());
        install_style(&cc.egui_ctx);
        let saved_config = shared.config_snapshot();
        let zoom = saved_config.general.ui_scale;
        cc.egui_ctx.set_zoom_factor(zoom);
        Self {
            status: shared.status_snapshot(),
            shared,
            tab,
            editor: None,
            confirming_stop_all: false,
            saved_config,
            dirty_since: None,
            applied_zoom: zoom,
            log_levels: [false, true, true, true],
            log_filter: String::new(),
            log_wrap: true,
        }
    }

    fn send(&self, command: Command) {
        self.shared.send(command);
    }

    /// Persist config edits a moment after the user stops fiddling.
    fn autosave(&mut self) {
        let current = self.shared.config_snapshot();
        if current == self.saved_config {
            self.dirty_since = None;
            return;
        }
        let since = *self.dirty_since.get_or_insert_with(Instant::now);
        if since.elapsed() < AUTOSAVE_DEBOUNCE {
            return;
        }
        {
            let mut config = self.shared.config();
            config.normalize();
        }
        let normalized = self.shared.config_snapshot();
        match normalized.save(self.shared.config_path()) {
            Ok(()) => {
                self.saved_config = normalized;
                self.shared.send(Command::Poke);
            }
            Err(err) => self.shared.error(format!("Saving config failed: {err:#}")),
        }
        self.dirty_since = None;
    }

    fn sync_zoom(&mut self, ctx: &egui::Context) {
        let wanted = self.shared.config().general.ui_scale;
        if (wanted - self.applied_zoom).abs() > f32::EPSILON {
            self.applied_zoom = wanted;
            ctx.set_zoom_factor(wanted);
        }
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for tab in Tab::ALL {
                let selected = self.tab == tab;
                let text = RichText::new(tab.label()).size(16.0).strong();
                let button = egui::Button::new(text)
                    .corner_radius(egui::CornerRadius::same(10))
                    .selected(selected);
                if ui.add_sized(egui::Vec2::new(132.0, 44.0), button).clicked() {
                    self.tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (color, text) = if self.status.headset_connected {
                    (widgets::GREEN, "HEADSET CONNECTED")
                } else if self.status.wivrn_running {
                    (widgets::BLUE, "WIVRN READY")
                } else {
                    (widgets::GREY, "WIVRN OFFLINE")
                };
                ui.label(RichText::new(text).size(14.0).strong().color(color));
            });
        });
        ui.add_space(6.0);
    }

    fn entry_editor_window(&mut self, ctx: &egui::Context) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let title = if editor.is_new {
            "New autostart entry"
        } else {
            "Edit autostart entry"
        };

        let viewport_id = egui::ViewportId::from_hash_of("entry_editor_viewport");
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([600.0, 640.0])
            .with_min_inner_size([480.0, 400.0])
            .with_resizable(true);

        let mut open = true;
        let mut close_and_save = false;
        let mut cancel = false;

        ctx.show_viewport_immediate(viewport_id, builder, |ctx, class| {
            let render_content = |ui: &mut egui::Ui, editor: &mut EntryEditor, close_and_save: &mut bool, cancel: &mut bool| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        autostart::editor_body(ui, editor);
                    });
                ui.separator();
                if let Some(error) = &editor.error {
                    ui.label(RichText::new(error).color(widgets::RED).size(15.0));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if widgets::big_button(ui, "Save", Some(widgets::GREEN), 160.0).clicked() {
                        *close_and_save = true;
                    }
                    if widgets::big_button(ui, "Cancel", None, 160.0).clicked() {
                        *cancel = true;
                    }
                });
            };

            if class == egui::ViewportClass::Root {
                egui::Window::new(title)
                    .open(&mut open)
                    .collapsible(false)
                    .resizable(true)
                    .default_size([600.0, 640.0])
                    .show(ctx, |ui| {
                        render_content(ui, editor, &mut close_and_save, &mut cancel);
                    });
            } else {
                egui::CentralPanel::default().show(ctx, |ui| {
                    render_content(ui, editor, &mut close_and_save, &mut cancel);
                });
                if ctx.input(|i| i.viewport().close_requested()) {
                    open = false;
                }
            }
        });

        if close_and_save {
            let editor = self.editor.as_mut().expect("editor is open");
            match editor.validate() {
                Ok(mut entry) => {
                    let is_new = editor.is_new;
                    {
                        let mut config = self.shared.config();
                        if is_new {
                            entry.id = config.unique_id(&entry.name);
                            config.autostart.push(entry);
                        } else if let Some(slot) =
                            config.autostart.iter_mut().find(|e| e.id == entry.id)
                        {
                            *slot = entry;
                        } else {
                            entry.id = config.unique_id(&entry.name);
                            config.autostart.push(entry);
                        }
                        config.normalize();
                    }
                    self.editor = None;
                    self.send(Command::Poke);
                }
                Err(message) => editor.error = Some(message),
            }
        } else if cancel || !open {
            self.editor = None;
        }
    }

    fn stop_all_confirm_window(&mut self, ctx: &egui::Context) {
        if !self.confirming_stop_all {
            return;
        }
        let mut open = true;
        let mut decision: Option<bool> = None;
        egui::Window::new("Stop everything VR?")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "This stops every managed app, shuts down WiVRn and puts audio back \
                         on the desktop devices.\n\nThe WiVRn watchdog stays paused until you \
                         start it again.",
                    )
                    .size(15.0),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if widgets::big_button(ui, "Stop everything", Some(widgets::RED), 220.0)
                        .clicked()
                    {
                        decision = Some(true);
                    }
                    if widgets::big_button(ui, "Cancel", None, 160.0).clicked() {
                        decision = Some(false);
                    }
                });
            });
        match decision {
            Some(true) => {
                self.send(Command::StopAllVr);
                self.confirming_stop_all = false;
            }
            Some(false) => self.confirming_stop_all = false,
            None => {
                if !open {
                    self.confirming_stop_all = false;
                }
            }
        }
    }

    fn request_stop_all(&mut self) {
        if self.shared.config().general.confirm_stop_all {
            self.confirming_stop_all = true;
        } else {
            self.send(Command::StopAllVr);
        }
    }
}

impl eframe::App for LvrApp {
    /// Runs even while the window is hidden in the tray, so tray commands and
    /// config saves keep working without a visible window.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.status = self.shared.status_snapshot();
        self.sync_zoom(ctx);

        if self.shared.take_show_window() {
            if let Some(tab) = self
                .shared
                .take_requested_tab()
                .and_then(|name| Tab::from_name(&name))
            {
                self.tab = tab;
            }
            ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }

        if self.shared.is_quitting() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        } else if ctx.input(|i| i.viewport().close_requested()) {
            if self.shared.config().general.close_to_tray {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            } else {
                self.shared.set_quitting();
                self.shared.send(Command::Quit);
            }
        }

        self.autosave();

        // Keep countdowns ticking even when nothing else asks for a repaint.
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("tabs").show(ui, |ui| self.tab_bar(ui));

        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Dashboard => dashboard::show(self, ui),
            Tab::Autostart => autostart::show(self, ui),
            Tab::Audio => audio_tab::show(self, ui),
            Tab::Settings => settings::show(self, ui),
            Tab::Logs => logs::show(self, ui),
        });

        let ctx = ui.ctx().clone();
        self.entry_editor_window(&ctx);
        self.stop_all_confirm_window(&ctx);
    }

    fn on_exit(&mut self) {
        if let Err(err) = self.shared.save_config() {
            tracing::error!("saving config on exit failed: {err:#}");
        }
        self.shared.set_quitting();
        self.shared.send(Command::Quit);
    }
}

/// Slightly larger text and roomier spacing than egui's defaults.
fn install_style(ctx: &egui::Context) {
    use egui::{FontFamily, FontId, TextStyle};

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (
                TextStyle::Heading,
                FontId::new(24.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(15.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(12.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(13.0, FontFamily::Monospace),
            ),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 8.0);
        style.spacing.interact_size.y = 32.0;
        style.spacing.slider_width = 220.0;
        style.visuals.widgets.hovered.expansion = 2.0;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(entry: AutostartEntry) -> EntryEditor {
        EntryEditor::new(entry, false)
    }

    #[test]
    fn editor_round_trips_grace_modes() {
        let keep = editor(AutostartEntry {
            grace_secs: -1,
            ..Default::default()
        });
        assert_eq!(keep.grace_mode, GraceMode::KeepRunning);
        assert_eq!(keep.build().grace_secs, -1);

        let now = editor(AutostartEntry {
            grace_secs: 0,
            ..Default::default()
        });
        assert_eq!(now.grace_mode, GraceMode::Immediately);
        assert_eq!(now.build().grace_secs, 0);

        let later = editor(AutostartEntry {
            grace_secs: 120,
            ..Default::default()
        });
        assert_eq!(later.grace_mode, GraceMode::After);
        assert_eq!(later.build().grace_secs, 120);
    }

    #[test]
    fn editor_round_trips_custom_process_triggers() {
        let mut editor = editor(AutostartEntry {
            trigger: Trigger::Process("Foo.exe".into()),
            ..Default::default()
        });
        assert_eq!(editor.trigger_process, "Foo.exe");
        editor.trigger_process = "Bar.exe".into();
        assert_eq!(editor.build().trigger, Trigger::Process("Bar.exe".into()));
    }

    #[test]
    fn editor_parses_patterns_line_by_line() {
        let mut editor = editor(AutostartEntry::default());
        editor.patterns_text = "  one  \n\n two\n".into();
        assert_eq!(
            editor.build().match_patterns,
            vec!["one".to_string(), "two".to_string()]
        );
    }

    #[test]
    fn validation_catches_the_obvious_mistakes() {
        let mut editor = editor(AutostartEntry::default());
        assert!(editor.validate().unwrap_err().contains("name"));

        editor.entry.name = "Thing".into();
        assert!(editor.validate().unwrap_err().contains("command"));

        editor.entry.command = "foo 'unbalanced".into();
        assert!(editor.validate().unwrap_err().contains("parsed"));

        editor.entry.command = "/bin/true".into();
        editor.entry.working_dir = "/definitely/not/here".into();
        assert!(editor.validate().unwrap_err().contains("Working directory"));

        editor.entry.working_dir = String::new();
        assert!(editor.validate().is_ok());
    }

    #[test]
    fn validation_requires_a_pattern_for_custom_triggers() {
        let mut editor = editor(AutostartEntry {
            name: "Thing".into(),
            command: "/bin/true".into(),
            trigger: Trigger::Process(String::new()),
            ..Default::default()
        });
        editor.trigger_kind = 3;
        editor.trigger_process = "   ".into();
        assert!(editor.validate().unwrap_err().contains("process name"));
        editor.trigger_process = "Foo.exe".into();
        assert!(editor.validate().is_ok());
    }

    #[test]
    fn shell_and_console_commands_skip_word_splitting_checks() {
        let editor = editor(AutostartEntry {
            name: "Thing".into(),
            command: "echo 'unbalanced".into(),
            use_shell: true,
            ..Default::default()
        });
        assert!(editor.validate().is_ok());
    }

    #[test]
    fn tabs_can_be_named_on_the_command_line() {
        assert_eq!(Tab::from_name("logs"), Some(Tab::Logs));
        assert_eq!(Tab::from_name("  AutoStart "), Some(Tab::Autostart));
        assert_eq!(Tab::from_name("nope"), None);
    }

    #[test]
    fn tabs_have_unique_labels() {
        let mut labels: Vec<&str> = Tab::ALL.iter().map(|t| t.label()).collect();
        labels.sort_unstable();
        let count = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), count);
    }

    #[test]
    fn ui_fits_in_viewport_without_horizontal_overflow() {
        let ctx = egui::Context::default();
        let (shared, _rx) = crate::state::Shared::new(
            crate::config::Config::default(),
            std::path::PathBuf::from("/tmp/lvr-viewport-test.toml"),
        );
        let saved_config = shared.config_snapshot();
        let mut app = LvrApp {
            status: shared.status_snapshot(),
            shared,
            tab: Tab::Dashboard,
            editor: None,
            confirming_stop_all: false,
            saved_config,
            dirty_since: None,
            applied_zoom: 1.0,
            log_levels: [false, true, true, true],
            log_filter: String::new(),
            log_wrap: true,
        };

        let viewport_size = egui::vec2(960.0, 640.0);
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                viewport_size,
            )),
            ..Default::default()
        };

        for tab in Tab::ALL {
            app.tab = tab;
            let mut full_output = ctx.run_ui(raw_input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| match app.tab {
                    Tab::Dashboard => dashboard::show(&mut app, ui),
                    Tab::Autostart => autostart::show(&mut app, ui),
                    Tab::Audio => audio_tab::show(&mut app, ui),
                    Tab::Settings => settings::show(&mut app, ui),
                    Tab::Logs => logs::show(&mut app, ui),
                });
            });
            full_output.textures_delta.clear();

            let used_rect = ctx.globally_used_rect();
            assert!(
                used_rect.max.x <= viewport_size.x + 1.0,
                "Tab {:?} overflowed viewport horizontally: used_x={}, viewport_x={}",
                tab,
                used_rect.max.x,
                viewport_size.x
            );
        }
    }
}
