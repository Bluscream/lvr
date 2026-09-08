//! Modular VRChat domain and media blocking system for LinuxVR (`lvr`).
//!
//! Provides granular blocking for:
//! - Video players (`urlList` pure video domains + yt-dlp stubbing)
//! - Image loading (`imageHostUrlList` pure image domains)
//! - String loading (`stringHostUrlList` pure string domains)
//!
//! Safety invariant:
//! Any domain that appears in two or more lists (e.g. `*.github.io`, `assets.vrchat.com`,
//! `*.v-market.work`, `*.poly.jp`, `ciel.topaz.chat`) is considered a shared asset
//! and is NEVER blocked by ANY toggle.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// The categories of network/media content that can be blocked in VRChat.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BlockCategory {
    Video,
    Images,
    Strings,
    Custom(String),
    Rest,
}

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

/// Embedded copy of VRChat remote config (`https://api.vrchat.cloud/api/1/config`)
/// captured at compile time as a fallback whenever the network is unavailable.
const EMBEDDED_FALLBACK_CONFIG: &str = include_str!("../assets/vrchat_config_fallback.json");

/// Detailed domain counts per category.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryCounts {
    pub video: usize,
    pub image: usize,
    pub string: usize,
    #[serde(default)]
    pub custom: BTreeMap<String, usize>,
    pub rest: usize,
}

impl CategoryCounts {
    pub fn total(&self) -> usize {
        self.video + self.image + self.string + self.custom.values().sum::<usize>() + self.rest
    }

    pub fn for_category(&self, cat: &BlockCategory) -> usize {
        match cat {
            BlockCategory::Video => self.video,
            BlockCategory::Images => self.image,
            BlockCategory::Strings => self.string,
            BlockCategory::Rest => self.rest,
            BlockCategory::Custom(name) => self
                .custom
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, &c)| c)
                .unwrap_or(0),
        }
    }

    pub fn set_for_category(&mut self, cat: &BlockCategory, count: usize) {
        match cat {
            BlockCategory::Video => self.video = count,
            BlockCategory::Images => self.image = count,
            BlockCategory::Strings => self.string = count,
            BlockCategory::Rest => self.rest = count,
            BlockCategory::Custom(name) => {
                if let Some(existing_key) = self
                    .custom
                    .keys()
                    .find(|k| k.eq_ignore_ascii_case(name))
                    .cloned()
                {
                    self.custom.insert(existing_key, count);
                } else {
                    self.custom.insert(name.clone(), count);
                }
            }
        }
    }
}

/// Statistics for a specific blocklist source (Official or Community).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlocklistStats {
    pub name: String,
    pub counts: CategoryCounts,
    pub enabled: bool,
}

/// Raw JSON blocklist payload with metadata.
#[derive(Debug, Clone)]
pub struct RawBlocklistInput {
    pub name: String,
    pub json: String,
    pub enabled: bool,
}

/// Dynamic or fallback domain store for all categories.
#[derive(Debug, Clone)]
pub struct DomainLists {
    pub video_domains: Vec<String>,
    pub image_domains: Vec<String>,
    pub string_domains: Vec<String>,
    pub custom_domains: BTreeMap<String, Vec<String>>,
    pub rest_domains: Vec<String>,
    #[cfg(test)]
    pub protected_domains: Vec<String>,
    pub list_stats: Vec<BlocklistStats>,
    pub total_counts: CategoryCounts,
    pub custom_categories: Vec<String>,
}

impl DomainLists {
    pub fn all_categories(&self) -> Vec<BlockCategory> {
        let mut cats = vec![
            BlockCategory::Video,
            BlockCategory::Images,
            BlockCategory::Strings,
        ];
        for custom in &self.custom_categories {
            cats.push(BlockCategory::Custom(custom.clone()));
        }
        cats.push(BlockCategory::Rest);
        cats
    }

    pub fn domains_for_category(&self, cat: &BlockCategory) -> &[String] {
        match cat {
            BlockCategory::Video => &self.video_domains,
            BlockCategory::Images => &self.image_domains,
            BlockCategory::Strings => &self.string_domains,
            BlockCategory::Rest => &self.rest_domains,
            BlockCategory::Custom(name) => self
                .custom_domains
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_slice())
                .unwrap_or(&[]),
        }
    }
}

static ACTIVE_DOMAIN_LISTS: RwLock<Option<Arc<DomainLists>>> = RwLock::new(None);
static LATEST_VRC_CONFIG_JSON: RwLock<Option<String>> = RwLock::new(None);
static LATEST_COMMUNITY_CONFIG_JSON: RwLock<Option<String>> = RwLock::new(None);
static LATEST_COMMUNITY_LISTS: RwLock<Vec<RawBlocklistInput>> = RwLock::new(Vec::new());
static LOAD_COMMUNITY_ENABLED: AtomicBool = AtomicBool::new(true);

