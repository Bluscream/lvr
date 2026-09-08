use std::collections::{BTreeMap, HashSet};

use anyhow::Result;

use super::types::{BlockCategory, BlocklistStats, CategoryCounts, DomainLists, RawBlocklistInput};

/// Known non-urllist keys in VRChat config to strip during blocklist parsing.
pub const KNOWN_NON_URLLIST_KEYS: &[&str] = &[
    "$schema",
    "CampaignStatus",
    "DisableBackgroundPreloads",
    "LocationGiftingNonSubPrioEnabled",
    "VoiceEnableDegradation",
    "VoiceEnableReceiverLimiting",
    "accessLogsUrls",
    "address",
    "ageVerificationInviteVisible",
    "ageVerificationP",
    "ageVerificationStatusVisible",
    "analysisMaxRetries",
    "analysisRetryInterval",
    "analyticsSegment_NewUI_PctOfUsers",
    "analyticsSegment_NewUI_Salt",
    "announcements",
    "arrayEspresso",
    "audioConfig",
    "availableLanguageCodes",
    "availableLanguages",
    "avatarPerfLimiter",
    "chatboxLogBufferSeconds",
    "clientApiKey",
    "clientBPSCeiling",
    "clientDisconnectTimeout",
    "clientMaxDatagrams",
    "clientNetDispatchThread",
    "clientNetDispatchThreadMobile",
    "clientQR",
    "clientReservedPlayerBPS",
    "clientSentCountAllowance",
    "clientUseAck2",
    "constants",
    "contactEmail",
    "copyrightEmail",
    "copyrightFormUrl",
    "currentPrivacyVersion",
    "currentTOSVersion",
    "defaultAvatar",
    "defaultStickerSet",
    "devLanguageCodes",
    "devSdkUrl",
    "devSdkVersion",
    "dis-countdown",
    "disableAVProInProton",
    "disableAvatarCopying",
    "disableAvatarGating",
    "disableCaptcha",
    "disableCommunityLabs",
    "disableCommunityLabsPromotion",
    "disableEmail",
    "disableEventStream",
    "disableFeedbackGating",
    "disableFrontendBuilds",
    "disableGiftDrops",
    "disableHello",
    "disableOculusSubs",
    "disableRegistration",
    "disableSteamNetworking",
    "disableTwoFactorAuth",
    "disableUdon",
    "disableUpgradeAccount",
    "downloadLinkWindows",
    "downloadUrls",
    "dwellBackoffSchedule",
    "dynamicWorldRows",
    "economyLedgerMode",
    "economyPauseEnd",
    "economyPauseStart",
    "economyPurchaseRepairEnabled",
    "economyState",
    "enableVRCPlusWorldLists",
    "eventShelfCampaigns",
    "events",
    "forceUseLatestWorld",
    "giftDropsConfig",
    "homeContentUrlPrefix",
    "homeWorldId",
    "hubWorldId",
    "imagePlacementConfig",
    "imagePlacementRules",
    "imagePlacements",
    "instanceQueueDropProbability",
    "instanceQueueMaxDwellTime",
    "instanceQueueMaxWaitTime",
    "instanceQueueRetryInterval",
    "invitationExpiryMinutes",
    "itemDropProbabilityMultiplier",
    "jobs",
    "launchWarningUrl",
    "minimumUpdateInterval",
    "moderationEmail",
    "notices",
    "pdfUrlList",
    "player-hud-banner",
    "player-hud-banner-sub",
    "player-hud-banner-url",
    "player-hud-banner-url-sub",
    "player-modal-banner",
    "player-modal-banner-sub",
    "player-modal-banner-url",
    "player-modal-banner-url-sub",
    "podcast",
    "promoNotificationId",
    "promoNotificationInterval",
    "promoNotificationTitle",
    "promoNotificationUrl",
    "questionnaireExpiryMinutes",
    "redisRateLimitMinutes",
    "reportCategories",
    "reportReasons",
    "reportReasonsEnforcement",
    "reportReasonsGroup",
    "safetyGatingMultiplier",
    "searchRateLimitCount",
    "searchRateLimitMinutes",
    "sentimentAnalysisThreshold",
    "sentimentThresholds",
    "serverName",
    "shareUrl",
    "stallDetectionMinutes",
    "supportEmail",
    "timeOutWorldId",
    "toastMessage",
    "tutorialWorldId",
    "updateRateMsCommunity",
    "updateRateMsFriends",
    "updateRateMsUdon",
    "updateRateMsUdonManual",
    "uploadAnalysisPercent",
    "useReliableUdpForVoice",
    "use_void_requiem_core",
    "varietyBoxPriority",
    "viveWindowsUrl",
    "voiceConfig",
    "voiceMaxPlaybackSourcesMobile",
    "voiceMaxPlaybackSourcesPC",
    "websocketMaxFriendsRefreshDelay",
    "websocketQuickReconnectTime",
    "websocketReconnectMaxDelay",
];

