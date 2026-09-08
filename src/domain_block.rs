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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlockCategory {
    Video,
    Images,
    Strings,
    Rest,
}

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

/// Embedded copy of VRChat remote config (`https://api.vrchat.cloud/api/1/config`)
/// captured at compile time as a fallback whenever the network is unavailable.
const EMBEDDED_FALLBACK_CONFIG: &str = include_str!("../assets/vrchat_config_fallback.json");

/// Remote URL for community blocklists extracted from community sources and logs.
#[allow(dead_code)]
pub const COMMUNITY_CONFIG_URL: &str =
    "https://github.com/Bluscream/lvr/raw/refs/heads/main/assets/lists/config.json";

/// Detailed domain counts per category.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CategoryCounts {
    pub video: usize,
    pub image: usize,
    pub string: usize,
    pub rest: usize,
}

impl CategoryCounts {
    pub fn total(&self) -> usize {
        self.video + self.image + self.string + self.rest
    }
}

/// Statistics for a specific blocklist source (Official or Community).
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub rest_domains: Vec<String>,
    #[allow(dead_code)]
    pub protected_domains: Vec<String>,
    pub list_stats: Vec<BlocklistStats>,
    pub total_counts: CategoryCounts,
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
        .unwrap_or_else(|| PathBuf::from("assets/lists/config.json"))
}

/// Primary file path where a specific community blocklist JSON is cached.
pub fn community_cache_path_for_id(id: &str) -> PathBuf {
    if id == "bluscream" || id == "default" {
        community_cache_path()
    } else if let Some(dirs) = directories::ProjectDirs::from("", "", "lvr") {
        dirs.cache_dir().join(format!("community_{id}.json"))
    } else {
        PathBuf::from(format!("assets/lists/config_{id}.json"))
    }
}

/// Checks whether a file exists and is less than the given maximum age.
#[allow(dead_code)]
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
    candidates.push(PathBuf::from("assets/lists/config.json"));
    candidates.push(PathBuf::from("/run/media/system/Data/Projects/lvr/assets/lists/config.json"));

    let mut best: Option<(PathBuf, std::time::Duration, String)> = None;

    for path in &candidates {
        if let Ok(meta) = fs::metadata(path) {
            if let Ok(modified) = meta.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    if let Ok(content) = fs::read_to_string(path) {
                        if !content.trim().is_empty() {
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
                }
            }
        }
    }

    best
}

/// Returns local community config content only if a local file exists and is less than 1 hour old.
pub fn get_fresh_local_community_config() -> Option<String> {
    if let Some((path, elapsed, content)) = get_newest_local_community_file() {
        if elapsed < COMMUNITY_CACHE_MAX_AGE {
            tracing::debug!(
                "Local community config at {} is fresh (age: {}s < 3600s), skipping redownload",
                path.display(),
                elapsed.as_secs()
            );
            return Some(content);
        }
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
    if is_file_fresh(&path, COMMUNITY_CACHE_MAX_AGE) {
        if let Ok(content) = fs::read_to_string(&path) {
            if !content.trim().is_empty() {
                return Some(content);
            }
        }
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
            if let Ok(stale) = fs::read_to_string(&cache_path) {
                if !stale.trim().is_empty() {
                    tracing::warn!("Failed to download '{}' ({err:#}); using stale cache", source.name);
                    return Ok(stale);
                }
            }
            if source.id == "bluscream" || source.id == "default" {
                if let Some((_, _, stale)) = get_newest_local_community_file() {
                    return Ok(stale);
                }
            }
            Err(err)
        }
    }
}

/// Unconditionally downloads the primary community config JSON from GitHub and saves it to local cache.
#[allow(dead_code)]
pub async fn fetch_community_config_remote() -> Result<String> {
    let source = crate::config::CommunityBlocklistSource {
        id: "bluscream".to_string(),
        name: "Community".to_string(),
        url: COMMUNITY_CONFIG_URL.to_string(),
        enabled: true,
    };
    fetch_remote_url(&source.url).await
}

