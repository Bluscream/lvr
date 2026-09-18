//! Simplified VRChat domain and media blocking system for LinuxVR (`lvr`).
//!
//! Fetches unified pre-compiled `domains.json` directly from GitHub,
//! with a bundled offline fallback. Official-only mode uses VRChat config.
//!
//! Domain blocking is enforced via the prefix hosts file (managed with the `hostsfile` crate),
//! the DNS shield (`liblvr_dns_shield.so`), and the yt-dlp stub.

use std::collections::BTreeMap;
use std::fs;
use std::net::IpAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

mod data;
pub mod dns_shield;
mod prefix;
#[cfg(test)]
mod tests;
pub mod vrchat_config;
#[cfg(test)]
use data::sanitize_domain_map;
pub use data::{
    active_domains, init_from_remote_or_fallback_with_config, reload_domain_lists_with_config,
};
pub use prefix::{detect_vrc_prefix, prefix_hosts_path, read_block_state, sync_all, vrc_tools_dir};

/// URL to download the pre-compiled, unified domains JSON from GitHub.
pub const DOMAINS_JSON_URL: &str =
    "https://raw.githubusercontent.com/Bluscream/lvr/main/assets/lists/domains.json";

/// Maximum age for the local cached domains.json before re-fetching (1 hour).
pub const DOMAINS_CACHE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(3600);

pub type DomainMap = BTreeMap<String, Vec<String>>;

static ACTIVE_DOMAIN_MAP: RwLock<Option<Arc<DomainMap>>> = RwLock::new(None);

/// Bumped every time [`ACTIVE_DOMAIN_MAP`] is replaced, so pollers can tell
/// whether the lists changed without cloning and comparing thousands of
/// domains.
static ACTIVE_DOMAIN_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Current revision of the active domain lists. Only equality is meaningful.
pub fn active_domain_generation() -> u64 {
    ACTIVE_DOMAIN_GENERATION.load(Ordering::SeqCst)
}

/// Install a new active domain map and publish it as a new generation.
fn set_active_domain_map(map: DomainMap) {
    *ACTIVE_DOMAIN_MAP.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(map));
    ACTIVE_DOMAIN_GENERATION.fetch_add(1, Ordering::SeqCst);
}

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
        match name {
            "Videos" => Self::Videos,
            "Images" => Self::Images,
            "Strings" => Self::Strings,
            "Shared" => Self::Shared,
            other => Self::Custom(other.to_string()),
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

    pub fn all_standard() -> [Self; 4] {
        [Self::Videos, Self::Images, Self::Strings, Self::Shared]
    }

    /// Returns the tag used by `hostsfile::HostsBuilder` (e.g. `LVR_Videos`).
    pub fn tag_name(&self) -> String {
        format!("LVR_{}", self.name())
    }

    /// Parses a category from a tag line or tag comment.
    pub fn from_tag(tag: &str) -> Option<Self> {
        let tag = tag.trim();
        // Standard hostsfile crate tag format: "# DO NOT EDIT LVR_<name> BEGIN"
        if let Some(rest) = tag.strip_prefix("# DO NOT EDIT LVR_")
            && let Some(name) = rest.strip_suffix(" BEGIN")
        {
            return Some(Self::from_name(name));
        }
        // Direct tag name: "LVR_<name>"
        if let Some(name) = tag.strip_prefix("LVR_") {
            return Some(Self::from_name(name));
        }
        None
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

    pub fn blocked_count(&self, state: &BlockState) -> usize {
        self.all_categories()
            .iter()
            .filter(|cat| state.is_blocked(cat))
            .map(|cat| self.count_for_category(cat))
            .sum()
    }
}

/// Represents the on/off block state for all categories.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
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
            BlockCategory::Custom(name) => self.custom_blocked.get(name).copied().unwrap_or(false),
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