pub fn clean_domain_entry(entry: &str) -> String {
    let mut s = entry.trim();
    if let Some(pos) = s.find("://") {
        s = &s[pos + 3..];
    }
    if let Some(pos) = s.find('/') {
        s = &s[..pos];
    }
    if let Some(pos) = s.find(':') {
        s = &s[..pos];
    }
    s.trim().to_lowercase()
}

pub fn is_valid_domain_or_glob(entry: &str) -> bool {
    let cleaned = clean_domain_entry(entry);
    if cleaned.is_empty() {
        return false;
    }
    if cleaned == "localhost" {
        return true;
    }
    if cleaned.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    let bare = cleaned.trim_start_matches("*.");
    if bare.is_empty() || bare.starts_with('.') || bare.ends_with('.') || bare.contains("..") {
        return false;
    }
    if !bare.contains('.') {
        return false;
    }
    for c in bare.chars() {
        if !c.is_ascii_alphanumeric() && c != '.' && c != '-' && c != '_' {
            return false;
        }
    }
    let parts: Vec<&str> = bare.split('.').collect();
    if parts.len() < 2 {
        return false;
    }
    for part in &parts {
        if part.is_empty() || part.starts_with('-') || part.ends_with('-') {
            return false;
        }
    }
    let tld = parts[parts.len() - 1];
    if tld.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    true
}

pub fn map_key_to_category(key: &str) -> Option<BlockCategory> {
    if KNOWN_NON_URLLIST_KEYS
        .iter()
        .any(|k| k.eq_ignore_ascii_case(key))
    {
        return None;
    }
    match key {
        "urlList" | "Videos" | "video" | "videos" => Some(BlockCategory::Video),
        "imageHostUrlList" | "Images" | "image" | "images" => Some(BlockCategory::Images),
        "stringHostUrlList" | "Strings" | "string" | "strings" => Some(BlockCategory::Strings),
        "whiteListedAssetUrls" => None,
        other => {
            let trimmed = other.trim();
            if trimmed.is_empty() || trimmed.starts_with('$') {
                None
            } else {
                Some(BlockCategory::Custom(trimmed.to_string()))
            }
        }
    }
}

struct CommData {
    name: String,
    enabled: bool,
    domains_by_cat: BTreeMap<BlockCategory, HashSet<String>>,
}

struct ClassifiedDomains {
    video_domains: Vec<String>,
    image_domains: Vec<String>,
    string_domains: Vec<String>,
    custom_domains: BTreeMap<String, Vec<String>>,
    rest_domains: Vec<String>,
    custom_categories: Vec<String>,
}

fn extract_protected_asset_urls(
    obj: &serde_json::Map<String, serde_json::Value>,
    protected: &mut HashSet<String>,
) {
    if let Some(asset_urls) = obj.get("whiteListedAssetUrls").and_then(|v| v.as_array()) {
        for item in asset_urls {
            if let Some(s) = item.as_str() {
                let cleaned = clean_domain_entry(s);
                if !cleaned.is_empty() {
                    protected.insert(cleaned.clone());
                    let bare = cleaned.trim_start_matches("*.");
                    protected.insert(bare.to_string());
                    protected.insert(format!("*.{bare}"));
                }
            }
        }
    }
}