/// Fetches the primary community config JSON, redownloading only if the local file is missing or over 1 hour old.
#[allow(dead_code)]
pub async fn fetch_community_config() -> Result<String> {
    let source = crate::config::CommunityBlocklistSource {
        id: "bluscream".to_string(),
        name: "Community".to_string(),
        url: COMMUNITY_CONFIG_URL.to_string(),
        enabled: true,
    };
    fetch_community_source(&source).await
}

/// Returns the community config JSON from memory cache, local disk cache, or repository file if present.
/// Note: Community blocklist is NEVER embedded at compile-time.
pub fn get_community_config_json() -> Option<String> {
    if let Ok(guard) = LATEST_COMMUNITY_CONFIG_JSON.read() {
        if let Some(ref json) = *guard {
            if !json.trim().is_empty() {
                return Some(json.clone());
            }
        }
    }
    get_newest_local_community_file().map(|(_, _, content)| content)
}

/// Returns the currently active domain lists, initializing from remote VRChat config or falling back to hardcoded.
pub fn active_domains() -> Arc<DomainLists> {
    if let Ok(guard) = ACTIVE_DOMAIN_LISTS.read() {
        if let Some(lists) = guard.as_ref() {
            return Arc::clone(lists);
        }
    }
    let load_comm = LOAD_COMMUNITY_ENABLED.load(Ordering::Relaxed);
    let fallback = Arc::new(build_domain_lists_fallback_with_community(load_comm));
    if let Ok(mut guard) = ACTIVE_DOMAIN_LISTS.write() {
        if guard.is_none() {
            *guard = Some(Arc::clone(&fallback));
        }
    }
    fallback
}

