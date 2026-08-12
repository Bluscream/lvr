//! Switching VRChat's Proton profile: compat tool plus launch options.
//!
//! Two named profiles ("Latest VRC" and "Video comp") each pin a compat tool
//! and a launch-option string for one Steam AppID. Steam rewrites both VDF
//! files when it exits, so applying a profile means: shut Steam down, edit the
//! files, start Steam again.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::config::SteamConfig;
use crate::procs;

/// Where the VDF files for one Steam installation live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamPaths {
    pub root: PathBuf,
    pub config_vdf: PathBuf,
    /// `userdata/<id>/config/localconfig.vdf` for the user that owns the app.
    pub localconfig_vdf: PathBuf,
}

impl SteamPaths {
    /// Locate Steam and the user profile that has `app_id` in its app list.
    pub fn discover(config: &SteamConfig, app_id: &str) -> Result<Self> {
        let root = if config.steam_root.trim().is_empty() {
            default_roots()
                .into_iter()
                .find(|p| p.join("config/config.vdf").is_file())
                .ok_or_else(|| anyhow!("could not find a Steam installation"))?
        } else {
            PathBuf::from(expand_tilde(config.steam_root.trim()))
        };

        let config_vdf = root.join("config/config.vdf");
        if !config_vdf.is_file() {
            bail!("{} does not exist", config_vdf.display());
        }

        let localconfig_vdf = if config.localconfig_vdf.trim().is_empty() {
            find_localconfig(&root, app_id)?
        } else {
            PathBuf::from(expand_tilde(config.localconfig_vdf.trim()))
        };
        if !localconfig_vdf.is_file() {
            bail!("{} does not exist", localconfig_vdf.display());
        }

        Ok(Self {
            root,
            config_vdf,
            localconfig_vdf,
        })
    }
}

fn default_roots() -> Vec<PathBuf> {
    let home = directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/root"));
    vec![
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ]
}

fn expand_tilde(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => directories::BaseDirs::new()
            .map(|d| d.home_dir().join(rest).to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string()),
        None => path.to_string(),
    }
}

/// Pick the `userdata` profile whose localconfig mentions the app.
fn find_localconfig(root: &Path, app_id: &str) -> Result<PathBuf> {
    let userdata = root.join("userdata");
    let entries = std::fs::read_dir(&userdata)
        .with_context(|| format!("reading {}", userdata.display()))?;
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join("config/localconfig.vdf");
        if path.is_file() {
            candidates.push(path);
        }
    }
    if candidates.is_empty() {
        bail!("no localconfig.vdf under {}", userdata.display());
    }
    let needle = format!("\"{app_id}\"");
    for path in &candidates {
        if std::fs::read_to_string(path)
            .map(|text| text.contains(&needle))
            .unwrap_or(false)
        {
            return Ok(path.clone());
        }
    }
    // Nothing mentions the app yet; the only profile is still the best guess.
    Ok(candidates.remove(0))
}

/// What is configured for the app right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppSetup {
    pub compat_tool: String,
    pub launch_options: String,
}

pub fn read_setup(paths: &SteamPaths, app_id: &str) -> Result<AppSetup> {
    let config_text = std::fs::read_to_string(&paths.config_vdf)
        .with_context(|| format!("reading {}", paths.config_vdf.display()))?;
    let local_text = std::fs::read_to_string(&paths.localconfig_vdf)
        .with_context(|| format!("reading {}", paths.localconfig_vdf.display()))?;
    Ok(AppSetup {
        compat_tool: find_value(&config_text, &["CompatToolMapping", app_id, "name"])
            .unwrap_or_default(),
        launch_options: find_value(&local_text, &["apps", app_id, "LaunchOptions"])
            .unwrap_or_default(),
    })
}

/// Name of the configured profile that matches the current on-disk state.
pub fn active_profile(config: &SteamConfig, setup: &AppSetup) -> Option<String> {
    config
        .profiles
        .iter()
        .find(|p| p.compat_tool == setup.compat_tool)
        .map(|p| p.name.clone())
}