fn parse_official_vrc_blocklist(
    vrc_json_str: &str,
    protected: &mut HashSet<String>,
    raw_domains_by_cat: &mut BTreeMap<BlockCategory, HashSet<String>>,
) -> Result<BTreeMap<BlockCategory, HashSet<String>>> {
    let parsed_vrc: serde_json::Value = serde_json::from_str(vrc_json_str)?;
    let mut official_domains_by_cat: BTreeMap<BlockCategory, HashSet<String>> = BTreeMap::new();

    if let Some(vrc_obj) = parsed_vrc.as_object() {
        extract_protected_asset_urls(vrc_obj, protected);

        for (key, val) in vrc_obj {
            if let Some(cat) = map_key_to_category(key)
                && let Some(arr) = val.as_array()
            {
                for item in arr {
                    if let Some(s) = item.as_str()
                        && is_valid_domain_or_glob(s)
                    {
                        let cleaned = clean_domain_entry(s);
                        if !cleaned.is_empty() {
                            official_domains_by_cat
                                .entry(cat.clone())
                                .or_default()
                                .insert(cleaned.clone());
                            raw_domains_by_cat
                                .entry(cat.clone())
                                .or_default()
                                .insert(cleaned);
                        }
                    }
                }
            }
        }
    }

    Ok(official_domains_by_cat)
}

fn parse_community_blocklists(
    community_lists: &[RawBlocklistInput],
    protected: &mut HashSet<String>,
    raw_domains_by_cat: &mut BTreeMap<BlockCategory, HashSet<String>>,
) -> Vec<CommData> {
    let mut comm_data: Vec<CommData> = Vec::new();

    for item in community_lists {
        if !item.enabled || item.json.trim().is_empty() {
            comm_data.push(CommData {
                name: item.name.clone(),
                enabled: false,
                domains_by_cat: BTreeMap::new(),
            });
            continue;
        }

        let parsed: serde_json::Value =
            serde_json::from_str(&item.json).unwrap_or(serde_json::Value::Null);
        let mut list_by_cat: BTreeMap<BlockCategory, HashSet<String>> = BTreeMap::new();

        if let Some(obj) = parsed.as_object() {
            extract_protected_asset_urls(obj, protected);

            for (key, val) in obj {
                if let Some(cat) = map_key_to_category(key)
                    && let Some(arr) = val.as_array()
                {
                    for entry in arr {
                        if let Some(s) = entry.as_str()
                            && is_valid_domain_or_glob(s)
                        {
                            let cleaned = clean_domain_entry(s);
                            if !cleaned.is_empty() {
                                list_by_cat
                                    .entry(cat.clone())
                                    .or_default()
                                    .insert(cleaned.clone());
                                raw_domains_by_cat
                                    .entry(cat.clone())
                                    .or_default()
                                    .insert(cleaned);
                            }
                        }
                    }
                }
            }
        }

        comm_data.push(CommData {
            name: item.name.clone(),
            enabled: true,
            domains_by_cat: list_by_cat,
        });
    }

    comm_data
}

fn ensure_core_asset_wildcards(protected: &mut HashSet<String>) {
    for base in &[
        "assets.vrchat.com",
        "vrchat.com",
        "*.vrchat.com",
        "vrchat.cloud",
        "*.vrchat.cloud",
        "dbinj8iahsbec.cloudfront.net",
        "*.dbinj8iahsbec.cloudfront.net",
    ] {
        protected.insert(base.to_string());
        let bare = base.trim_start_matches("*.");
        protected.insert(bare.to_string());
    }
}

fn find_multi_list_domains(
    raw_domains_by_cat: &BTreeMap<BlockCategory, HashSet<String>>,
) -> HashSet<String> {
    let mut in_multiple = HashSet::new();
    let mut all_unique_domains: HashSet<String> = HashSet::new();
    for set in raw_domains_by_cat.values() {
        all_unique_domains.extend(set.iter().cloned());
    }

    for d in &all_unique_domains {
        let mut category_count = 0;
        for set in raw_domains_by_cat.values() {
            if set.contains(d) {
                category_count += 1;
            }
        }
        if category_count >= 2 {
            in_multiple.insert(d.clone());
        }
    }
    in_multiple
}

