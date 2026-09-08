//! Steam installation discovery and path resolution.

use std::fs;
use std::path::{Path, PathBuf};
use anyhow::{Context, Result, anyhow, bail};

use super::VRCHAT_APPID;

/// Where the VDF files for one Steam installation live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamPaths {
    pub root: PathBuf,
    pub config_vdf: PathBuf,
    pub localconfig_vdf: PathBuf,
}

impl SteamPaths {
    /// Locate Steam and the user profile that has `VRCHAT_APPID` in its app list.
    #[allow(dead_code)]
    pub fn discover() -> Result<Self> {
        Self::discover_for_app(VRCHAT_APPID)
    }

    /// Locate Steam and the user profile that has `app_id` in its app list.
    pub fn discover_for_app(app_id: &str) -> Result<Self> {
        let roots = default_roots();
        let root = roots
            .into_iter()
            .find(|p| p.join("config/config.vdf").is_file())
            .ok_or_else(|| anyhow!("Could not locate Steam installation directory"))?;

        let config_vdf = root.join("config/config.vdf");
        let localconfig_vdf = find_localconfig(&root, app_id)?;

        Ok(Self {
            root,
            config_vdf,
            localconfig_vdf,
        })
    }
}

pub fn default_roots() -> Vec<PathBuf> {
    let home = directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/root"));

    vec![
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        PathBuf::from("/run/media/system/Data/Games/Steam"),
    ]
}

/// Pick the `userdata` profile whose localconfig mentions the app.
pub fn find_localconfig(root: &Path, app_id: &str) -> Result<PathBuf> {
    let userdata = root.join("userdata");
    let entries = fs::read_dir(&userdata)
        .with_context(|| format!("reading {}", userdata.display()))?;

    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join("config/localconfig.vdf");
        if path.is_file() {
            candidates.push(path);
        }
    }

    if candidates.is_empty() {
        bail!("No localconfig.vdf found under {}", userdata.display());
    }

    let needle = format!("\"{app_id}\"");
    for path in &candidates {
        if fs::read_to_string(path)
            .map(|text| text.contains(&needle))
            .unwrap_or(false)
        {
            return Ok(path.clone());
        }
    }

    Ok(candidates.remove(0))
}