/// Write both VDF files. Steam must not be running.
pub fn write_setup(paths: &SteamPaths, app_id: &str, setup: &AppSetup) -> Result<()> {
    edit_vdf(
        &paths.config_vdf,
        &["CompatToolMapping", app_id, "name"],
        &setup.compat_tool,
    )?;
    edit_vdf(
        &paths.localconfig_vdf,
        &["apps", app_id, "LaunchOptions"],
        &setup.launch_options,
    )?;
    Ok(())
}

/// Replace one leaf value, keeping the rest of the file byte-for-byte intact.
/// Keeps a `.lvr.bak` copy of the previous contents.
fn edit_vdf(path: &Path, key_path: &[&str], value: &str) -> Result<()> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let updated = replace_value(&text, key_path, value)
        .ok_or_else(|| anyhow!("{} has no {} entry", path.display(), key_path.join(" / ")))?;
    if updated == text {
        return Ok(());
    }
    let backup = path.with_extension("vdf.lvr.bak");
    std::fs::write(&backup, &text).with_context(|| format!("writing {}", backup.display()))?;
    let tmp = path.with_extension("vdf.lvr.tmp");
    std::fs::write(&tmp, &updated).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Shut Steam down and wait for it to go away, so it cannot overwrite the VDFs.
pub async fn shutdown_steam(config: &SteamConfig) -> Result<()> {
    if !steam_running() {
        return Ok(());
    }
    if let Err(err) = procs::run_command_line(config.shutdown_command.trim()).await {
        tracing::warn!("steam shutdown command failed: {err:#}");
    }
    let deadline = std::time::Instant::now()
        + Duration::from_secs(config.shutdown_timeout_secs.clamp(5, 300));
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if !steam_running() {
            // Steam flushes its config a moment after the process exits.
            tokio::time::sleep(Duration::from_millis(1500)).await;
            return Ok(());
        }
    }
    bail!("Steam is still running after {}s", config.shutdown_timeout_secs);
}

pub fn steam_running() -> bool {
    let mut scanner = procs::ProcessScanner::new();
    let snapshot = scanner.scan();
    snapshot.any_matching(&["steam.sh".into(), "/steam ".into(), "steamwebhelper".into()], &[])
}

pub async fn start_steam(config: &SteamConfig) -> Result<()> {
    procs::spawn_command_line(config.start_command.trim()).map(|_| ())
}

// --------------------------------------------------------------- tiny VDF bits

/// Walk a Steam text VDF and return the byte range of the value belonging to
/// `key_path`. Matching is case-insensitive (Steam is inconsistent) and the
/// path may skip intermediate levels, so `["apps", "438100", "LaunchOptions"]`
/// finds the key wherever the `apps` block happens to sit.
fn value_span(text: &str, key_path: &[&str]) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    // One entry per open brace: how many path elements were matched at it.
    let mut stack: Vec<usize> = Vec::new();
    let mut matched = 0usize;
    let mut pending_key: Option<String> = None;

    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                let (token, start, end) = read_string(text, index)?;
                index = end;
                match pending_key.take() {
                    // Two strings in a row: key followed by its value.
                    Some(key) => {
                        if matched == key_path.len() - 1
                            && key.eq_ignore_ascii_case(key_path[matched])
                        {
                            return Some((start + 1, end - 1));
                        }
                    }
                    None => pending_key = Some(token),
                }
            }
            b'{' => {
                let key = pending_key.take().unwrap_or_default();
                stack.push(matched);
                if matched < key_path.len() - 1 && key.eq_ignore_ascii_case(key_path[matched]) {
                    matched += 1;
                }
                index += 1;
            }
            b'}' => {
                matched = stack.pop().unwrap_or(0);
                pending_key = None;
                index += 1;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += text[index..].find('\n').map(|n| n + 1).unwrap_or(bytes.len() - index);
            }
            _ => index += 1,
        }
    }
    None
}

/// Read the quoted string starting at `index`; returns (unescaped contents,
/// opening-quote offset, offset just past the closing quote).
fn read_string(text: &str, index: usize) -> Option<(String, usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' if cursor + 1 < bytes.len() => {
                out.push(match bytes[cursor + 1] {
                    b'n' => '\n',
                    b't' => '\t',
                    other => other as char,
                });
                cursor += 2;
            }
            b'"' => return Some((out, index, cursor + 1)),
            other => {
                out.push(other as char);
                cursor += 1;
            }
        }
    }
    None
}

