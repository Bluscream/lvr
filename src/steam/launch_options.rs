//! Steam launch options inspection and modification.

use anyhow::Result;
use std::fs;
use std::time::SystemTime;

use super::VRCHAT_APPID;
use super::paths::SteamPaths;
use super::vdf::{edit_vdf, find_value};

/// Repeatedly reads one app's launch options without paying for it every time.
///
/// A bare [`read_launch_options`] is expensive to poll: profile discovery
/// `read_to_string`s *every* `localconfig.vdf` under `userdata` looking for the
/// app id (half a megabyte on a typical install), and then the winning file is
/// read a second time. This keeps the discovered profile and re-parses only
/// once the file's mtime moves — which covers our own writes too, since those
/// go through [`write_launch_options`].
#[derive(Debug, Default)]
pub struct LaunchOptionsCache {
    paths: Option<SteamPaths>,
    seen: Option<SystemTime>,
    value: String,
}

impl LaunchOptionsCache {
    /// The app's launch options, re-reading only when the file changed.
    ///
    /// On any failure the last known value is returned; discovery is retried on
    /// the next call, so a Steam install that appears later is picked up.
    pub fn read(&mut self, app_id: &str) -> String {
        let paths = match &self.paths {
            Some(paths) => paths,
            None => match SteamPaths::discover_for_app(app_id) {
                Ok(paths) => self.paths.insert(paths),
                Err(_) => {
                    self.value.clear();
                    self.seen = None;
                    return self.value.clone();
                }
            },
        };

        let modified = fs::metadata(&paths.localconfig_vdf).and_then(|m| m.modified());
        let Ok(modified) = modified else {
            // The profile moved or Steam was uninstalled; rediscover next call.
            self.paths = None;
            self.seen = None;
            return self.value.clone();
        };
        if self.seen == Some(modified) {
            return self.value.clone();
        }

        if let Ok(text) = fs::read_to_string(&paths.localconfig_vdf) {
            self.value = find_value(&text, &["apps", app_id, "LaunchOptions"]).unwrap_or_default();
            self.seen = Some(modified);
        }
        self.value.clone()
    }

    /// Launch options for VRChat.
    pub fn read_vrchat(&mut self) -> String {
        self.read(VRCHAT_APPID)
    }
}

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
