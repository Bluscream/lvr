use crate::config::{AutostartRule, TriggerType};
use crate::service::AppState;
use chrono::Local;
use eframe::egui::{self, Color32, FontId, RichText, Vec2};
use std::sync::{Arc, Mutex};

#[derive(PartialEq)]
enum Tab {
    Dashboard,
    AutostartRules,
    AudioSettings,
    EventLogs,
}

pub struct LinuxVrGui {
    pub state: AppState,
    pub nuke_trigger: Arc<Mutex<bool>>,
    current_tab: Tab,

    // New rule form fields
    new_rule_name: String,
    new_rule_cmd: String,
    new_rule_trigger: TriggerType,
    new_rule_grace: String,
    new_rule_patterns: String,
    show_add_modal: bool,
}

impl LinuxVrGui {
    pub fn new(state: AppState, nuke_trigger: Arc<Mutex<bool>>) -> Self {
        Self {
            state,
            nuke_trigger,
            current_tab: Tab::Dashboard,
            new_rule_name: String::new(),
            new_rule_cmd: String::new(),
            new_rule_trigger: TriggerType::VRChat,
            new_rule_grace: "120".to_string(),
            new_rule_patterns: String::new(),
            show_add_modal: false,
        }
    }

    fn apply_vr_style(ctx: &egui::Context) {
        ctx.style_mut_of(egui::Theme::Dark, |style| {
            style.spacing.item_spacing = Vec2::new(12.0, 12.0);
            style.spacing.button_padding = Vec2::new(16.0, 12.0);
            style.spacing.interact_size = Vec2::new(40.0, 40.0);
        });

        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(18, 22, 32);
        visuals.window_fill = Color32::from_rgb(24, 28, 40);
        visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(28, 34, 48);
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(38, 46, 64);
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(52, 64, 90);
        visuals.widgets.active.bg_fill = Color32::from_rgb(64, 80, 112);

        visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.5, Color32::from_rgb(220, 230, 245));
        visuals.widgets.hovered.fg_stroke = egui::Stroke::new(2.0, Color32::WHITE);
        visuals.widgets.active.fg_stroke = egui::Stroke::new(2.0, Color32::WHITE);

        ctx.set_visuals(visuals);
    }
}

impl eframe::App for LinuxVrGui {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        Self::apply_vr_style(&ctx);

        // Periodically request repaint for live status updates
        ctx.request_repaint_after(std::time::Duration::from_millis(500));

        // Header Panel
        egui::Frame::NONE
            .fill(Color32::from_rgb(14, 18, 26))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(
                        RichText::new("🥽 LinuxVR (lvr)")
                            .font(FontId::proportional(26.0))
                            .color(Color32::from_rgb(80, 200, 255))
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let wivrn_running = *self.state.wivrn_running.lock().unwrap();
                        let audio_conn = *self.state.wivrn_audio_connected.lock().unwrap();

                        if audio_conn {
                            ui.label(
                                RichText::new("🎧 Audio: VR Headset")
                                    .font(FontId::proportional(16.0))
                                    .color(Color32::from_rgb(100, 255, 150))
                                    .strong(),
                            );
                        }
                        if wivrn_running {
                            ui.label(
                                RichText::new("🟢 WiVRn: Running")
                                    .font(FontId::proportional(16.0))
                                    .color(Color32::from_rgb(100, 255, 150))
                                    .strong(),
                            );
                        } else {
                            ui.label(
                                RichText::new("🔴 WiVRn: Stopped")
                                    .font(FontId::proportional(16.0))
                                    .color(Color32::from_rgb(255, 100, 100))
                                    .strong(),
                            );
                        }
                    });
                });

                ui.add_space(10.0);

                // Navigation Tabs - Large Touch Targets
                ui.horizontal(|ui| {
                    let tab_btn = |ui: &mut egui::Ui, text: &str, _tab: Tab, selected: bool| -> bool {
                        let color = if selected {
                            Color32::from_rgb(0, 180, 255)
                        } else {
                            Color32::from_rgb(45, 55, 75)
                        };
                        let text_color = if selected { Color32::WHITE } else { Color32::from_rgb(180, 190, 210) };
                        let btn = egui::Button::new(
                            RichText::new(text)
                                .font(FontId::proportional(18.0))
                                .color(text_color)
                                .strong(),
                        )
                        .fill(color)
                        .min_size(Vec2::new(160.0, 48.0));
                        ui.add(btn).clicked()
                    };

                    if tab_btn(ui, "🏠 Dashboard", Tab::Dashboard, self.current_tab == Tab::Dashboard) {
                        self.current_tab = Tab::Dashboard;
                    }
                    if tab_btn(ui, "⚡ Autostart Rules", Tab::AutostartRules, self.current_tab == Tab::AutostartRules) {
                        self.current_tab = Tab::AutostartRules;
                    }
                    if tab_btn(ui, "🔊 Audio Switcher", Tab::AudioSettings, self.current_tab == Tab::AudioSettings) {
                        self.current_tab = Tab::AudioSettings;
                    }
                    if tab_btn(ui, "📋 Activity Logs", Tab::EventLogs, self.current_tab == Tab::EventLogs) {
                        self.current_tab = Tab::EventLogs;
                    }
                });
            });

        ui.add_space(10.0);

        match self.current_tab {
            Tab::Dashboard => self.show_dashboard(ui),
            Tab::AutostartRules => self.show_autostart_rules(ui),
            Tab::AudioSettings => self.show_audio_settings(ui),
            Tab::EventLogs => self.show_event_logs(ui),
        }
    }
}