/// Maximum age for the local cached community blocklist before redownloading (1 hour).
pub const COMMUNITY_CACHE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Primary file path where downloaded community blocklist JSON is cached.
pub fn community_cache_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "lvr")
        .map(|dirs| dirs.cache_dir().join("community_config.json"))
        .unwrap_or_else(|| PathBuf::from("assets/lists/community.json"))
}

/// Primary file path where a specific community blocklist JSON is cached.
pub fn community_cache_path_for_id(id: &str) -> PathBuf {
    if id == "bluscream" || id == "default" {
        community_cache_path()
    } else if let Some(dirs) = directories::ProjectDirs::from("", "", "lvr") {
        dirs.cache_dir().join(format!("community_{id}.json"))
    } else {
        PathBuf::from(format!("assets/lists/community_{id}.json"))
    }
}

/// Checks whether a file exists and is less than the given maximum age.
pub fn is_file_fresh(path: &Path, max_age: std::time::Duration) -> bool {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .map(|elapsed| elapsed < max_age)
        .unwrap_or(false)
}

/// Returns the path, age, and content of the newest existing local community config file.
pub fn get_newest_local_community_file() -> Option<(PathBuf, std::time::Duration, String)> {
    let mut candidates = Vec::new();
    candidates.push(community_cache_path());
    if let Some(dirs) = directories::ProjectDirs::from("", "", "lvr") {
        candidates.push(dirs.config_dir().join("community_config.json"));
    }
    candidates.push(PathBuf::from("assets/lists/community.json"));
    candidates.push(PathBuf::from("/run/media/system/Data/Projects/lvr/assets/lists/community.json"));
    // Backwards compatibility fallback if older file still exists
    candidates.push(PathBuf::from("assets/lists/config.json"));

    let mut best: Option<(PathBuf, std::time::Duration, String)> = None;

    for path in &candidates {
        if let Ok(meta) = fs::metadata(path)
            && let Ok(modified) = meta.modified()
                && let Ok(elapsed) = modified.elapsed()
                    && let Ok(content) = fs::read_to_string(path)
                        && !content.trim().is_empty() {
                            match best {
                                Some((_, best_elapsed, _)) if elapsed < best_elapsed => {
                                    best = Some((path.clone(), elapsed, content));
                                }
                                None => {
                                    best = Some((path.clone(), elapsed, content));
                                }
                                _ => {}
                            }
                        }
    }

    best
}

/// Returns local community config content only if a local file exists and is less than 1 hour old.
pub fn get_fresh_local_community_config() -> Option<String> {
    if let Some((path, elapsed, content)) = get_newest_local_community_file()
        && elapsed < COMMUNITY_CACHE_MAX_AGE {
            tracing::debug!(
                "Local community config at {} is fresh (age: {}s < 3600s), skipping redownload",
                path.display(),
                elapsed.as_secs()
            );
            return Some(content);
        }
    None
}

/// Checks whether a local community config file exists and is fresh (< 1h) for a specific source.
pub fn get_fresh_local_community_config_for_source(
    source: &crate::config::CommunityBlocklistSource,
) -> Option<String> {
    if source.id == "bluscream" || source.id == "default" {
        return get_fresh_local_community_config();
    }
    let path = community_cache_path_for_id(&source.id);
    if is_file_fresh(&path, COMMUNITY_CACHE_MAX_AGE)
        && let Ok(content) = fs::read_to_string(&path)
            && !content.trim().is_empty() {
                return Some(content);
            }
    None
}

/// Unconditionally downloads text from a remote HTTP URL or reads from local file path.
pub async fn fetch_remote_url(url: &str) -> Result<String> {
    if let Some(file_path) = url.strip_prefix("file://") {
        return Ok(fs::read_to_string(file_path)?);
    }
    if url.starts_with('/') {
        return Ok(fs::read_to_string(url)?);
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .user_agent("lvr/0.1.0")
        .build()?;

    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        anyhow::bail!("Request to {} failed with HTTP status {}", url, response.status());
    }
    Ok(response.text().await?)
}

/// Fetches a community blocklist source from network or reads from local disk if fresh (< 1h).
pub async fn fetch_community_source(
    source: &crate::config::CommunityBlocklistSource,
) -> Result<String> {
    if let Some(fresh) = get_fresh_local_community_config_for_source(source) {
        return Ok(fresh);
    }

    tracing::info!("Downloading community blocklist '{}' from {}", source.name, source.url);
    match fetch_remote_url(&source.url).await {
        Ok(text) => {
            let cache_path = community_cache_path_for_id(&source.id);
            if let Some(parent) = cache_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(e) = fs::write(&cache_path, &text) {
                tracing::warn!("Failed to save cache to {}: {e}", cache_path.display());
            } else {
                tracing::info!("Saved fresh cache to {}", cache_path.display());
            }
            Ok(text)
        }
        Err(err) => {
            let cache_path = community_cache_path_for_id(&source.id);
            if let Ok(stale) = fs::read_to_string(&cache_path)
                && !stale.trim().is_empty() {
                    tracing::warn!("Failed to download '{}' ({err:#}); using stale cache", source.name);
                    return Ok(stale);
                }
            if (source.id == "bluscream" || source.id == "default")
                && let Some((_, _, stale)) = get_newest_local_community_file() {
                    return Ok(stale);
                }
            Err(err)
        }
    }
}



