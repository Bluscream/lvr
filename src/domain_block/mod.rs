//! Simplified VRChat domain and media blocking system for LinuxVR (`lvr`).
//!
//! Fetches unified pre-compiled `domains.json` directly from GitHub,
//! with fallback to live official VRChat remote config, and final fallback
//! to embedded compile-time VRChat config.
//!
//! Domain blocking is enforced via the DNS shield (`liblvr_dns_shield.so`) and
//! yt-dlp stub.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::net::IpAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub mod dns_shield;
pub mod vrchat_config;

/// URL to download the pre-compiled, unified domains JSON from GitHub.
pub const DOMAINS_JSON_URL: &str =
    "https://raw.githubusercontent.com/Bluscream/lvr/main/assets/lists/domains.json";

/// Maximum age for the local cached domains.json before re-fetching (1 hour).
pub const DOMAINS_CACHE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(3600);

pub type DomainMap = BTreeMap<String, Vec<String>>;

static ACTIVE_DOMAIN_MAP: RwLock<Option<Arc<DomainMap>>> = RwLock::new(None);


/// Represents the toggleable blocking categories.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BlockCategory {
    Videos,
    Images,
    Strings,
    Shared,
    Custom(String),
}

impl BlockCategory {
    pub fn name(&self) -> &str {
        match self {
            Self::Videos => "Videos",
            Self::Images => "Images",
            Self::Strings => "Strings",
            Self::Shared => "Shared",
            Self::Custom(name) => name.as_str(),
        }
    }

    pub fn from_name(name: &str) -> Self {
        if name.eq_ignore_ascii_case("Videos") || name.eq_ignore_ascii_case("Video") {
            Self::Videos
        } else if name.eq_ignore_ascii_case("Images") || name.eq_ignore_ascii_case("Image") {
            Self::Images
        } else if name.eq_ignore_ascii_case("Strings") || name.eq_ignore_ascii_case("String") {
            Self::Strings
        } else if name.eq_ignore_ascii_case("Shared") || name.eq_ignore_ascii_case("Rest") {
            Self::Shared
        } else {
            Self::Custom(name.to_string())
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Videos => "Videos",
            Self::Images => "Images",
            Self::Strings => "Strings",
            Self::Shared => "Shared",
            Self::Custom(name) => name.as_str(),
        }
    }

    pub fn short_label(&self) -> &str {
        self.label()
    }
}

/// Wrapper for UI grids and domain summaries
#[derive(Debug, Clone, Default)]
pub struct DomainLists {
    pub domains: BTreeMap<String, Vec<String>>,
}

impl DomainLists {
    pub fn all_categories(&self) -> Vec<BlockCategory> {
        let mut cats = Vec::new();
        for key in self.domains.keys() {
            cats.push(BlockCategory::from_name(key));
        }
        if !cats.iter().any(|c| matches!(c, BlockCategory::Videos)) {
            cats.push(BlockCategory::Videos);
        }
        if !cats.iter().any(|c| matches!(c, BlockCategory::Images)) {
            cats.push(BlockCategory::Images);
        }
        if !cats.iter().any(|c| matches!(c, BlockCategory::Strings)) {
            cats.push(BlockCategory::Strings);
        }
        if !cats.iter().any(|c| matches!(c, BlockCategory::Shared)) {
            cats.push(BlockCategory::Shared);
        }
        cats.sort();
        cats.dedup();
        cats
    }

    pub fn count_for_category(&self, cat: &BlockCategory) -> usize {
        self.domains.get(cat.name()).map_or(0, |v| v.len())
    }

    pub fn total_count(&self) -> usize {
        self.domains.values().map(|v| v.len()).sum()
    }
}

/// Represents the on/off block state for all categories.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockState {
    pub video_blocked: bool,
    pub image_blocked: bool,
    pub string_blocked: bool,
    pub shared_blocked: bool,
    pub custom_blocked: BTreeMap<String, bool>,
}

impl BlockState {
    pub fn is_blocked(&self, category: &BlockCategory) -> bool {
        match category {
            BlockCategory::Videos => self.video_blocked,
            BlockCategory::Images => self.image_blocked,
            BlockCategory::Strings => self.string_blocked,
            BlockCategory::Shared => self.shared_blocked,
            BlockCategory::Custom(name) => {
                self.custom_blocked.get(name).copied().unwrap_or(false)
            }
        }
    }

    pub fn set_blocked(&mut self, category: &BlockCategory, blocked: bool) {
        match category {
            BlockCategory::Videos => self.video_blocked = blocked,
            BlockCategory::Images => self.image_blocked = blocked,
            BlockCategory::Strings => self.string_blocked = blocked,
            BlockCategory::Shared => self.shared_blocked = blocked,
            BlockCategory::Custom(name) => {
                self.custom_blocked.insert(name.clone(), blocked);
            }
        }
    }
}

/// Primary file path where downloaded `domains.json` is cached locally.
pub fn domains_cache_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "lvr")
        .map(|dirs| dirs.cache_dir().join("domains.json"))
        .unwrap_or_else(|| PathBuf::from("assets/lists/domains.json"))
}

pub fn is_file_fresh(path: &Path, max_age: std::time::Duration) -> bool {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .map(|elapsed| elapsed < max_age)
        .unwrap_or(false)
}

