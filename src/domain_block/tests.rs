use std::collections::HashSet;
use std::fs;

use super::*;

#[test]
fn domain_lists_have_zero_overlap_between_categories() {
    let fallback = build_domain_lists_fallback_with_inputs(&[]);
    let video_set: HashSet<_> = fallback.video_domains.iter().cloned().collect();
    let image_set: HashSet<_> = fallback.image_domains.iter().cloned().collect();
    let string_set: HashSet<_> = fallback.string_domains.iter().cloned().collect();
    let rest_set: HashSet<_> = fallback.rest_domains.iter().cloned().collect();
    let protected_set: HashSet<_> = fallback.protected_domains.iter().cloned().collect();

    // 1. None of the four categories share any domains
    assert!(
        video_set.is_disjoint(&image_set),
        "Video and Image lists overlap!"
    );
    assert!(
        video_set.is_disjoint(&string_set),
        "Video and String lists overlap!"
    );
    assert!(
        video_set.is_disjoint(&rest_set),
        "Video and Rest lists overlap!"
    );
    assert!(
        image_set.is_disjoint(&string_set),
        "Image and String lists overlap!"
    );
    assert!(
        image_set.is_disjoint(&rest_set),
        "Image and Rest lists overlap!"
    );
    assert!(
        string_set.is_disjoint(&rest_set),
        "String and Rest lists overlap!"
    );

    // 2. Core VRChat and whiteListedAssetUrls are strictly protected and not in ANY category
    for core in &[
        "assets.vrchat.com",
        "vrchat.com",
        "vrchat.cloud",
        "dbinj8iahsbec.cloudfront.net",
    ] {
        assert!(!video_set.contains(*core), "core {core} in Video list!");
        assert!(!image_set.contains(*core), "core {core} in Image list!");
        assert!(!string_set.contains(*core), "core {core} in String list!");
        assert!(!rest_set.contains(*core), "core {core} in Rest list!");
        assert!(
            protected_set.contains(*core),
            "core {core} not in Protected list!"
        );
    }

    // 3. Multi-list domain github.io is excluded from pure lists, present in Rest
    assert!(!video_set.contains("github.io") && !video_set.contains("*.github.io"));
    assert!(!image_set.contains("github.io") && !image_set.contains("*.github.io"));
    assert!(!string_set.contains("github.io") && !string_set.contains("*.github.io"));
    assert!(rest_set.contains("*.github.io"));
}

#[test]
fn parse_vrchat_config_json_partitions_and_protects_shared() {
    let sample_json = r#"{
        "urlList": ["*.youtube.com", "twitch.tv", "ciel.topaz.chat", "*.github.io", "assets.vrchat.com", "redirect.vrchat.com"],
        "imageHostUrlList": ["i.imgur.com", "ciel.topaz.chat", "*.github.io", "assets.vrchat.com"],
        "stringHostUrlList": ["pastebin.com", "*.github.io"],
        "whiteListedAssetUrls": ["https://assets.vrchat.com/adminfiles/", "https://dbinj8iahsbec.cloudfront.net/plugins"]
    }"#;

    let lists =
        parse_all_domain_lists(sample_json, &[]).expect("failed to parse sample config JSON");

    // 1. Check pure separation
    assert!(lists.video_domains.contains(&"*.youtube.com".to_string()));
    assert!(lists.video_domains.contains(&"twitch.tv".to_string()));
    assert!(lists.image_domains.contains(&"i.imgur.com".to_string()));
    assert!(lists.string_domains.contains(&"pastebin.com".to_string()));

    // 2. Shared across lists (*.github.io, ciel.topaz.chat) MUST NOT be in pure video/image/string lists
    for shared in &["*.github.io", "ciel.topaz.chat"] {
        assert!(
            !lists.video_domains.contains(&shared.to_string()),
            "shared {shared} in video"
        );
        assert!(
            !lists.image_domains.contains(&shared.to_string()),
            "shared {shared} in images"
        );
        assert!(
            !lists.string_domains.contains(&shared.to_string()),
            "shared {shared} in strings"
        );
    }

    // 3. Multi-list domains NOT in whiteListedAssetUrls / vrchat core MUST be in rest_domains
    assert!(lists.rest_domains.contains(&"*.github.io".to_string()));
    assert!(lists.rest_domains.contains(&"ciel.topaz.chat".to_string()));

    // 4. whiteListedAssetUrls & vrchat core domains MUST NOT be in ANY list, including rest
    for asset in &[
        "assets.vrchat.com",
        "dbinj8iahsbec.cloudfront.net",
        "redirect.vrchat.com",
    ] {
        assert!(
            !lists.video_domains.contains(&asset.to_string()),
            "asset {asset} in video"
        );
        assert!(
            !lists.image_domains.contains(&asset.to_string()),
            "asset {asset} in images"
        );
        assert!(
            !lists.string_domains.contains(&asset.to_string()),
            "asset {asset} in strings"
        );
        assert!(
            !lists.rest_domains.contains(&asset.to_string()),
            "asset {asset} in rest"
        );
    }
}