fn find_value(text: &str, key_path: &[&str]) -> Option<String> {
    let (start, end) = value_span(text, key_path)?;
    read_string(text, start - 1).map(|(value, _, _)| value).or(Some(text[start..end].to_string()))
}

fn replace_value(text: &str, key_path: &[&str], value: &str) -> Option<String> {
    let (start, end) = value_span(text, key_path)?;
    let mut out = String::with_capacity(text.len() + value.len());
    out.push_str(&text[..start]);
    out.push_str(&escape(value));
    out.push_str(&text[end..]);
    Some(out)
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
"InstallConfigStore"
{
    "Software"
    {
        "Valve"
        {
            "Steam"
            {
                "CompatToolMapping"
                {
                    "0"
                    {
                        "name"      "proton_experimental"
                        "priority"  "75"
                    }
                    "438100"
                    {
                        "name"      "GE-Proton9-25"
                        "config"    ""
                        "priority"  "250"
                    }
                }
            }
        }
    }
}
"#;

    const LOCAL: &str = r#"
"UserLocalConfigStore"
{
    "Software"
    {
        "Valve"
        {
            "Steam"
            {
                "apps"
                {
                    "620980"
                    {
                        "LaunchOptions"     "%command% --other"
                    }
                    "438100"
                    {
                        "LaunchOptions"     "WINEDLLOVERRIDES=\"iyuv_32=\" %command% --enable-hw-video-decoding"
                        "Playtime2wks"      "8239"
                    }
                }
            }
        }
    }
    "system"
    {
        "LaunchOptions"     "not this one"
    }
}
"#;

    #[test]
    fn reads_the_compat_tool_and_launch_options_of_one_app() {
        assert_eq!(
            find_value(CONFIG, &["CompatToolMapping", "438100", "name"]).as_deref(),
            Some("GE-Proton9-25")
        );
        assert_eq!(
            find_value(LOCAL, &["apps", "438100", "LaunchOptions"]).as_deref(),
            Some(r#"WINEDLLOVERRIDES="iyuv_32=" %command% --enable-hw-video-decoding"#)
        );
        // Sibling apps and same-named keys elsewhere must not be picked up.
        assert_eq!(
            find_value(LOCAL, &["apps", "620980", "LaunchOptions"]).as_deref(),
            Some("%command% --other")
        );
        assert_eq!(find_value(CONFIG, &["CompatToolMapping", "999", "name"]), None);
    }

    #[test]
    fn replacing_a_value_touches_nothing_else() {
        let updated =
            replace_value(CONFIG, &["CompatToolMapping", "438100", "name"], "Proton-GE RTSP Latest")
                .expect("entry exists");
        assert!(updated.contains(r#""name"      "Proton-GE RTSP Latest""#));
        assert!(updated.contains(r#""name"      "proton_experimental""#));
        assert_eq!(updated.len(), CONFIG.len() + "Proton-GE RTSP Latest".len() - "GE-Proton9-25".len());
    }

    #[test]
    fn replacing_launch_options_escapes_quotes() {
        let wanted = r#"WINEDLLOVERRIDES="iyuv_32=" %command% --enable-avpro-in-proton"#;
        let updated = replace_value(LOCAL, &["apps", "438100", "LaunchOptions"], wanted)
            .expect("entry exists");
        assert!(updated.contains(r#"WINEDLLOVERRIDES=\"iyuv_32=\" %command% --enable-avpro-in-proton"#));
        assert_eq!(
            find_value(&updated, &["apps", "438100", "LaunchOptions"]).as_deref(),
            Some(wanted)
        );
        assert!(updated.contains(r#""LaunchOptions"     "not this one""#));
    }

    #[test]
    fn active_profile_matches_on_the_compat_tool() {
        let config = SteamConfig::default();
        let setup = AppSetup {
            compat_tool: "GE-Proton9-25".into(),
            launch_options: String::new(),
        };
        assert_eq!(active_profile(&config, &setup).as_deref(), Some("Video comp"));
        let setup = AppSetup {
            compat_tool: "something-else".into(),
            launch_options: String::new(),
        };
        assert_eq!(active_profile(&config, &setup), None);
    }
}