impl LinuxVrGui {
    fn show_dashboard(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);

        // Huge Nuke Button (Primary VR Command)
        let nuke_btn = egui::Button::new(
            RichText::new("💥 NUKE ALL VR & RESTART WIVRN")
                .font(FontId::proportional(22.0))
                .color(Color32::WHITE)
                .strong(),
        )
        .fill(Color32::from_rgb(220, 40, 40))
        .min_size(Vec2::new(ui.available_width(), 65.0));

        if ui.add(nuke_btn).clicked() {
            *self.nuke_trigger.lock().unwrap() = true;
        }

        ui.add_space(20.0);
        ui.separator();
        ui.add_space(10.0);

        // Status & Toggles Grid
        ui.columns(2, |cols| {
            cols[0].group(|ui| {
                ui.heading(RichText::new("⚙️ Service Watchdogs").font(FontId::proportional(20.0)));
                ui.add_space(10.0);

                let mut cfg = self.state.config.lock().unwrap();

                let mut wivrn_auto = cfg.auto_restart_wivrn;
                if ui.checkbox(&mut wivrn_auto, RichText::new("Auto-Restart WiVRn on Crash/Close").font(FontId::proportional(17.0))).changed() {
                    cfg.auto_restart_wivrn = wivrn_auto;
                    let _ = cfg.save();
                }

                ui.add_space(10.0);

                let mut audio_auto = cfg.auto_switch_audio;
                if ui.checkbox(&mut audio_auto, RichText::new("Auto-Switch Mic & Output to VR Headset").font(FontId::proportional(17.0))).changed() {
                    cfg.auto_switch_audio = audio_auto;
                    let _ = cfg.save();
                }
            });

            cols[1].group(|ui| {
                ui.heading(RichText::new("📊 Live Status").font(FontId::proportional(20.0)));
                ui.add_space(10.0);

                let wivrn_running = *self.state.wivrn_running.lock().unwrap();
                let vrchat_running = *self.state.vrchat_running.lock().unwrap();
                let audio_conn = *self.state.wivrn_audio_connected.lock().unwrap();

                ui.horizontal(|ui| {
                    ui.label(RichText::new("WiVRn Service:").font(FontId::proportional(17.0)));
                    if wivrn_running {
                        ui.label(RichText::new("RUNNING").font(FontId::proportional(17.0)).color(Color32::GREEN).strong());
                    } else {
                        ui.label(RichText::new("STOPPED").font(FontId::proportional(17.0)).color(Color32::RED).strong());
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(RichText::new("VRChat Process:").font(FontId::proportional(17.0)));
                    if vrchat_running {
                        ui.label(RichText::new("RUNNING").font(FontId::proportional(17.0)).color(Color32::GREEN).strong());
                    } else {
                        ui.label(RichText::new("NOT DETECTED").font(FontId::proportional(17.0)).color(Color32::GRAY));
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(RichText::new("VR Audio Routing:").font(FontId::proportional(17.0)));
                    if audio_conn {
                        ui.label(RichText::new("CONNECTED TO WIVRN").font(FontId::proportional(17.0)).color(Color32::CYAN).strong());
                    } else {
                        ui.label(RichText::new("STANDARD SYSTEM AUDIO").font(FontId::proportional(17.0)).color(Color32::LIGHT_GRAY));
                    }
                });
            });
        });
    }

    fn show_autostart_rules(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("⚡ Autostart & Grace Period Rules").font(FontId::proportional(22.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let add_btn = egui::Button::new(
                    RichText::new("➕ Add New Rule")
                        .font(FontId::proportional(16.0))
                        .strong(),
                )
                .fill(Color32::from_rgb(0, 160, 220))
                .min_size(Vec2::new(150.0, 40.0));

                if ui.add(add_btn).clicked() {
                    self.show_add_modal = !self.show_add_modal;
                }

                let reset_btn = egui::Button::new(
                    RichText::new("🔄 Reset Defaults")
                        .font(FontId::proportional(16.0)),
                )
                .fill(Color32::from_rgb(80, 90, 110))
                .min_size(Vec2::new(140.0, 40.0));

                if ui.add(reset_btn).clicked() {
                    let mut cfg = self.state.config.lock().unwrap();
                    *cfg = crate::config::Config::default();
                    let _ = cfg.save();
                    self.state.add_log("Reset autostart rules to default configuration.");
                }
            });
        });