/// Returns the community config JSON from memory cache, local disk cache, or repository file if present.
/// Note: Community blocklist is NEVER embedded at compile-time.
pub fn get_community_config_json() -> Option<String> {
    if let Ok(guard) = LATEST_COMMUNITY_CONFIG_JSON.read()
        && let Some(ref json) = *guard
            && !json.trim().is_empty() {
                return Some(json.clone());
            }
    get_newest_local_community_file().map(|(_, _, content)| content)
}

/// Returns the currently active domain lists, initializing from remote VRChat config or falling back to hardcoded.
pub fn active_domains() -> Arc<DomainLists> {
    if let Ok(guard) = ACTIVE_DOMAIN_LISTS.read()
        && let Some(lists) = guard.as_ref() {
            return Arc::clone(lists);
        }
    let load_comm = LOAD_COMMUNITY_ENABLED.load(Ordering::Relaxed);
    let fallback = Arc::new(build_domain_lists_fallback_with_community(load_comm));
    if let Ok(mut guard) = ACTIVE_DOMAIN_LISTS.write()
        && guard.is_none() {
            *guard = Some(Arc::clone(&fallback));
        }
    fallback
}



/// Rebuilds and updates `ACTIVE_DOMAIN_LISTS` dynamically with a full configuration.
pub async fn reload_domain_lists_with_config(
    cfg: &crate::config::DomainBlockConfig,
) -> Arc<DomainLists> {
    LOAD_COMMUNITY_ENABLED.store(cfg.load_community_blocklists, Ordering::Relaxed);
    let mut inputs = Vec::new();

    for source in &cfg.community_sources {
        if !cfg.load_community_blocklists || !source.enabled {
            inputs.push(RawBlocklistInput {
                name: source.name.clone(),
                json: String::new(),
                enabled: false,
            });
            continue;
        }

        match fetch_community_source(source).await {
            Ok(json) => {
                inputs.push(RawBlocklistInput {
                    name: source.name.clone(),
                    json,
                    enabled: true,
                });
            }
            Err(err) => {
                tracing::warn!("Failed to reload community list '{}': {err:#}", source.name);
                inputs.push(RawBlocklistInput {
                    name: source.name.clone(),
                    json: String::new(),
                    enabled: false,
                });
            }
        }
    }

    if let Ok(mut guard) = LATEST_COMMUNITY_LISTS.write() {
        *guard = inputs.clone();
    }
    if let Some(first) = inputs.iter().find(|i| i.enabled && !i.json.is_empty())
        && let Ok(mut guard) = LATEST_COMMUNITY_CONFIG_JSON.write() {
            *guard = Some(first.json.clone());
        }

    let vrc_json = LATEST_VRC_CONFIG_JSON
        .read()
        .ok()
        .and_then(|g| g.clone());

    let lists = match vrc_json {
        Some(ref body) => {
            parse_all_domain_lists(body, &inputs)
                .unwrap_or_else(|_| build_domain_lists_fallback_with_inputs(&inputs))
        }
        None => build_domain_lists_fallback_with_inputs(&inputs),
    };

    let arc = Arc::new(lists);
    if let Ok(mut guard) = ACTIVE_DOMAIN_LISTS.write() {
        *guard = Some(Arc::clone(&arc));
    }
    arc
}