/// Rebuilds and updates `ACTIVE_DOMAIN_LISTS` dynamically (e.g. when toggling community blocklists).
#[allow(dead_code)]
pub async fn reload_domain_lists(load_community: bool) -> Arc<DomainLists> {
    let cfg = crate::config::DomainBlockConfig {
        load_community_blocklists: load_community,
        community_sources: crate::config::default_community_sources(),
    };
    reload_domain_lists_with_config(&cfg).await
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
    if let Some(first) = inputs.iter().find(|i| i.enabled && !i.json.is_empty()) {
        if let Ok(mut guard) = LATEST_COMMUNITY_CONFIG_JSON.write() {
            *guard = Some(first.json.clone());
        }
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

/// Try to fetch live VRChat config and community config asynchronously and initialize active domain lists.
#[allow(dead_code)]
pub async fn init_from_remote_or_fallback(load_community: bool) {
    let cfg = crate::config::DomainBlockConfig {
        load_community_blocklists: load_community,
        community_sources: crate::config::default_community_sources(),
    };
    init_from_remote_or_fallback_with_config(&cfg).await;
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
    if let Some(first) = inputs.iter().find(|i| i.enabled && !i.json.is_empty()) {
        if let Ok(mut guard) = LATEST_COMMUNITY_CONFIG_JSON.write() {
            *guard = Some(first.json.clone());
        }
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

#[allow(dead_code)]
pub fn build_domain_lists_fallback() -> DomainLists {
    build_domain_lists_fallback_with_community(LOAD_COMMUNITY_ENABLED.load(Ordering::Relaxed))
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

#[allow(dead_code)]
pub fn build_domain_lists_fallback_with_community_json(comm_json: Option<&str>) -> DomainLists {
    let inputs = match comm_json {
        Some(s) if !s.trim().is_empty() => vec![RawBlocklistInput {
            name: "Community".to_string(),
            json: s.to_string(),
            enabled: true,
        }],
        _ => Vec::new(),
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

/// Parse raw JSON from VRChat `/api/1/config` and partition domains strictly into pure video, image, string sets.
#[allow(dead_code)]
pub fn parse_vrchat_config_json(json_str: &str) -> Result<DomainLists> {
    parse_vrchat_and_community_config_json(json_str, None)
}

/// Parse VRChat config and optionally merge a single community blocklist.
pub fn parse_vrchat_and_community_config_json(
    vrc_json_str: &str,
    community_json_str: Option<&str>,
) -> Result<DomainLists> {
    let inputs = match community_json_str {
        Some(s) if !s.trim().is_empty() => vec![RawBlocklistInput {
            name: "Community".to_string(),
            json: s.to_string(),
            enabled: true,
        }],
        _ => Vec::new(),
    };
    parse_all_domain_lists(vrc_json_str, &inputs)
}

/// Parse VRChat config and multiple community blocklists, smartly enforcing safety invariants
/// and tracking blocked domain counts per list and total.
pub fn parse_all_domain_lists(
    vrc_json_str: &str,
    community_lists: &[RawBlocklistInput],
) -> Result<DomainLists> {
    #[derive(Deserialize, Default)]
    struct VrcRemoteConfig {
        #[serde(default, rename = "urlList")]
        url_list: Vec<String>,
        #[serde(default, rename = "imageHostUrlList")]
        image_list: Vec<String>,
        #[serde(default, rename = "stringHostUrlList")]
        string_list: Vec<String>,
        #[serde(default, rename = "whiteListedAssetUrls")]
        asset_urls: Vec<String>,
    }

    let parsed_vrc: VrcRemoteConfig = serde_json::from_str(vrc_json_str)?;

    let mut official_video: HashSet<String> = HashSet::new();
    for u in &parsed_vrc.url_list {
        let cleaned = clean_domain_entry(u);
        if !cleaned.is_empty() {
            official_video.insert(cleaned);
        }
    }
    let mut official_images: HashSet<String> = HashSet::new();
    for u in &parsed_vrc.image_list {
        let cleaned = clean_domain_entry(u);
        if !cleaned.is_empty() {
            official_images.insert(cleaned);
        }
    }
    let mut official_strings: HashSet<String> = HashSet::new();
    for u in &parsed_vrc.string_list {
        let cleaned = clean_domain_entry(u);
        if !cleaned.is_empty() {
            official_strings.insert(cleaned);
        }
    }
    let mut protected: HashSet<String> = HashSet::new();
    for u in &parsed_vrc.asset_urls {
        let cleaned = clean_domain_entry(u);
        if !cleaned.is_empty() {
            protected.insert(cleaned.clone());
            let bare = cleaned.trim_start_matches("*.");
            protected.insert(bare.to_string());
            protected.insert(format!("*.{bare}"));
        }
    }

    struct CommData {
        name: String,
        enabled: bool,
        video: HashSet<String>,
        images: HashSet<String>,
        strings: HashSet<String>,
    }

    let mut comm_data: Vec<CommData> = Vec::new();
    let mut raw_video = official_video.clone();
    let mut raw_images = official_images.clone();
    let mut raw_strings = official_strings.clone();

    for item in community_lists {
        if !item.enabled || item.json.trim().is_empty() {
            comm_data.push(CommData {
                name: item.name.clone(),
                enabled: false,
                video: HashSet::new(),
                images: HashSet::new(),
                strings: HashSet::new(),
            });
            continue;
        }

        let parsed: VrcRemoteConfig = serde_json::from_str(&item.json).unwrap_or_default();
        let mut v_set = HashSet::new();
        for u in &parsed.url_list {
            let cleaned = clean_domain_entry(u);
            if !cleaned.is_empty() {
                v_set.insert(cleaned.clone());
                raw_video.insert(cleaned);
            }
        }
        let mut i_set = HashSet::new();
        for u in &parsed.image_list {
            let cleaned = clean_domain_entry(u);
            if !cleaned.is_empty() {
                i_set.insert(cleaned.clone());
                raw_images.insert(cleaned);
            }
        }
        let mut s_set = HashSet::new();
        for u in &parsed.string_list {
            let cleaned = clean_domain_entry(u);
            if !cleaned.is_empty() {
                s_set.insert(cleaned.clone());
                raw_strings.insert(cleaned);
            }
        }
        for u in &parsed.asset_urls {
            let cleaned = clean_domain_entry(u);
            if !cleaned.is_empty() {
                protected.insert(cleaned.clone());
                let bare = cleaned.trim_start_matches("*.");
                protected.insert(bare.to_string());
                protected.insert(format!("*.{bare}"));
            }
        }

        comm_data.push(CommData {
            name: item.name.clone(),
            enabled: true,
            video: v_set,
            images: i_set,
            strings: s_set,
        });
    }

    // Always guarantee core asset wildcards
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

    // Detect any domain present in two or more lists and strictly protect it from pure video/image/string blocking
    let mut in_multiple = HashSet::new();
    for d in raw_video.iter().chain(raw_images.iter()).chain(raw_strings.iter()) {
        let count = (raw_video.contains(d) as usize)
            + (raw_images.contains(d) as usize)
            + (raw_strings.contains(d) as usize);
        if count >= 2 {
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

    let mut video_domains: Vec<String> = raw_video
        .into_iter()
        .filter(|d| !is_pure_category_protected(d))
        .collect();
    let mut image_domains: Vec<String> = raw_images
        .into_iter()
        .filter(|d| !is_pure_category_protected(d))
        .collect();
    let mut string_domains: Vec<String> = raw_strings
        .into_iter()
        .filter(|d| !is_pure_category_protected(d))
        .collect();

    let mut rest_domains: Vec<String> = in_multiple
        .into_iter()
        .filter(|d| !is_vrc_or_asset_protected(d))
        .collect();

    video_domains.sort();
    image_domains.sort();
    string_domains.sort();
    rest_domains.sort();

    let mut protected_list: Vec<String> = protected.into_iter().collect();
    protected_list.sort();

    // Calculate per-list stats
    let v_set: HashSet<&str> = video_domains.iter().map(|s| s.as_str()).collect();
    let i_set: HashSet<&str> = image_domains.iter().map(|s| s.as_str()).collect();
    let s_set: HashSet<&str> = string_domains.iter().map(|s| s.as_str()).collect();
    let r_set: HashSet<&str> = rest_domains.iter().map(|s| s.as_str()).collect();

    let mut list_stats = Vec::new();

    // 1. Official stats
    let off_v = official_video.iter().filter(|d| v_set.contains(d.as_str())).count();
    let off_i = official_images.iter().filter(|d| i_set.contains(d.as_str())).count();
    let off_s = official_strings.iter().filter(|d| s_set.contains(d.as_str())).count();
    let off_r = official_video.iter()
        .chain(official_images.iter())
        .chain(official_strings.iter())
        .filter(|d| r_set.contains(d.as_str()))
        .collect::<HashSet<_>>()
        .len();

    list_stats.push(BlocklistStats {
        name: "Official".to_string(),
        counts: CategoryCounts {
            video: off_v,
            image: off_i,
            string: off_s,
            rest: off_r,
        },
        enabled: true,
    });

    // 2. Community stats per list
    for comm in comm_data {
        if comm.enabled {
            let cv = comm.video.iter().filter(|d| v_set.contains(d.as_str())).count();
            let ci = comm.images.iter().filter(|d| i_set.contains(d.as_str())).count();
            let cs = comm.strings.iter().filter(|d| s_set.contains(d.as_str())).count();
            let cr = comm.video.iter()
                .chain(comm.images.iter())
                .chain(comm.strings.iter())
                .filter(|d| r_set.contains(d.as_str()))
                .collect::<HashSet<_>>()
                .len();
            list_stats.push(BlocklistStats {
                name: comm.name,
                counts: CategoryCounts {
                    video: cv,
                    image: ci,
                    string: cs,
                    rest: cr,
                },
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

    let total_counts = CategoryCounts {
        video: video_domains.len(),
        image: image_domains.len(),
        string: string_domains.len(),
        rest: rest_domains.len(),
    };

    Ok(DomainLists {
        video_domains,
        image_domains,
        string_domains,
        rest_domains,
        protected_domains: protected_list,
        list_stats,
        total_counts,
    })
}

fn clean_domain_entry(entry: &str) -> String {
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

impl BlockCategory {
    pub const ALL: [Self; 4] = [Self::Video, Self::Images, Self::Strings, Self::Rest];

    pub fn label(&self) -> &'static str {
        match self {
            Self::Video => "Videos",
            Self::Images => "Images",
            Self::Strings => "Strings",
            Self::Rest => "Rest",
        }
    }

    pub fn header_tag(&self) -> &'static str {
        match self {
            Self::Video => "# ----- BEGIN LVR VIDEO BLOCK -----",
            Self::Images => "# ----- BEGIN LVR IMAGE BLOCK -----",
            Self::Strings => "# ----- BEGIN LVR STRING BLOCK -----",
            Self::Rest => "# ----- BEGIN LVR REST BLOCK -----",
        }
    }

    pub fn footer_tag(&self) -> &'static str {
        match self {
            Self::Video => "# ----- END LVR VIDEO BLOCK -----",
            Self::Images => "# ----- END LVR IMAGE BLOCK -----",
            Self::Strings => "# ----- END LVR STRING BLOCK -----",
            Self::Rest => "# ----- END LVR REST BLOCK -----",
        }
    }

    /// The pure domains assigned exclusively to this category.
    /// Uses dynamic domains if fetched on startup, or the embedded fallback config.
    pub fn domains(&self) -> Vec<String> {
        let lists = active_domains();
        match self {
            Self::Video => lists.video_domains.clone(),
            Self::Images => lists.image_domains.clone(),
            Self::Strings => lists.string_domains.clone(),
            Self::Rest => lists.rest_domains.clone(),
        }
    }
}

/// State of all domain blocking categories.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockState {
    pub video_blocked: bool,
    pub images_blocked: bool,
    pub strings_blocked: bool,
    pub rest_blocked: bool,
}

impl BlockState {
    pub fn is_blocked(&self, category: BlockCategory) -> bool {
        match category {
            BlockCategory::Video => self.video_blocked,
            BlockCategory::Images => self.images_blocked,
            BlockCategory::Strings => self.strings_blocked,
            BlockCategory::Rest => self.rest_blocked,
        }
    }

    pub fn set_blocked(&mut self, category: BlockCategory, blocked: bool) {
        match category {
            BlockCategory::Video => self.video_blocked = blocked,
            BlockCategory::Images => self.images_blocked = blocked,
            BlockCategory::Strings => self.strings_blocked = blocked,
            BlockCategory::Rest => self.rest_blocked = blocked,
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
    BlockState {
        video_blocked: content.contains(BlockCategory::Video.header_tag()),
        images_blocked: content.contains(BlockCategory::Images.header_tag()),
        strings_blocked: content.contains(BlockCategory::Strings.header_tag()),
        rest_blocked: content.contains(BlockCategory::Rest.header_tag()),
    }
}

/// Set the blocking state for a given category.
pub fn set_category_blocked(prefix: &Path, category: BlockCategory, block: bool) -> Result<()> {
    let mut state = read_block_state(prefix);
    state.set_blocked(category, block);
    sync_all(prefix, state)?;
    Ok(())
}

/// Apply full block state (hosts + yt-dlp) cleanly in one atomic operation.
pub fn sync_all(prefix: &Path, state: BlockState) -> Result<()> {
    update_hosts_file(prefix, state)?;
    update_ytdlp_file(prefix, state.video_blocked)?;
    Ok(())
}

fn update_hosts_file(prefix: &Path, state: BlockState) -> Result<()> {
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
    let mut active_block: Option<BlockCategory> = None;

    for line in existing.lines() {
        let trimmed = line.trim();
        if let Some(cat) = BlockCategory::ALL.iter().find(|c| c.header_tag() == trimmed) {
            active_block = Some(*cat);
            continue;
        }
        if let Some(cat) = active_block {
            if trimmed == cat.footer_tag() {
                active_block = None;
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
    for cat in BlockCategory::ALL {
        if state.is_blocked(cat) {
            result.push_str(cat.header_tag());
            result.push('\n');
            result.push_str(&format!(
                "# Blocked by LinuxVR (lvr): {}\n",
                cat.label()
            ));

            for domain in cat.domains() {
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

            result.push_str(cat.footer_tag());
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
        let fallback = build_domain_lists_fallback();
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

        let lists = parse_vrchat_config_json(sample_json).expect("failed to parse sample config JSON");

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

        let lists = parse_vrchat_and_community_config_json(vrc_json, Some(community_json))
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
            COMMUNITY_CONFIG_URL,
            "https://github.com/Bluscream/lvr/raw/refs/heads/main/assets/lists/config.json"
        );

        // When community blocklists are disabled, only VRChat default fallback lists are present
        let no_comm = build_domain_lists_fallback_with_community(false);
        assert!(!no_comm.video_domains.contains(&"andre-stinkt.de".to_string()));

        // When reloading with false, ACTIVE_DOMAIN_LISTS is updated without community lists
        let reloaded_no_comm = reload_domain_lists(false).await;
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
}