#[test]
fn parse_vrchat_and_community_config_json_merges_smartly() {
    let vrc_json = r#"{
        "urlList": ["*.youtube.com", "twitch.tv"],
        "imageHostUrlList": ["i.imgur.com"],
        "stringHostUrlList": ["pastebin.com"],
        "whiteListedAssetUrls": ["https://assets.vrchat.com/adminfiles/"]
    }"#;

    let community_json = r#"{
        "urlList": ["custom-video.com", "shared-multipurpose.com", "assets.vrchat.com"],
        "imageHostUrlList": ["custom-image.com", "shared-multipurpose.com"],
        "stringHostUrlList": ["custom-string.com"]
    }"#;

    let inputs = vec![RawBlocklistInput {
        name: "Community".to_string(),
        json: community_json.to_string(),
        enabled: true,
    }];
    let lists =
        parse_all_domain_lists(vrc_json, &inputs).expect("merging configs should succeed");

    // 1. Community video and image domains are merged
    assert!(lists.video_domains.contains(&"custom-video.com".to_string()));
    assert!(lists.image_domains.contains(&"custom-image.com".to_string()));
    assert!(lists.string_domains.contains(&"custom-string.com".to_string()));

    // 2. Multi-list domain shared-multipurpose.com goes to rest, not pure video or image
    assert!(!lists
        .video_domains
        .contains(&"shared-multipurpose.com".to_string()));
    assert!(!lists
        .image_domains
        .contains(&"shared-multipurpose.com".to_string()));
    assert!(lists
        .rest_domains
        .contains(&"shared-multipurpose.com".to_string()));

    // 3. assets.vrchat.com is protected and not in ANY category
    assert!(!lists.video_domains.contains(&"assets.vrchat.com".to_string()));
    assert!(!lists.rest_domains.contains(&"assets.vrchat.com".to_string()));

    // 4. Fallback with community blocklists has zero category overlap
    let fallback_comm = build_domain_lists_fallback_with_community(true);
    let v_set: HashSet<_> = fallback_comm.video_domains.iter().cloned().collect();
    let i_set: HashSet<_> = fallback_comm.image_domains.iter().cloned().collect();
    let s_set: HashSet<_> = fallback_comm.string_domains.iter().cloned().collect();
    let r_set: HashSet<_> = fallback_comm.rest_domains.iter().cloned().collect();
    assert!(v_set.is_disjoint(&i_set));
    assert!(v_set.is_disjoint(&s_set));
    assert!(v_set.is_disjoint(&r_set));
    assert!(i_set.is_disjoint(&s_set));
    assert!(i_set.is_disjoint(&r_set));
    assert!(s_set.is_disjoint(&r_set));
}

#[tokio::test]
async fn community_config_url_and_dynamic_loading() {
    assert_eq!(
        crate::config::DEFAULT_COMMUNITY_CONFIG_URL,
        "https://github.com/Bluscream/lvr/raw/refs/heads/main/assets/lists/community.json"
    );

    // When community blocklists are disabled, only VRChat default fallback lists are present
    let no_comm = build_domain_lists_fallback_with_community(false);
    assert!(!no_comm.video_domains.contains(&"andre-stinkt.de".to_string()));

    // When reloading with false, ACTIVE_DOMAIN_LISTS is updated without community lists
    let cfg = crate::config::DomainBlockConfig {
        load_community_blocklists: false,
        community_sources: crate::config::default_community_sources(),
    };
    let reloaded_no_comm = reload_domain_lists_with_config(&cfg).await;
    assert!(!reloaded_no_comm
        .video_domains
        .contains(&"andre-stinkt.de".to_string()));
}

