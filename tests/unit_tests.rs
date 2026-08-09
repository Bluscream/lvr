use lvr::audio::{AudioState, AudioSwitcher};
use lvr::config::{AutostartRule, Config, TriggerType};
use lvr::process::haystack_matches;

#[test]
fn test_default_config() {
    let cfg = Config::default();
    assert!(cfg.auto_restart_wivrn);
    assert!(cfg.auto_switch_audio);
    assert_eq!(cfg.autostart_rules.len(), 6);

    let vrc_video_cacher = cfg
        .autostart_rules
        .iter()
        .find(|r| r.id == "vrc-video-cacher")
        .unwrap();
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

/// The switcher's bookkeeping, without touching the machine's audio.
///
/// The previous version of this test called `restore_previous_audio()`, which
/// shells out to `pactl set-default-source`; running the test suite actually
/// changed the developer's default microphone.
#[test]
fn test_audio_switcher_state_machine() {
    let switcher = AudioSwitcher::new();
    assert_eq!(switcher.current_state, AudioState::Disconnected);
    assert!(switcher.previous_sink.is_none());
    assert!(switcher.previous_source.is_none());

    let connected = AudioSwitcher {
        previous_sink: Some("alsa_output.example".to_string()),
        previous_source: Some("alsa_input.example".to_string()),
        current_state: AudioState::ConnectedToWiVRn,
    };
    assert_eq!(connected.current_state, AudioState::ConnectedToWiVRn);
    assert_eq!(connected.previous_sink.as_deref(), Some("alsa_output.example"));
}

/// Every default rule must recognise the process it launches.
///
/// A rule that cannot see its own process is considered "not running" on every
/// poll and gets launched again and again.
#[test]
fn test_every_default_rule_can_match_something() {
    for rule in Config::default().autostart_rules {
        let patterns = rule.effective_patterns();
        assert!(
            !patterns.is_empty(),
            "rule '{}' has no way to detect its process",
            rule.name
        );
        assert!(
            patterns.iter().all(|p| !p.starts_with('-')),
            "rule '{}' derived a command-line flag as a match pattern: {:?}",
            rule.name,
            patterns
        );
    }
}

#[test]
fn test_rules_match_their_real_world_processes() {
    let cfg = Config::default();
    let find = |id: &str| cfg.autostart_rules.iter().find(|r| r.id == id).unwrap();

    // Command lines as they actually appear in `ps` on the target machine.
    let cases = [
        ("vrcx-0", "vrcx-0 /var/home/blu/appimages/vrcx0.appimage --autostart"),
        (
            "vrcx-extras",
            "start.sh /bin/bash /run/media/system/data/projects/vrcx-extras/start.sh",
        ),
        ("slimevr", "slimevr /app/main/slimevr"),
        (
            "vrcosc",
            "wine c:\\users\\steamuser\\appdata\\local\\vrcosc\\vrcosc.exe",
        ),
    ];

    for (id, cmdline) in cases {
        let rule = find(id);
        assert!(
            haystack_matches(cmdline, &rule.effective_patterns()),
            "rule '{}' does not match its own process line `{}`",
            rule.name,
            cmdline
        );
    }
}

#[test]
fn test_grace_period_is_read_from_the_rule() {
    let rule = AutostartRule {
        id: "test-app".to_string(),
        name: "Test App".to_string(),
        exec_cmd: "echo test".to_string(),
        trigger: TriggerType::VRChat,
        grace_period_secs: 5,
        ..Default::default()
    };
    assert!(!rule.keeps_running());

    let forever = AutostartRule {
        grace_period_secs: -1,
        ..rule.clone()
    };
    assert!(forever.keeps_running());
}