/// Try to fetch live VRChat config and all community configs asynchronously with a full configuration.
pub async fn init_from_remote_or_fallback_with_config(
    cfg: &crate::config::DomainBlockConfig,
) {
    LOAD_COMMUNITY_ENABLED.store(cfg.load_community_blocklists, Ordering::Relaxed);
    let mut inputs = Vec::new();

    for source in &cfg.community_sources {
        if !cfg.load_community_blocklists || !source.enabled {
            inputs.push(RawBlocklistInput {
                name: source.name.clone(),
                json: String::new(),
                enabled: false,
            });
            continue;
        }

        match fetch_community_source(source).await {
            Ok(json) => {
                inputs.push(RawBlocklistInput {
                    name: source.name.clone(),
                    json,
                    enabled: true,
                });
            }
            Err(err) => {
                tracing::warn!("Failed to load community list '{}': {err:#}", source.name);
                inputs.push(RawBlocklistInput {
                    name: source.name.clone(),
                    json: String::new(),
                    enabled: false,
                });
            }
        }
    }

    if let Ok(mut guard) = LATEST_COMMUNITY_LISTS.write() {
        *guard = inputs.clone();
    }
    if let Some(first) = inputs.iter().find(|i| i.enabled && !i.json.is_empty())
        && let Ok(mut guard) = LATEST_COMMUNITY_CONFIG_JSON.write() {
            *guard = Some(first.json.clone());
        }

    let lists = match fetch_remote_config().await {
        Ok(body) => {
            if let Ok(mut guard) = LATEST_VRC_CONFIG_JSON.write() {
                *guard = Some(body.clone());
            }
            let merged = parse_all_domain_lists(&body, &inputs)
                .unwrap_or_else(|_| build_domain_lists_fallback_with_inputs(&inputs));
            tracing::info!(
                "Successfully built media blocking domain lists dynamically (videos: {}, images: {}, strings: {}, rest: {}, total: {})",
                merged.video_domains.len(),
                merged.image_domains.len(),
                merged.string_domains.len(),
                merged.rest_domains.len(),
                merged.total_counts.total()
            );
            merged
        }
        Err(err) => {
            tracing::warn!("Failed to fetch live VRChat config ({err:#}); falling back to hardcoded domain lists");
            build_domain_lists_fallback_with_inputs(&inputs)
        }
    };

    if let Ok(mut guard) = ACTIVE_DOMAIN_LISTS.write() {
        *guard = Some(Arc::new(lists));
    }
}

pub fn build_domain_lists_fallback_with_community(load_community: bool) -> DomainLists {
    let inputs = if load_community {
        let json = get_community_config_json();
        vec![RawBlocklistInput {
            name: "Community".to_string(),
            json: json.unwrap_or_default(),
            enabled: true,
        }]
    } else {
        vec![RawBlocklistInput {
            name: "Community".to_string(),
            json: String::new(),
            enabled: false,
        }]
    };
    build_domain_lists_fallback_with_inputs(&inputs)
}



pub fn build_domain_lists_fallback_with_inputs(inputs: &[RawBlocklistInput]) -> DomainLists {
    parse_all_domain_lists(EMBEDDED_FALLBACK_CONFIG, inputs)
        .expect("fallback VRChat config JSON must always be valid")
}