#[test]
fn community_config_cache_freshness_check() {
    let temp_dir = std::env::temp_dir();
    let fresh_file = temp_dir.join("lvr_test_fresh_config.json");
    let _ = fs::write(&fresh_file, r#"{"urlList": ["fresh.com"]}"#);

    assert!(is_file_fresh(&fresh_file, COMMUNITY_CACHE_MAX_AGE));
    assert!(!is_file_fresh(
        &fresh_file,
        std::time::Duration::from_nanos(1)
    ));

    let missing_file = temp_dir.join("lvr_test_missing_config.json");
    let _ = fs::remove_file(&missing_file);
    assert!(!is_file_fresh(&missing_file, COMMUNITY_CACHE_MAX_AGE));

    let _ = fs::remove_file(&fresh_file);
}

#[test]
fn parse_all_domain_lists_tracks_per_list_and_total_stats() {
    let vrc_json = r#"{
        "urlList": ["youtube.com", "twitch.tv", "shared.com"],
        "imageHostUrlList": ["imgur.com", "shared.com"],
        "stringHostUrlList": ["pastebin.com"]
    }"#;

    let comm1_json = r#"{
        "urlList": ["custom-video1.com", "shared.com"],
        "imageHostUrlList": ["custom-image1.com"]
    }"#;

    let comm2_json = r#"{
        "urlList": ["custom-video2.com"],
        "stringHostUrlList": ["custom-string2.com", "shared.com"]
    }"#;

    let inputs = vec![
        RawBlocklistInput {
            name: "Community One".to_string(),
            json: comm1_json.to_string(),
            enabled: true,
        },
        RawBlocklistInput {
            name: "Community Two".to_string(),
            json: comm2_json.to_string(),
            enabled: true,
        },
        RawBlocklistInput {
            name: "Community Disabled".to_string(),
            json: String::new(),
            enabled: false,
        },
    ];

    let lists =
        parse_all_domain_lists(vrc_json, &inputs).expect("parse multiple community lists");

    // "shared.com" appears in both video & image (and string in comm2) -> moved to Rest
    assert!(lists.rest_domains.contains(&"shared.com".to_string()));
    assert!(!lists.video_domains.contains(&"shared.com".to_string()));

    // Check list_stats length: Official + 3 configured lists = 4 stats
    assert_eq!(lists.list_stats.len(), 4);

    // Official stats
    let off = &lists.list_stats[0];
    assert_eq!(off.name, "Official");
    assert!(off.enabled);
    assert_eq!(off.counts.video, 2); // youtube.com, twitch.tv
    assert_eq!(off.counts.image, 1); // imgur.com
    assert_eq!(off.counts.string, 1); // pastebin.com
    assert_eq!(off.counts.rest, 1); // shared.com
    assert_eq!(off.counts.total(), 5);

    // Community One stats
    let c1 = &lists.list_stats[1];
    assert_eq!(c1.name, "Community One");
    assert!(c1.enabled);
    assert_eq!(c1.counts.video, 1); // custom-video1.com
    assert_eq!(c1.counts.image, 1); // custom-image1.com
    assert_eq!(c1.counts.rest, 1); // shared.com
    assert_eq!(c1.counts.total(), 3);

    // Community Two stats
    let c2 = &lists.list_stats[2];
    assert_eq!(c2.name, "Community Two");
    assert!(c2.enabled);
    assert_eq!(c2.counts.video, 1); // custom-video2.com
    assert_eq!(c2.counts.string, 1); // custom-string2.com
    assert_eq!(c2.counts.rest, 1); // shared.com
    assert_eq!(c2.counts.total(), 3);

    // Disabled community list
    let c3 = &lists.list_stats[3];
    assert_eq!(c3.name, "Community Disabled");
    assert!(!c3.enabled);
    assert_eq!(c3.counts.total(), 0);

    // Total counts
    assert_eq!(lists.total_counts.video, 4); // youtube.com, twitch.tv, custom-video1.com, custom-video2.com
    assert_eq!(lists.total_counts.image, 2); // imgur.com, custom-image1.com
    assert_eq!(lists.total_counts.string, 2); // pastebin.com, custom-string2.com
    assert_eq!(lists.total_counts.rest, 1); // shared.com
    assert_eq!(lists.total_counts.total(), 9);
}

