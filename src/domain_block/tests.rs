use super::*;

#[test]
fn embedded_vrchat_fallback_parses_cleanly() {
    let parsed = vrchat_config::get_embedded_fallback_categories();
    assert!(parsed.contains_key("Videos"));
    assert!(parsed.contains_key("Images"));
    assert!(parsed.contains_key("Strings"));
    assert!(parsed.contains_key("Shared"));
}

#[test]
fn hostsfile_builder_sync_and_unblock() {
    let temp_dir = std::env::temp_dir().join("lvr_hostsfile_test");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let hosts_path = temp_dir.join("hosts");
    fs::write(&hosts_path, "127.0.0.1 localhost\n::1 localhost\n").unwrap();

    // 1. Write block for Videos
    let mut builder = hostsfile::HostsBuilder::new(BlockCategory::Videos.tag_name());
    let zero_ip: IpAddr = "0.0.0.0".parse().unwrap();
    builder.add_hostnames(zero_ip, ["youtube.com", "twitch.tv"]);
    builder.write_to(&hosts_path).unwrap();

    let content = fs::read_to_string(&hosts_path).unwrap();
    assert!(content.contains("# DO NOT EDIT LVR_Videos BEGIN"));
    assert!(content.contains("0.0.0.0 youtube.com twitch.tv"));
    assert!(content.contains("# DO NOT EDIT LVR_Videos END"));

    // 2. Clear block for Videos (empty builder)
    let empty_builder = hostsfile::HostsBuilder::new(BlockCategory::Videos.tag_name());
    empty_builder.write_to(&hosts_path).unwrap();

    let cleared_content = fs::read_to_string(&hosts_path).unwrap();
    assert!(!cleared_content.contains("LVR_Videos"));
    assert!(!cleared_content.contains("youtube.com"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn sanitize_domain_map_strips_ip_addresses() {
    let mut raw = BTreeMap::new();
    raw.insert(
        "Videos".to_string(),
        vec![
            "youtube.com".to_string(),
            "127.0.0.1".to_string(),
            "100.100.1.10".to_string(),
            "178.254.25.37".to_string(),
            "192.168.2.10".to_string(),
            "84.146.79.168".to_string(),
            "::1".to_string(),
            "*.twitch.tv".to_string(),
            "*.127.0.0.1".to_string(),
        ],
    );

    let sanitized = sanitize_domain_map(raw);
    let video = sanitized.get("Videos").unwrap();
    assert_eq!(video, &["*.twitch.tv", "youtube.com"]);
}

#[test]
fn invalid_and_protected_rules_cannot_enter_hosts_files() {
    let domains = sanitize_domain_map(BTreeMap::from([(
        "Videos".into(),
        vec![
            "API.VRCHAT.CLOUD".into(),
            "*.com".into(),
            "bad.local\n0.0.0.0 stolen.local".into(),
            "good.example.com".into(),
            "good.example.com".into(),
        ],
    )]));
    assert_eq!(domains["Videos"], ["good.example.com"]);
}
#[test]
fn shared_families_stay_out_of_individual_categories() {
    let domains = sanitize_domain_map(BTreeMap::from([
        ("Videos".into(), vec!["*.example.com".into()]),
        ("Images".into(), vec!["www.example.com".into()]),
    ]));
    assert!(domains["Videos"].is_empty());
    assert!(domains["Images"].is_empty());
    assert_eq!(domains["Shared"].len(), 2);
}