async fn fetch_remote_config() -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent("lvr/0.1.0")
        .build()?;

    let response = client
        .get("https://api.vrchat.cloud/api/1/config")
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("VRChat config request failed with HTTP status {}", response.status());
    }

    Ok(response.text().await?)
}

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
    "generatorPublicCaptcha",
    "giftDisplayType",
    "globalCacheVersion",
    "globalCacheVersionDefault",
    "googleApiClientId",
    "googleApiUnityClientId",
    "heightTimeoutMap",
    "homeWorldId",
    "homepageRedirectTarget",
    "hubWorldId",
    "immunityHeaderGitEvents",
    "iosAppVersion",
    "iosVersion",
    "jobsEmail",
    "justifyProfileCountryTimeout",
    "justifyRankEntryBulk",
    "labelSubscriberGenerator",
    "loadingScreenWeights",
    "localizedInstanceExcludedLanguageCodes",
    "loopNewsPhotonLoggingWeight",
    "lowMemoryGoHomeTimeout",
    "maxUserEmoji",
    "maxUserStickers",
    "maximumUnityVersionForUploads",
    "minSupportedClientBuildNumber",
    "minimumUnityVersionForUploads",
    "moderationEmail",
    "ninkilim",
    "notAllowedToSelectAvatarInPrivateWorldMessage",
    "offlineAnalysis",
    "photonNameserverOverrides",
    "photonPublicKeys",
    "player-url-resolver-sha1",
    "player-url-resolver-sha1-gfn-override",
    "player-url-resolver-version",
    "player-url-resolver-version-gfn-override",
    "profileDefaults",
    "propComponentList",
    "publicKey",
    "publicTimer",
    "questMinimumLowMemoryThreshold",
    "reportCategories",
    "reportFormUrl",
    "reportOptions",
    "reportReasons",
    "requireAgeVerificationBetaTag",
    "restNightlyApi",
    "rotationAttribute",
    "sdkDeveloperFaqUrl",
    "sdkDiscordUrl",
    "sdkNotAllowedToPublishMessage",
    "sdkUnityVersion",
    "semaphoreSouvenir",
    "showLoginQRCode",
    "skipLegDay",
    "sliceActiveFormatTtlListener",
    "supportEmail",
    "supportFormUrl",
    "thresholdProtocolArrayIpv4",
    "throttleLoss",
    "ticketFormationSegmentPocket",
    "timeOutWorldId",
    "timeRankingBeachKeyword",
    "timekeeping",
    "timeoutPrototypeRankGroup",
    "tutorialWorldId",
    "updateRateMsMaximum",
    "updateRateMsMinimum",
    "updateRateMsNormal",
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
    if KNOWN_NON_URLLIST_KEYS.iter().any(|k| k.eq_ignore_ascii_case(key)) {
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

fn classify_domains(
    raw_domains_by_cat: &BTreeMap<BlockCategory, HashSet<String>>,
    protected: &HashSet<String>,
) -> ClassifiedDomains {
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

    let is_vrc_or_asset_protected = |d: &str| -> bool {
        let bare = d.trim_start_matches("*.");
        if protected.contains(d) || protected.contains(bare) || protected.contains(&format!("*.{bare}")) {
            return true;
        }
        if bare == "vrchat.com" || bare.ends_with(".vrchat.com")
            || bare == "vrchat.cloud" || bare.ends_with(".vrchat.cloud")
            || bare == "dbinj8iahsbec.cloudfront.net" || bare.ends_with(".dbinj8iahsbec.cloudfront.net")
        {
            return true;
        }
        false
    };

    let is_pure_category_protected = |d: &str| -> bool {
        if in_multiple.contains(d) {
            return true;
        }
        is_vrc_or_asset_protected(d)
    };

    let mut video_domains: Vec<String> = raw_domains_by_cat
        .get(&BlockCategory::Video)
        .map(|s| s.iter().filter(|d| !is_pure_category_protected(d)).cloned().collect())
        .unwrap_or_default();
    let mut image_domains: Vec<String> = raw_domains_by_cat
        .get(&BlockCategory::Images)
        .map(|s| s.iter().filter(|d| !is_pure_category_protected(d)).cloned().collect())
        .unwrap_or_default();
    let mut string_domains: Vec<String> = raw_domains_by_cat
        .get(&BlockCategory::Strings)
        .map(|s| s.iter().filter(|d| !is_pure_category_protected(d)).cloned().collect())
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
        .filter(|d| !is_vrc_or_asset_protected(d))
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
    counts.rest = all_domains.iter().filter(|d| r_set.contains(d.as_str())).count();
    counts
}

fn compute_list_stats(
    official_domains_by_cat: &BTreeMap<BlockCategory, HashSet<String>>,
    comm_data: Vec<CommData>,
    classified: &ClassifiedDomains,
) -> (Vec<BlocklistStats>, CategoryCounts) {
    let v_set: HashSet<&str> = classified.video_domains.iter().map(|s| s.as_str()).collect();
    let i_set: HashSet<&str> = classified.image_domains.iter().map(|s| s.as_str()).collect();
    let s_set: HashSet<&str> = classified.string_domains.iter().map(|s| s.as_str()).collect();
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

impl BlockCategory {
    pub fn label(&self) -> &str {
        match self {
            Self::Video => "Videos",
            Self::Images => "Images",
            Self::Strings => "Strings",
            Self::Custom(name) => name.as_str(),
            Self::Rest => "Rest",
        }
    }

    pub fn short_label(&self) -> &str {
        match self {
            Self::Video => "Video",
            Self::Images => "Image",
            Self::Strings => "String",
            Self::Custom(name) => name.as_str(),
            Self::Rest => "Rest",
        }
    }

    pub fn header_tag(&self) -> String {
        match self {
            Self::Video => "# ----- BEGIN LVR VIDEO BLOCK -----".to_string(),
            Self::Images => "# ----- BEGIN LVR IMAGE BLOCK -----".to_string(),
            Self::Strings => "# ----- BEGIN LVR STRING BLOCK -----".to_string(),
            Self::Custom(name) => format!("# ----- BEGIN LVR {} BLOCK -----", name.to_uppercase()),
            Self::Rest => "# ----- BEGIN LVR REST BLOCK -----".to_string(),
        }
    }

    pub fn footer_tag(&self) -> String {
        match self {
            Self::Video => "# ----- END LVR VIDEO BLOCK -----".to_string(),
            Self::Images => "# ----- END LVR IMAGE BLOCK -----".to_string(),
            Self::Strings => "# ----- END LVR STRING BLOCK -----".to_string(),
            Self::Custom(name) => format!("# ----- END LVR {} BLOCK -----", name.to_uppercase()),
            Self::Rest => "# ----- END LVR REST BLOCK -----".to_string(),
        }
    }

    pub fn from_tag(tag: &str) -> Option<Self> {
        let trimmed = tag.trim();
        let middle = trimmed
            .strip_prefix("# ----- BEGIN LVR ")?
            .strip_suffix(" BLOCK -----")?
            .trim();
        match middle {
            "VIDEO" => Some(Self::Video),
            "IMAGE" => Some(Self::Images),
            "STRING" => Some(Self::Strings),
            "REST" => Some(Self::Rest),
            custom => {
                let lists = active_domains();
                if let Some(matching) = lists.custom_categories.iter().find(|c| c.eq_ignore_ascii_case(custom)) {
                    Some(Self::Custom(matching.clone()))
                } else {
                    Some(Self::Custom(custom.to_string()))
                }
            }
        }
    }
}

/// State of all domain blocking categories.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockState {
    pub video_blocked: bool,
    pub images_blocked: bool,
    pub strings_blocked: bool,
    pub rest_blocked: bool,
    #[serde(default)]
    pub custom_blocked: BTreeMap<String, bool>,
}

impl BlockState {
    pub fn is_blocked(&self, category: &BlockCategory) -> bool {
        match category {
            BlockCategory::Video => self.video_blocked,
            BlockCategory::Images => self.images_blocked,
            BlockCategory::Strings => self.strings_blocked,
            BlockCategory::Rest => self.rest_blocked,
            BlockCategory::Custom(name) => self
                .custom_blocked
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, &b)| b)
                .unwrap_or(false),
        }
    }

    pub fn set_blocked(&mut self, category: &BlockCategory, blocked: bool) {
        match category {
            BlockCategory::Video => self.video_blocked = blocked,
            BlockCategory::Images => self.images_blocked = blocked,
            BlockCategory::Strings => self.strings_blocked = blocked,
            BlockCategory::Rest => self.rest_blocked = blocked,
            BlockCategory::Custom(name) => {
                if let Some(existing_key) = self
                    .custom_blocked
                    .keys()
                    .find(|k| k.eq_ignore_ascii_case(name))
                    .cloned()
                {
                    self.custom_blocked.insert(existing_key, blocked);
                } else {
                    self.custom_blocked.insert(name.clone(), blocked);
                }
            }
        }
    }
}