#[test]
fn custom_category_expansion_and_non_urllist_key_stripping() {
    let vrc_json = r#"{
        "urlList": ["youtube.com"],
        "imageHostUrlList": ["imgur.com"],
        "stringHostUrlList": ["pastebin.com"],
        "availableLanguages": ["English", "French", "German"],
        "availableLanguageCodes": ["en", "fr", "de"],
        "clientApiKey": "some-key",
        "analysisMaxRetries": 14
    }"#;

    let comm_json = r#"{
        "$schema": "./blocklist.schema.json",
        "Analytics": ["api.amplitude.com", "analytics.unity3d.com"],
        "Telemetry": ["telemetry.example.com"],
        "invalidKey": ["not a domain", "also not domain"]
    }"#;

    let inputs = vec![RawBlocklistInput {
        name: "Community".to_string(),
        json: comm_json.to_string(),
        enabled: true,
    }];

    let lists = parse_all_domain_lists(vrc_json, &inputs).expect("parse custom categories");

    // Check custom categories are parsed
    assert!(lists.custom_categories.contains(&"Analytics".to_string()));
    assert!(lists.custom_categories.contains(&"Telemetry".to_string()));

    // Check non-urllist keys are stripped
    assert!(!lists
        .custom_categories
        .contains(&"availableLanguages".to_string()));
    assert!(!lists
        .custom_categories
        .contains(&"availableLanguageCodes".to_string()));
    assert!(!lists
        .custom_categories
        .contains(&"clientApiKey".to_string()));
    assert!(!lists.custom_categories.contains(&"invalidKey".to_string()));

    // Check all_categories ordering: Videos, Images, Strings, custom..., Rest
    let all_cats = lists.all_categories();
    assert_eq!(all_cats[0], BlockCategory::Video);
    assert_eq!(all_cats[1], BlockCategory::Images);
    assert_eq!(all_cats[2], BlockCategory::Strings);
    assert!(all_cats.contains(&BlockCategory::Custom("Analytics".to_string())));
    assert!(all_cats.contains(&BlockCategory::Custom("Telemetry".to_string())));
    assert_eq!(*all_cats.last().unwrap(), BlockCategory::Rest);

    // Check domains for custom category
    let analytics_domains =
        lists.domains_for_category(&BlockCategory::Custom("Analytics".to_string()));
    assert!(analytics_domains.contains(&"api.amplitude.com".to_string()));
    assert!(analytics_domains.contains(&"analytics.unity3d.com".to_string()));

    // Check tags and header parsing
    let analytics_cat = BlockCategory::Custom("Analytics".to_string());
    assert_eq!(
        analytics_cat.header_tag(),
        "# ----- BEGIN LVR ANALYTICS BLOCK -----"
    );
    assert_eq!(
        analytics_cat.footer_tag(),
        "# ----- END LVR ANALYTICS BLOCK -----"
    );
    assert_eq!(
        BlockCategory::from_tag("# ----- BEGIN LVR ANALYTICS BLOCK -----"),
        Some(analytics_cat)
    );
}

#[test]
fn local_community_json_loads_analytics_category() {
    let lists = build_domain_lists_fallback_with_community(true);
    assert_eq!(lists.custom_categories, vec!["Analytics".to_string()]);
    assert!(!lists
        .custom_categories
        .contains(&"photonNameserverOverrides".to_string()));
    assert!(!lists
        .custom_categories
        .contains(&"propComponentList".to_string()));
    let analytics_domains =
        lists.domains_for_category(&BlockCategory::Custom("Analytics".to_string()));
    assert!(analytics_domains.contains(&"api.amplitude.com".to_string()));
    assert!(analytics_domains.len() >= 100);

    let official_stats = lists
        .list_stats
        .iter()
        .find(|s| s.name == "Official")
        .expect("Official stats");
    assert_eq!(
        official_stats
            .counts
            .custom
            .get("photonNameserverOverrides"),
        None
    );
    assert_eq!(official_stats.counts.custom.get("propComponentList"), None);
}

#[test]
fn official_vrc_config_never_produces_custom_categories() {
    let vrc_json = r#"{
        "urlList": ["youtube.com"],
        "imageHostUrlList": ["imgur.com"],
        "stringHostUrlList": ["pastebin.com"],
        "photonNameserverOverrides": ["ns.photonengine.io"],
        "propComponentList": ["VRC.SDK3.Components.VRCMirrorReflection"],
        "newFutureArrayAddedByVrc": ["unexpected.vrchat.com", "other.domain.com"],
        "anotherKey": ["test.org"]
    }"#;
    let lists = parse_all_domain_lists(vrc_json, &[]).expect("parse official vrc");
    assert!(
        lists.custom_categories.is_empty(),
        "Official config must NEVER create custom categories, got: {:?}",
        lists.custom_categories
    );
}