fn is_vrc_or_asset_protected(d: &str, protected: &HashSet<String>) -> bool {
    let bare = d.trim_start_matches("*.");
    if protected.contains(d)
        || protected.contains(bare)
        || protected.contains(&format!("*.{bare}"))
    {
        return true;
    }
    bare == "vrchat.com"
        || bare.ends_with(".vrchat.com")
        || bare == "vrchat.cloud"
        || bare.ends_with(".vrchat.cloud")
        || bare == "dbinj8iahsbec.cloudfront.net"
        || bare.ends_with(".dbinj8iahsbec.cloudfront.net")
}

fn classify_domains(
    raw_domains_by_cat: &BTreeMap<BlockCategory, HashSet<String>>,
    protected: &HashSet<String>,
) -> ClassifiedDomains {
    let in_multiple = find_multi_list_domains(raw_domains_by_cat);

    let is_pure_category_protected = |d: &str| -> bool {
        in_multiple.contains(d) || is_vrc_or_asset_protected(d, protected)
    };

    let mut video_domains: Vec<String> = raw_domains_by_cat
        .get(&BlockCategory::Video)
        .map(|s| {
            s.iter()
                .filter(|d| !is_pure_category_protected(d))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut image_domains: Vec<String> = raw_domains_by_cat
        .get(&BlockCategory::Images)
        .map(|s| {
            s.iter()
                .filter(|d| !is_pure_category_protected(d))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut string_domains: Vec<String> = raw_domains_by_cat
        .get(&BlockCategory::Strings)
        .map(|s| {
            s.iter()
                .filter(|d| !is_pure_category_protected(d))
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    let mut custom_domains: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut custom_categories_set: HashSet<String> = HashSet::new();

    for (cat, domains) in raw_domains_by_cat {
        if let BlockCategory::Custom(name) = cat {
            let pure: Vec<String> = domains
                .iter()
                .filter(|d| !is_pure_category_protected(d))
                .cloned()
                .collect();
            custom_domains.insert(name.clone(), pure);
            custom_categories_set.insert(name.clone());
        }
    }

    let mut rest_domains: Vec<String> = in_multiple
        .into_iter()
        .filter(|d| !is_vrc_or_asset_protected(d, protected))
        .collect();

    video_domains.sort();
    image_domains.sort();
    string_domains.sort();
    rest_domains.sort();
    for v in custom_domains.values_mut() {
        v.sort();
    }

    let mut custom_categories: Vec<String> = custom_categories_set.into_iter().collect();
    custom_categories.sort();

    ClassifiedDomains {
        video_domains,
        image_domains,
        string_domains,
        custom_domains,
        rest_domains,
        custom_categories,
    }
}

fn count_map_categories(
    domains_by_cat: &BTreeMap<BlockCategory, HashSet<String>>,
    v_set: &HashSet<&str>,
    i_set: &HashSet<&str>,
    s_set: &HashSet<&str>,
    r_set: &HashSet<&str>,
    custom_sets: &BTreeMap<String, HashSet<&str>>,
) -> CategoryCounts {
    let mut counts = CategoryCounts::default();
    if let Some(set) = domains_by_cat.get(&BlockCategory::Video) {
        counts.video = set.iter().filter(|d| v_set.contains(d.as_str())).count();
    }
    if let Some(set) = domains_by_cat.get(&BlockCategory::Images) {
        counts.image = set.iter().filter(|d| i_set.contains(d.as_str())).count();
    }
    if let Some(set) = domains_by_cat.get(&BlockCategory::Strings) {
        counts.string = set.iter().filter(|d| s_set.contains(d.as_str())).count();
    }
    for (name, c_set) in custom_sets {
        if let Some(set) = domains_by_cat.get(&BlockCategory::Custom(name.clone())) {
            let count = set.iter().filter(|d| c_set.contains(d.as_str())).count();
            counts.set_for_category(&BlockCategory::Custom(name.clone()), count);
        }
    }
    let mut all_domains = HashSet::new();
    for set in domains_by_cat.values() {
        all_domains.extend(set.iter());
    }
    counts.rest = all_domains
        .iter()
        .filter(|d| r_set.contains(d.as_str()))
        .count();
    counts
}

fn compute_list_stats(
    official_domains_by_cat: &BTreeMap<BlockCategory, HashSet<String>>,
    comm_data: Vec<CommData>,
    classified: &ClassifiedDomains,
) -> (Vec<BlocklistStats>, CategoryCounts) {
    let v_set: HashSet<&str> = classified
        .video_domains
        .iter()
        .map(|s| s.as_str())
        .collect();
    let i_set: HashSet<&str> = classified
        .image_domains
        .iter()
        .map(|s| s.as_str())
        .collect();
    let s_set: HashSet<&str> = classified
        .string_domains
        .iter()
        .map(|s| s.as_str())
        .collect();
    let r_set: HashSet<&str> = classified.rest_domains.iter().map(|s| s.as_str()).collect();
    let mut custom_sets: BTreeMap<String, HashSet<&str>> = BTreeMap::new();
    for (name, list) in &classified.custom_domains {
        custom_sets.insert(name.clone(), list.iter().map(|s| s.as_str()).collect());
    }

    let mut list_stats = Vec::new();

    // 1. Official stats
    let off_counts = count_map_categories(
        official_domains_by_cat,
        &v_set,
        &i_set,
        &s_set,
        &r_set,
        &custom_sets,
    );
    list_stats.push(BlocklistStats {
        name: "Official".to_string(),
        counts: off_counts,
        enabled: true,
    });

    // 2. Community stats per list
    for comm in comm_data {
        if comm.enabled {
            let counts = count_map_categories(
                &comm.domains_by_cat,
                &v_set,
                &i_set,
                &s_set,
                &r_set,
                &custom_sets,
            );
            list_stats.push(BlocklistStats {
                name: comm.name,
                counts,
                enabled: true,
            });
        } else {
            list_stats.push(BlocklistStats {
                name: comm.name,
                counts: CategoryCounts::default(),
                enabled: false,
            });
        }
    }

    let mut total_counts = CategoryCounts {
        video: classified.video_domains.len(),
        image: classified.image_domains.len(),
        string: classified.string_domains.len(),
        custom: BTreeMap::new(),
        rest: classified.rest_domains.len(),
    };
    for (name, domains) in &classified.custom_domains {
        total_counts.set_for_category(&BlockCategory::Custom(name.clone()), domains.len());
    }

    (list_stats, total_counts)
}

/// Parse VRChat config and multiple community blocklists, smartly enforcing safety invariants
/// and tracking blocked domain counts per list and total.
pub fn parse_all_domain_lists(
    vrc_json_str: &str,
    community_lists: &[RawBlocklistInput],
) -> Result<DomainLists> {
    let mut protected: HashSet<String> = HashSet::new();
    let mut raw_domains_by_cat: BTreeMap<BlockCategory, HashSet<String>> = BTreeMap::new();

    let official_domains_by_cat =
        parse_official_vrc_blocklist(vrc_json_str, &mut protected, &mut raw_domains_by_cat)?;
    let comm_data =
        parse_community_blocklists(community_lists, &mut protected, &mut raw_domains_by_cat);

    ensure_core_asset_wildcards(&mut protected);

    let classified = classify_domains(&raw_domains_by_cat, &protected);

    #[cfg(test)]
    let mut protected_list: Vec<String> = protected.into_iter().collect();
    #[cfg(test)]
    protected_list.sort();

    let (list_stats, total_counts) =
        compute_list_stats(&official_domains_by_cat, comm_data, &classified);

    Ok(DomainLists {
        video_domains: classified.video_domains,
        image_domains: classified.image_domains,
        string_domains: classified.string_domains,
        custom_domains: classified.custom_domains,
        rest_domains: classified.rest_domains,
        #[cfg(test)]
        protected_domains: protected_list,
        list_stats,
        total_counts,
        custom_categories: classified.custom_categories,
    })
}
