use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::Result;

use super::parser::parse_all_domain_lists;
use super::types::{DomainLists, RawBlocklistInput};

/// Embedded copy of VRChat remote config (`https://api.vrchat.cloud/api/1/config`)
/// captured at compile time as a fallback whenever the network is unavailable.
pub const EMBEDDED_FALLBACK_CONFIG: &str =
    include_str!(concat!(env!("OUT_DIR"), "/vrchat_config_fallback.json"));

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
    candidates.push(PathBuf::from(
        "/run/media/system/Data/Projects/lvr/assets/lists/community.json",
    ));
    // Backwards compatibility fallback if older file still exists
    candidates.push(PathBuf::from("assets/lists/config.json"));

    let mut best: Option<(PathBuf, std::time::Duration, String)> = None;

    for path in &candidates {
        if let Ok(meta) = fs::metadata(path)
            && let Ok(modified) = meta.modified()
            && let Ok(elapsed) = modified.elapsed()
            && let Ok(content) = fs::read_to_string(path)
            && !content.trim().is_empty()
        {
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
        && elapsed < COMMUNITY_CACHE_MAX_AGE
    {
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
        && !content.trim().is_empty()
    {
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
        anyhow::bail!(
            "Request to {} failed with HTTP status {}",
            url,
            response.status()
        );
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

    tracing::info!(
        "Downloading community blocklist '{}' from {}",
        source.name,
        source.url
    );
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
                && !stale.trim().is_empty()
            {
                tracing::warn!(
                    "Failed to download '{}' ({err:#}); using stale cache",
                    source.name
                );
                return Ok(stale);
            }
            if (source.id == "bluscream" || source.id == "default")
                && let Some((_, _, stale)) = get_newest_local_community_file()
            {
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
        && !json.trim().is_empty()
    {
        return Some(json.clone());
    }
    get_newest_local_community_file().map(|(_, _, content)| content)
}

/// Returns the currently active domain lists, initializing from remote VRChat config or falling back to hardcoded.
pub fn active_domains() -> Arc<DomainLists> {
    if let Ok(guard) = ACTIVE_DOMAIN_LISTS.read()
        && let Some(lists) = guard.as_ref()
    {
        return Arc::clone(lists);
    }
    let load_comm = LOAD_COMMUNITY_ENABLED.load(Ordering::Relaxed);
    let fallback = Arc::new(build_domain_lists_fallback_with_community(load_comm));
    if let Ok(mut guard) = ACTIVE_DOMAIN_LISTS.write()
        && guard.is_none()
    {
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
                tracing::warn!(
                    "Failed to reload community list '{}': {err:#}",
                    source.name
                );
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
        && let Ok(mut guard) = LATEST_COMMUNITY_CONFIG_JSON.write()
    {
        *guard = Some(first.json.clone());
    }

    let vrc_json = LATEST_VRC_CONFIG_JSON
        .read()
        .ok()
        .and_then(|g| g.clone());

    let lists = match vrc_json {
        Some(ref body) => parse_all_domain_lists(body, &inputs)
            .unwrap_or_else(|_| build_domain_lists_fallback_with_inputs(&inputs)),
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
        && let Ok(mut guard) = LATEST_COMMUNITY_CONFIG_JSON.write()
    {
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
        anyhow::bail!(
            "VRChat config request failed with HTTP status {}",
            response.status()
        );
    }

    Ok(response.text().await?)
}
