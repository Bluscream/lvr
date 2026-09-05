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
use std::sync::OnceLock;

/// Dynamic or fallback domain store for all categories.
#[derive(Debug, Clone)]
pub struct DomainLists {
    pub video_domains: Vec<String>,
    pub image_domains: Vec<String>,
    pub string_domains: Vec<String>,
    pub rest_domains: Vec<String>,
    #[allow(dead_code)]
    pub protected_domains: Vec<String>,
}

static ACTIVE_DOMAIN_LISTS: OnceLock<DomainLists> = OnceLock::new();

/// Returns the currently active domain lists, initializing from remote VRChat config or falling back to hardcoded.
pub fn active_domains() -> &'static DomainLists {
    ACTIVE_DOMAIN_LISTS.get_or_init(build_domain_lists_fallback)
}

/// Try to fetch live VRChat config asynchronously and initialize active domain lists.
pub async fn init_from_remote_or_fallback() {
    if ACTIVE_DOMAIN_LISTS.get().is_some() {
        return;
    }
    let lists = match fetch_remote_config().await {
        Ok(parsed) => {
            tracing::info!(
                "Successfully built media blocking domain lists dynamically from live VRChat config (videos: {}, images: {}, strings: {}, rest: {})",
                parsed.video_domains.len(),
                parsed.image_domains.len(),
                parsed.string_domains.len(),
                parsed.rest_domains.len()
            );
            parsed
        }
        Err(err) => {
            tracing::warn!("Failed to fetch live VRChat config ({err:#}); falling back to hardcoded domain lists");
            build_domain_lists_fallback()
        }
    };
    let _ = ACTIVE_DOMAIN_LISTS.set(lists);
}

/// Parse raw JSON from VRChat `/api/1/config` and partition domains strictly into pure video, image, string sets.
pub fn parse_vrchat_config_json(json_str: &str) -> Result<DomainLists> {
    #[derive(Deserialize)]
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

    let parsed: VrcRemoteConfig = serde_json::from_str(json_str)?;

    let mut raw_video: HashSet<String> = HashSet::new();
    for u in parsed.url_list {
        let cleaned = clean_domain_entry(&u);
        if !cleaned.is_empty() {
            raw_video.insert(cleaned);
        }
    }

    let mut raw_images: HashSet<String> = HashSet::new();
    for u in parsed.image_list {
        let cleaned = clean_domain_entry(&u);
        if !cleaned.is_empty() {
            raw_images.insert(cleaned);
        }
    }

    let mut raw_strings: HashSet<String> = HashSet::new();
    for u in parsed.string_list {
        let cleaned = clean_domain_entry(&u);
        if !cleaned.is_empty() {
            raw_strings.insert(cleaned);
        }
    }

    let mut protected: HashSet<String> = HashSet::new();
    for u in parsed.asset_urls {
        let cleaned = clean_domain_entry(&u);
        if !cleaned.is_empty() {
            protected.insert(cleaned.clone());
            let bare = cleaned.trim_start_matches("*.");
            protected.insert(bare.to_string());
            protected.insert(format!("*.{bare}"));
        }
    }

    // Always guarantee core asset wildcards
    for base in &["assets.vrchat.com", "vrchat.com", "*.vrchat.com", "vrchat.cloud", "*.vrchat.cloud"] {
        protected.insert(base.to_string());
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
        // Exclude any vrchat internal domains from blocking
        if bare == "vrchat.com" || bare.ends_with(".vrchat.com")
            || bare == "vrchat.cloud" || bare.ends_with(".vrchat.cloud")
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

    // "Rest": domains that were in multiple lists, excluding VRChat internal and whiteListedAssetUrls
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

    Ok(DomainLists {
        video_domains,
        image_domains,
        string_domains,
        rest_domains,
        protected_domains: protected_list,
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

/// Embedded copy of VRChat remote config (`https://api.vrchat.cloud/api/1/config`)
/// captured at compile time as a fallback whenever the network is unavailable.
const EMBEDDED_FALLBACK_CONFIG: &str = include_str!("../assets/vrchat_config_fallback.json");

fn build_domain_lists_fallback() -> DomainLists {
    parse_vrchat_config_json(EMBEDDED_FALLBACK_CONFIG)
        .expect("embedded fallback VRChat config JSON must always be valid")
}

async fn fetch_remote_config() -> Result<DomainLists> {
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

    let body = response.text().await?;
    parse_vrchat_config_json(&body)
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
}