/// Resolve the VRChat Proton prefix directory.
pub fn detect_vrc_prefix(configured_prefix: &str) -> Option<PathBuf> {
    if !configured_prefix.trim().is_empty() {
        let p = PathBuf::from(expand_tilde(configured_prefix.trim()));
        if p.is_dir() {
            return Some(p);
        }
    }

    let candidates = [
        "/run/media/system/Data/Games/Steam/steamapps/compatdata/438100",
        "/run/media/system/Data/SteamLibrary/steamapps/compatdata/438100",
    ];

    for candidate in candidates {
        let p = PathBuf::from(candidate);
        if p.is_dir() {
            return Some(p);
        }
    }

    let home = directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/root"));

    let steam_roots = [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ];

    for root in steam_roots {
        let p = root.join("steamapps/compatdata/438100");
        if p.is_dir() {
            return Some(p);
        }
    }

    None
}

fn expand_tilde(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => directories::BaseDirs::new()
            .map(|d| d.home_dir().join(rest).to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string()),
        None => path.to_string(),
    }
}

pub fn prefix_hosts_path(prefix: &Path) -> PathBuf {
    prefix.join("pfx/drive_c/windows/system32/drivers/etc/hosts")
}

pub fn vrc_tools_dir(prefix: &Path) -> PathBuf {
    prefix.join("pfx/drive_c/users/steamuser/AppData/LocalLow/VRChat/VRChat/Tools")
}

/// Read current blocking state of all categories from prefix.
pub fn read_block_state(prefix: &Path) -> BlockState {
    let hosts_path = prefix_hosts_path(prefix);
    let content = fs::read_to_string(&hosts_path).unwrap_or_default();
    let mut state = BlockState::default();

    for line in content.lines() {
        if let Some(cat) = BlockCategory::from_tag(line.trim()) {
            state.set_blocked(&cat, true);
        }
    }

    state
}

/// Set the blocking state for a given category.
pub fn set_category_blocked(prefix: &Path, category: &BlockCategory, block: bool) -> Result<()> {
    let mut state = read_block_state(prefix);
    state.set_blocked(category, block);
    sync_all(prefix, &state)?;
    Ok(())
}

/// Apply full block state (hosts + yt-dlp) cleanly in one atomic operation.
pub fn sync_all(prefix: &Path, state: &BlockState) -> Result<()> {
    update_hosts_file(prefix, state)?;
    update_ytdlp_file(prefix, state.video_blocked)?;
    Ok(())
}

