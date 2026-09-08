//! Steam launch options inspection and modification.

use anyhow::Result;
use std::fs;

use super::paths::SteamPaths;
use super::vdf::{edit_vdf, find_value};
use super::VRCHAT_APPID;

/// Read the current Steam LaunchOptions string configured for an app.
pub fn read_launch_options(app_id: &str) -> Result<String> {
    let paths = SteamPaths::discover_for_app(app_id)?;
    let text = fs::read_to_string(&paths.localconfig_vdf)?;
    let val = find_value(&text, &["apps", app_id, "LaunchOptions"]).unwrap_or_default();
    Ok(val)
}

/// Read the current Steam LaunchOptions string configured for VRChat.
pub fn read_vrchat_launch_options() -> Result<String> {
    read_launch_options(VRCHAT_APPID)
}

/// Writes new launch options for an app into Steam's localconfig.vdf.
pub fn write_launch_options(app_id: &str, new_options: &str) -> Result<()> {
    let paths = SteamPaths::discover_for_app(app_id)?;
    edit_vdf(
        &paths.localconfig_vdf,
        &["apps", app_id, "LaunchOptions"],
        new_options,
    )
}

/// Writes new launch options for VRChat into Steam's localconfig.vdf.
pub fn write_vrchat_launch_options(new_options: &str) -> Result<()> {
    write_launch_options(VRCHAT_APPID, new_options)
}