#[test]
fn domain_validation_rejects_csharp_types_and_code_identifiers() {
    assert!(!is_valid_domain_or_glob(
        "VRC.SDK3.Components.VRCMirrorReflection"
    ));
    assert!(!is_valid_domain_or_glob(
        "UnityEngine.Networking.UnityWebRequest"
    ));
    assert!(!is_valid_domain_or_glob("System.Collections.Generic.List"));
    assert!(!is_valid_domain_or_glob(
        "vrc.sdk3.components.vrcmirrorreflection"
    ));
    assert!(is_valid_domain_or_glob("ns.photonengine.io"));
    assert!(is_valid_domain_or_glob("api.amplitude.com"));
    assert!(is_valid_domain_or_glob("*.vrchat.cloud"));
}

#[test]
fn hosts_file_has_zero_duplicates_and_always_blocks_www_equivalent() {
    let temp_dir = std::env::temp_dir().join(format!("lvr_test_hosts_{}", std::process::id()));
    let _ = fs::create_dir_all(&temp_dir);
    let mut state = BlockState {
        video_blocked: true,
        images_blocked: true,
        strings_blocked: true,
        rest_blocked: true,
        custom_blocked: std::collections::BTreeMap::new(),
    };
    state.set_blocked(&BlockCategory::Custom("Analytics".to_string()), true);

    sync_all(&temp_dir, &state).expect("sync_all to temp prefix");

    let hosts_path = prefix_hosts_path(&temp_dir);
    let content = fs::read_to_string(&hosts_path).expect("read hosts file");

    let mut seen = std::collections::HashSet::new();
    let mut category_by_host = std::collections::HashMap::new();
    let mut current_cat = "UNKNOWN";

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("# ----- BEGIN LVR ") {
            current_cat = trimmed;
            continue;
        }
        if trimmed.starts_with("# ----- END LVR ") {
            current_cat = "NONE";
            continue;
        }
        if trimmed.starts_with("0.0.0.0 ") {
            let parts: Vec<&str> = trimmed.split('#').collect();
            assert_eq!(
                parts.len(),
                2,
                "Every hosts entry must have a # comment showing its source list: {trimmed}"
            );
            let comment = parts[1].trim();
            assert!(
                comment.contains("Official") || comment.contains("Community"),
                "Comment '{comment}' does not contain expected list name in {trimmed}"
            );

            let without_comment = parts[0].trim();
            if let Some(host) = without_comment.strip_prefix("0.0.0.0 ") {
                let host = host.trim().to_lowercase();
                assert!(
                    seen.insert(host.clone()),
                    "Duplicate host found in hosts file: {host}"
                );
                if let Some(prev_cat) = category_by_host.insert(host.clone(), current_cat) {
                    panic!("Host {host} was emitted in both {prev_cat} and {current_cat}!");
                }
            }
        }
    }

    // Verify www equivalent rule: for every host foo, www.foo must also exist
    for host in &seen {
        if let Some(stripped) = host.strip_prefix("www.") {
            assert!(
                seen.contains(stripped),
                "Missing base domain {stripped} for www host {host}"
            );
        } else {
            let with_www = format!("www.{host}");
            assert!(
                seen.contains(&with_www),
                "Missing www equivalent {with_www} for host {host}"
            );
        }
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn all_categories_are_mutually_exclusive_by_canonical_key() {
    let lists = build_domain_lists_fallback_with_community(true);
    let mut cat_map: std::collections::HashMap<String, BlockCategory> =
        std::collections::HashMap::new();

    for cat in lists.all_categories() {
        for domain in lists.domains_for_category(&cat) {
            let key = canonical_domain_key(domain);
            if let Some(existing_cat) = cat_map.get(&key) {
                if existing_cat != &cat {
                    panic!(
                        "Domain key '{key}' (from '{domain}') exists in both {:?} and {:?}! Categories must be mutually exclusive.",
                        existing_cat, cat
                    );
                }
            } else {
                cat_map.insert(key, cat.clone());
            }
        }
    }
}

#[test]
fn domain_sources_tracking_records_originating_lists() {
    let lists = build_domain_lists_fallback_with_community(true);
    let yt_sources = lists.sources_for_domain("youtube.com");
    assert!(yt_sources.contains(&"Official".to_string()));
    assert!(yt_sources.contains(&"Community".to_string()));

    let amp_sources = lists.sources_for_domain("api.amplitude.com");
    assert_eq!(amp_sources, &["Community".to_string()]);
}

#[test]
#[ignore]
fn sync_real_vrc_prefix_hosts() {
    let prefix = std::path::Path::new("/run/media/system/Data/Games/Steam/steamapps/compatdata/438100");
    if prefix.is_dir() {
        let state = read_block_state(prefix);
        sync_all(prefix, &state).expect("sync to real prefix");
    }
}