fn update_hosts_file(prefix: &Path, state: &BlockState) -> Result<()> {
    let hosts_path = prefix_hosts_path(prefix);
    if let Some(parent) = hosts_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let existing = if hosts_path.is_file() {
        fs::read_to_string(&hosts_path).unwrap_or_default()
    } else {
        String::new()
    };

    // Remove any existing LVR blocks
    let mut cleaned = Vec::new();
    let mut inside_lvr_block = false;

    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("# ----- BEGIN LVR ") && trimmed.ends_with(" BLOCK -----") {
            inside_lvr_block = true;
            continue;
        }
        if inside_lvr_block {
            if trimmed.starts_with("# ----- END LVR ") && trimmed.ends_with(" BLOCK -----") {
                inside_lvr_block = false;
            }
            continue;
        }
        cleaned.push(line);
    }

    let mut result = cleaned.join("\n");
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }

    // Append blocks for whichever categories are enabled
    let lists = active_domains();
    for cat in lists.all_categories() {
        if state.is_blocked(&cat) {
            result.push_str(&cat.header_tag());
            result.push('\n');
            result.push_str(&format!(
                "# Blocked by LinuxVR (lvr): {}\n",
                cat.label()
            ));

            for domain in lists.domains_for_category(&cat) {
                let bare = domain.trim_start_matches("*.");
                if bare.is_empty() || bare == "localhost" {
                    continue;
                }
                // Skip raw IP addresses in hosts file domain entries
                if bare.parse::<std::net::IpAddr>().is_ok() {
                    continue;
                }
                result.push_str(&format!("0.0.0.0 {bare}\n"));
                if !domain.starts_with("*.") && !domain.starts_with("www.") {
                    result.push_str(&format!("0.0.0.0 www.{bare}\n"));
                }
            }

            result.push_str(&cat.footer_tag());
            result.push('\n');
        }
    }

    let tmp = hosts_path.with_extension("lvr_tmp");
    fs::write(&tmp, result)?;
    fs::rename(tmp, hosts_path)?;
    Ok(())
}

fn update_ytdlp_file(prefix: &Path, block_video: bool) -> Result<()> {
    let tools_dir = vrc_tools_dir(prefix);
    if !tools_dir.is_dir() {
        return Ok(());
    }

    let ytdl_path = tools_dir.join("yt-dlp.exe");
    let lvr_backup = tools_dir.join("yt-dlp.exe.lvr_orig");

    if block_video {
        if !ytdl_path.is_file() {
            return Ok(());
        }

        // Back up current yt-dlp.exe without touching any VRCVideoCacher yt-dlp.exe.bkp
        if !lvr_backup.is_file() {
            let _ = make_file_writable(&ytdl_path);
            let _ = fs::copy(&ytdl_path, &lvr_backup);
        }

        // Fast-failing stub
        let stub_content = b"MZ\x00\x00ERROR: Video disabled by lvr\r\n";
        let _ = make_file_writable(&ytdl_path);
        if let Ok(mut f) = fs::File::create(&ytdl_path) {
            let _ = f.write_all(stub_content);
        }

        // Mark read-only (0444) so VRChat launch cannot overwrite it
        let _ = make_file_readonly(&ytdl_path);
    } else {
        if lvr_backup.is_file() {
            let _ = make_file_writable(&ytdl_path);
            let _ = fs::copy(&lvr_backup, &ytdl_path);
            let _ = fs::remove_file(&lvr_backup);
            let _ = make_file_writable(&ytdl_path);
        } else if ytdl_path.is_file() {
            let _ = make_file_writable(&ytdl_path);
        }
    }

    Ok(())
}

