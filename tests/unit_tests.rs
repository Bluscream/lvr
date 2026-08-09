#[cfg(test)]
mod tests {
    use lvr::audio::{AudioState, AudioSwitcher};
    use lvr::config::{AutostartRule, Config, TriggerType};
    use std::time::{Duration, Instant};

    #[test]
    fn test_default_config() {
        let cfg = Config::default();
        assert!(cfg.auto_restart_wivrn);
        assert!(cfg.auto_switch_audio);
        assert_eq!(cfg.autostart_rules.len(), 6);

        let vrc_video_cacher = cfg.autostart_rules.iter().find(|r| r.id == "vrc-video-cacher").unwrap();
        assert_eq!(vrc_video_cacher.grace_period_secs, 120);
        assert_eq!(vrc_video_cacher.trigger, TriggerType::VRChat);

        let vrcx = cfg.autostart_rules.iter().find(|r| r.id == "vrcx-0").unwrap();
        assert_eq!(vrcx.grace_period_secs, -1);
        assert_eq!(vrcx.trigger, TriggerType::VRChat);

        let slimevr = cfg.autostart_rules.iter().find(|r| r.id == "slimevr").unwrap();
        assert_eq!(slimevr.grace_period_secs, 300);
        assert_eq!(slimevr.trigger, TriggerType::WiVRn);
    }

    #[test]
    fn test_config_serialization() {
        let cfg = Config::default();
        let json = serde_json::to_string(&cfg).expect("Serialization failed");
        let deserialized: Config = serde_json::from_str(&json).expect("Deserialization failed");
        assert_eq!(cfg.auto_restart_wivrn, deserialized.auto_restart_wivrn);
        assert_eq!(cfg.autostart_rules.len(), deserialized.autostart_rules.len());
    }

    #[test]
    fn test_audio_switcher_state_machine() {
        let mut switcher = AudioSwitcher::new();
        assert_eq!(switcher.current_state, AudioState::Disconnected);
        assert!(switcher.previous_sink.is_none());
        assert!(switcher.previous_source.is_none());

        switcher.previous_sink = Some("alsa_output.pci-0000_00_1f.3.analog-stereo".to_string());
        switcher.previous_source = Some("alsa_input.pci-0000_00_1f.3.analog-stereo".to_string());
        switcher.current_state = AudioState::ConnectedToWiVRn;

        switcher.restore_previous_audio();
        assert!(switcher.previous_sink.is_none());
        assert!(switcher.previous_source.is_none());
    }

    #[test]
    fn test_grace_period_logic() {
        let rule = AutostartRule {
            id: "test-app".to_string(),
            name: "Test App".to_string(),
            enabled: true,
            exec_cmd: "echo test".to_string(),
            trigger: TriggerType::VRChat,
            grace_period_secs: 5,
        };

        let now = Instant::now();
        let expires_at = now + Duration::from_secs(rule.grace_period_secs as u64);
        assert!(expires_at > now);
        assert_eq!(expires_at.duration_since(now).as_secs(), 5);
    }
}