/// Fetches the pre-compiled `domains.json` from GitHub.
pub async fn fetch_domains_json_remote() -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent("lvr/0.1.0")
        .build()?;

    let response = client.get(DOMAINS_JSON_URL).send().await?;
    if !response.status().is_success() {
        anyhow::bail!("Failed to fetch domains.json: HTTP {}", response.status());
    }
    Ok(response.text().await?)
}

/// Filters out any direct IP addresses (IPv4 or IPv6) from domain lists.
pub fn sanitize_domain_map(raw: BTreeMap<String, Vec<String>>) -> BTreeMap<String, Vec<String>> {
    raw.into_iter()
        .map(|(cat, list)| {
            let filtered: Vec<String> = list
                .into_iter()
                .filter(|d| {
                    let bare = d.trim_start_matches("*.");
                    bare.parse::<IpAddr>().is_err() && d.parse::<IpAddr>().is_err()
                })
                .collect();
            (cat, filtered)
        })
        .collect()
}

/// Loads domains into memory with priority:
/// 1. Fresh local cached `domains.json` (< 1 hour old)
/// 2. Remote GitHub fetch of `domains.json`
/// 3. Live official VRChat remote config parsing (`https://api.vrchat.cloud/api/1/config`)
/// 4. Embedded compile-time VRChat fallback config
pub async fn load_domains() -> BTreeMap<String, Vec<String>> {
    let cache_file = domains_cache_path();

    // 1. Fresh local cache of domains.json
    if is_file_fresh(&cache_file, DOMAINS_CACHE_MAX_AGE)
        && let Ok(content) = fs::read_to_string(&cache_file)
        && let Ok(parsed) = serde_json::from_str::<BTreeMap<String, Vec<String>>>(&content)
    {
        tracing::debug!("Loaded domains from fresh local cache at {}", cache_file.display());
        return sanitize_domain_map(parsed);
    }

    // 2. Fetch pre-compiled domains.json from GitHub
    match fetch_domains_json_remote().await {
        Ok(body) => {
            if let Ok(parsed) = serde_json::from_str::<BTreeMap<String, Vec<String>>>(&body) {
                if let Some(parent) = cache_file.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::write(&cache_file, &body);
                tracing::info!("Fetched and cached domains.json from GitHub");
                return sanitize_domain_map(parsed);
            }
        }
        Err(err) => {
            tracing::warn!("Failed to fetch remote domains.json ({err:#}); falling back to VRChat config");
        }
    }

    // 3. Fall back to VRChat remote config
    match vrchat_config::fetch_vrchat_remote_config().await {
        Ok(vrc_body) => {
            if let Ok(parsed) = vrchat_config::parse_vrchat_config_to_categories(&vrc_body) {
                tracing::info!("Parsed domains from live VRChat remote config");
                return sanitize_domain_map(parsed);
            }
        }
        Err(err) => {
            tracing::warn!("Failed to fetch live VRChat config ({err:#}); falling back to embedded compile-time VRChat config");
        }
    }

    // 4. Embedded compile-time VRChat fallback config
    sanitize_domain_map(vrchat_config::get_embedded_fallback_categories())
}

/// Returns currently active domain lists in memory.
pub fn active_domains() -> DomainLists {
    if let Ok(guard) = ACTIVE_DOMAIN_MAP.read()
        && let Some(arc) = guard.as_ref()
    {
        return DomainLists {
            domains: (**arc).clone(),
        };
    }

    let default = Arc::new(vrchat_config::get_embedded_fallback_categories());
    if let Ok(mut guard) = ACTIVE_DOMAIN_MAP.write() {
        *guard = Some(default.clone());
    }
    DomainLists {
        domains: (*default).clone(),
    }
}

/// Initializes active domains asynchronously.
pub async fn init_domains() {
    let loaded = load_domains().await;
    if let Ok(mut guard) = ACTIVE_DOMAIN_MAP.write() {
        *guard = Some(Arc::new(loaded));
    }
}

pub async fn init_from_remote_or_fallback_with_config(_cfg: &crate::config::DomainBlockConfig) {
    init_domains().await;
}

pub async fn reload_domain_lists() -> DomainLists {
    init_domains().await;
    active_domains()
}

pub async fn reload_domain_lists_with_config(_cfg: &crate::config::DomainBlockConfig) -> DomainLists {
    reload_domain_lists().await
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

pub fn vrc_tools_dir(prefix: &Path) -> PathBuf {
    prefix.join("pfx/drive_c/users/steamuser/AppData/LocalLow/VRChat/VRChat/Tools")
}

/// Apply full block state (yt-dlp stub + dns_shield rules) cleanly in one atomic operation.
pub fn sync_all(prefix: &Path, state: &BlockState) -> Result<()> {
    update_ytdlp_file(prefix, state.video_blocked)?;
    let _ = dns_shield::sync_shield_rules(state);
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



    #[test]
    fn embedded_vrchat_fallback_parses_cleanly() {
        let parsed = vrchat_config::get_embedded_fallback_categories();
        assert!(parsed.contains_key("Videos"));
        assert!(parsed.contains_key("Images"));
        assert!(parsed.contains_key("Strings"));
        assert!(parsed.contains_key("Shared"));
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
        assert_eq!(video, &["youtube.com", "*.twitch.tv"]);
    }
}