fn make_file_writable(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

fn make_file_readonly(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o444);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn domain_lists_have_zero_overlap_between_categories() {
        let fallback = build_domain_lists_fallback_with_inputs(&[]);
        let video_set: HashSet<_> = fallback.video_domains.iter().cloned().collect();
        let image_set: HashSet<_> = fallback.image_domains.iter().cloned().collect();
        let string_set: HashSet<_> = fallback.string_domains.iter().cloned().collect();
        let rest_set: HashSet<_> = fallback.rest_domains.iter().cloned().collect();
        let protected_set: HashSet<_> = fallback.protected_domains.iter().cloned().collect();

        // 1. None of the four categories share any domains
        assert!(video_set.is_disjoint(&image_set), "Video and Image lists overlap!");
        assert!(video_set.is_disjoint(&string_set), "Video and String lists overlap!");
        assert!(video_set.is_disjoint(&rest_set), "Video and Rest lists overlap!");
        assert!(image_set.is_disjoint(&string_set), "Image and String lists overlap!");
        assert!(image_set.is_disjoint(&rest_set), "Image and Rest lists overlap!");
        assert!(string_set.is_disjoint(&rest_set), "String and Rest lists overlap!");

        // 2. Core VRChat and whiteListedAssetUrls are strictly protected and not in ANY category
        for core in &["assets.vrchat.com", "vrchat.com", "vrchat.cloud", "dbinj8iahsbec.cloudfront.net"] {
            assert!(!video_set.contains(*core), "core {core} in Video list!");
            assert!(!image_set.contains(*core), "core {core} in Image list!");
            assert!(!string_set.contains(*core), "core {core} in String list!");
            assert!(!rest_set.contains(*core), "core {core} in Rest list!");
            assert!(protected_set.contains(*core), "core {core} not in Protected list!");
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

        let lists = parse_all_domain_lists(sample_json, &[]).expect("failed to parse sample config JSON");

        // 1. Check pure separation
        assert!(lists.video_domains.contains(&"*.youtube.com".to_string()));
        assert!(lists.video_domains.contains(&"twitch.tv".to_string()));
        assert!(lists.image_domains.contains(&"i.imgur.com".to_string()));
        assert!(lists.string_domains.contains(&"pastebin.com".to_string()));

        // 2. Shared across lists (*.github.io, ciel.topaz.chat) MUST NOT be in pure video/image/string lists
        for shared in &["*.github.io", "ciel.topaz.chat"] {
            assert!(!lists.video_domains.contains(&shared.to_string()), "shared {shared} in video");
            assert!(!lists.image_domains.contains(&shared.to_string()), "shared {shared} in images");
            assert!(!lists.string_domains.contains(&shared.to_string()), "shared {shared} in strings");
        }

        // 3. Multi-list domains NOT in whiteListedAssetUrls / vrchat core MUST be in rest_domains
        assert!(lists.rest_domains.contains(&"*.github.io".to_string()));
        assert!(lists.rest_domains.contains(&"ciel.topaz.chat".to_string()));

        // 4. whiteListedAssetUrls & vrchat core domains MUST NOT be in ANY list, including rest
        for asset in &["assets.vrchat.com", "dbinj8iahsbec.cloudfront.net", "redirect.vrchat.com"] {
            assert!(!lists.video_domains.contains(&asset.to_string()), "asset {asset} in video");
            assert!(!lists.image_domains.contains(&asset.to_string()), "asset {asset} in images");
            assert!(!lists.string_domains.contains(&asset.to_string()), "asset {asset} in strings");
            assert!(!lists.rest_domains.contains(&asset.to_string()), "asset {asset} in rest");
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
        let lists = parse_all_domain_lists(vrc_json, &inputs)
            .expect("merging configs should succeed");

        // 1. Community video and image domains are merged
        assert!(lists.video_domains.contains(&"custom-video.com".to_string()));
        assert!(lists.image_domains.contains(&"custom-image.com".to_string()));
        assert!(lists.string_domains.contains(&"custom-string.com".to_string()));

        // 2. Multi-list domain shared-multipurpose.com goes to rest, not pure video or image
        assert!(!lists.video_domains.contains(&"shared-multipurpose.com".to_string()));
        assert!(!lists.image_domains.contains(&"shared-multipurpose.com".to_string()));
        assert!(lists.rest_domains.contains(&"shared-multipurpose.com".to_string()));

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
        assert!(!reloaded_no_comm.video_domains.contains(&"andre-stinkt.de".to_string()));
    }

    #[test]
    fn community_config_cache_freshness_check() {
        let temp_dir = std::env::temp_dir();
        let fresh_file = temp_dir.join("lvr_test_fresh_config.json");
        let _ = fs::write(&fresh_file, r#"{"urlList": ["fresh.com"]}"#);

        assert!(is_file_fresh(&fresh_file, COMMUNITY_CACHE_MAX_AGE));
        assert!(!is_file_fresh(&fresh_file, std::time::Duration::from_nanos(1)));

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

        let lists = parse_all_domain_lists(vrc_json, &inputs).expect("parse multiple community lists");

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
        assert!(!lists.custom_categories.contains(&"availableLanguages".to_string()));
        assert!(!lists.custom_categories.contains(&"availableLanguageCodes".to_string()));
        assert!(!lists.custom_categories.contains(&"clientApiKey".to_string()));
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
        let analytics_domains = lists.domains_for_category(&BlockCategory::Custom("Analytics".to_string()));
        assert!(analytics_domains.contains(&"api.amplitude.com".to_string()));
        assert!(analytics_domains.contains(&"analytics.unity3d.com".to_string()));

        // Check tags and header parsing
        let analytics_cat = BlockCategory::Custom("Analytics".to_string());
        assert_eq!(analytics_cat.header_tag(), "# ----- BEGIN LVR ANALYTICS BLOCK -----");
        assert_eq!(analytics_cat.footer_tag(), "# ----- END LVR ANALYTICS BLOCK -----");
        assert_eq!(BlockCategory::from_tag("# ----- BEGIN LVR ANALYTICS BLOCK -----"), Some(analytics_cat));
    }

    #[test]
    fn local_community_json_loads_analytics_category() {
        let lists = build_domain_lists_fallback_with_community(true);
        assert!(lists.custom_categories.contains(&"Analytics".to_string()));
        let analytics_domains = lists.domains_for_category(&BlockCategory::Custom("Analytics".to_string()));
        assert!(analytics_domains.contains(&"api.amplitude.com".to_string()));
        assert!(analytics_domains.len() >= 100);
    }
}