        ui.add_space(10.0);

        // Add Rule Form Modal / Inline
        if self.show_add_modal {
            ui.group(|ui| {
                ui.heading(RichText::new("Create New Autostart Rule").font(FontId::proportional(18.0)));
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut self.new_rule_name);
                    ui.label("Trigger:");
                    egui::ComboBox::from_id_salt("trigger_combo")
                        .selected_text(self.new_rule_trigger.to_string())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.new_rule_trigger, TriggerType::VRChat, "VRChat");
                            ui.selectable_value(&mut self.new_rule_trigger, TriggerType::WiVRn, "WiVRn");
                            ui.selectable_value(&mut self.new_rule_trigger, TriggerType::Always, "Always");
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("Command:");
                    ui.text_edit_singleline(&mut self.new_rule_cmd);
                    ui.label("Grace (sec, -1=infinite):");
                    ui.text_edit_singleline(&mut self.new_rule_grace);
                });
                ui.horizontal(|ui| {
                    ui.label("Match (comma separated, blank = derive from command):");
                    ui.text_edit_singleline(&mut self.new_rule_patterns);
                });
                ui.horizontal(|ui| {
                    if ui.button("Save Rule").clicked()
                        && !self.new_rule_name.is_empty()
                        && !self.new_rule_cmd.is_empty()
                    {
                        let grace: i64 = self.new_rule_grace.parse().unwrap_or(120);
                        let rule = AutostartRule {
                            id: format!("custom-{}", Local::now().timestamp_millis()),
                            name: self.new_rule_name.clone(),
                            enabled: true,
                            exec_cmd: self.new_rule_cmd.clone(),
                            trigger: self.new_rule_trigger.clone(),
                            grace_period_secs: grace,
                            match_patterns: self
                                .new_rule_patterns
                                .split(',')
                                .map(|p| p.trim().to_string())
                                .filter(|p| !p.is_empty())
                                .collect(),
                        };
                        let mut cfg = self.state.config.lock().unwrap();
                        cfg.autostart_rules.push(rule);
                        let _ = cfg.save();
                        self.new_rule_name.clear();
                        self.new_rule_cmd.clear();
                        self.new_rule_patterns.clear();
                        self.show_add_modal = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.show_add_modal = false;
                    }
                });
            });
            ui.add_space(10.0);
        }

        // Rules List Table
        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut cfg = self.state.config.lock().unwrap();
            let mut delete_id: Option<String> = None;
            let mut run_cmd: Option<String> = None;

            for rule in cfg.autostart_rules.iter_mut() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        let mut enabled = rule.enabled;
                        if ui.checkbox(&mut enabled, "").changed() {
                            rule.enabled = enabled;
                        }

                        ui.label(
                            RichText::new(&rule.name)
                                .font(FontId::proportional(18.0))
                                .strong(),
                        );

                        ui.label(
                            RichText::new(format!("[Trigger: {}]", rule.trigger))
                                .font(FontId::proportional(15.0))
                                .color(Color32::from_rgb(0, 200, 255)),
                        );

                        let grace_str = if rule.grace_period_secs < 0 {
                            "Keep Running (-1)".to_string()
                        } else {
                            format!("Grace: {}s", rule.grace_period_secs)
                        };
                        ui.label(
                            RichText::new(grace_str)
                                .font(FontId::proportional(15.0))
                                .color(Color32::from_rgb(255, 200, 100)),
                        );

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(RichText::new("🗑 Delete").color(Color32::LIGHT_RED)).clicked() {
                                delete_id = Some(rule.id.clone());
                            }
                            if ui.button("▶ Run Now").clicked() {
                                run_cmd = Some(rule.exec_cmd.clone());
                            }
                        });
                    });

                    ui.label(
                        RichText::new(format!("Cmd: {}", rule.exec_cmd))
                            .font(FontId::monospace(14.0))
                            .color(Color32::GRAY),
                    );
                });
                ui.add_space(6.0);
            }

            if let Some(id) = delete_id {
                cfg.autostart_rules.retain(|r| r.id != id);
                let _ = cfg.save();
            }

            if let Some(cmd) = run_cmd {
                self.state.add_log(format!("Manual trigger exec: {}", cmd));
                let _ = std::process::Command::new("sh").args(["-c", &cmd]).spawn();
            }
        });
    }

    fn show_audio_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("🔊 Audio Output & Microphone Router").font(FontId::proportional(22.0)));
        ui.add_space(10.0);

        let default_sink = crate::audio::AudioSwitcher::get_default_sink().unwrap_or_else(|| "Unknown".to_string());
        let default_source = crate::audio::AudioSwitcher::get_default_source().unwrap_or_else(|| "Unknown".to_string());
        let (wivrn_sink, wivrn_source) = crate::audio::AudioSwitcher::is_wivrn_audio_available();

        ui.group(|ui| {
            ui.heading("Current Active PipeWire / PulseAudio Devices");
            ui.add_space(8.0);
            ui.label(format!("Default Output Sink: {}", default_sink));
            ui.label(format!("Default Microphone Source: {}", default_source));
            ui.add_space(8.0);
            ui.label(format!("WiVRn Sink Node Available: {}", if wivrn_sink { "Yes (wivrn.sink)" } else { "No" }));
            ui.label(format!("WiVRn Source Node Available: {}", if wivrn_source { "Yes (wivrn.source)" } else { "No" }));
        });

        ui.add_space(15.0);

        ui.horizontal(|ui| {
            let switch_btn = egui::Button::new(
                RichText::new("🎧 Force Switch to WiVRn Audio")
                    .font(FontId::proportional(18.0))
                    .strong(),
            )
            .fill(Color32::from_rgb(0, 160, 220))
            .min_size(Vec2::new(260.0, 50.0));

            if ui.add(switch_btn).clicked() {
                if wivrn_sink {
                    crate::audio::AudioSwitcher::set_default_sink("wivrn.sink");
                }
                if wivrn_source {
                    crate::audio::AudioSwitcher::set_default_source("wivrn.source");
                }
                self.state.add_log("Manually switched audio output and mic to WiVRn");
            }

            let restore_btn = egui::Button::new(
                RichText::new("🔊 Restore Standard System Audio")
                    .font(FontId::proportional(18.0)),
            )
            .fill(Color32::from_rgb(70, 80, 100))
            .min_size(Vec2::new(260.0, 50.0));

            if ui.add(restore_btn).clicked() {
                self.state
                    .audio_switcher
                    .lock()
                    .unwrap()
                    .restore_previous_audio();
                *self.state.wivrn_audio_connected.lock().unwrap() = false;
                self.state.add_log("Manually restored system default audio devices.");
            }
        });
    }

    fn show_event_logs(&mut self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("📋 Real-time Activity Logs").font(FontId::proportional(22.0)));
        ui.add_space(10.0);

        let logs = self.state.log_messages.lock().unwrap().clone();

        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in logs {
                    ui.label(
                        RichText::new(line)
                            .font(FontId::monospace(14.0))
                            .color(Color32::from_rgb(180, 220, 240)),
                    );
                }
            });
    }
}
