//! DNS Shield management and Steam Launch Options integration for VRChat.
//!
//! Provides:
//! 1. Active rules file sync (`~/.cache/lvr/shield_rules.json` / `$XDG_RUNTIME_DIR/lvr/shield_rules.json`).
//! 2. Installation and deployment of `liblvr_dns_shield.so`.
//! 3. Steam launch options detection and 1-click install/remove via `localconfig.vdf`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use anyhow::{Context, Result, anyhow, bail};

use crate::procs;
use super::{BlockState, active_domains};

pub const EMBEDDED_DNS_SHIELD_SO: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/liblvr_dns_shield.so"));

pub const VRCHAT_STEAM_APPID: &str = "438100";

/// Returns default deployment path for the DNS shield shared library.
pub fn installed_shield_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "lvr")
        .map(|dirs| dirs.data_dir().join("liblvr_dns_shield.so"))
        .unwrap_or_else(|| {
            directories::BaseDirs::new()
                .map(|b| b.home_dir().join(".local/share/lvr/liblvr_dns_shield.so"))
                .unwrap_or_else(|| PathBuf::from("/tmp/liblvr_dns_shield.so"))
        })
}

/// Returns paths where the active rules JSON file should be written for the shim.
pub fn shield_rules_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR")
        && !runtime.trim().is_empty()
    {
        paths.push(PathBuf::from(runtime).join("lvr/shield_rules.json"));
    }
    if let Some(dirs) = directories::ProjectDirs::from("", "", "lvr") {
        paths.push(dirs.cache_dir().join("shield_rules.json"));
    } else if let Some(base) = directories::BaseDirs::new() {
        paths.push(base.cache_dir().join("lvr/shield_rules.json"));
    }
    paths.push(PathBuf::from("/tmp/lvr_shield_rules.json"));
    paths
}

/// Deploys the embedded `liblvr_dns_shield.so` to the user's data directory.
pub fn deploy_shield_library() -> Result<PathBuf> {
    let target = installed_shield_path();
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating directory for {}", target.display()))?;
    }
    fs::write(&target, EMBEDDED_DNS_SHIELD_SO)
        .with_context(|| format!("writing {}", target.display()))?;
    Ok(target)
}

/// Serializes and writes currently active blocked domain/wildcard rules into
/// the shared JSON files read by `liblvr_dns_shield.so`.
pub fn sync_shield_rules(state: &BlockState) -> Result<()> {
    let lists = active_domains();
    let mut blocked_rules = Vec::new();

    for cat in lists.all_categories() {
        if state.is_blocked(&cat)
            && let Some(domains) = lists.domains.get(cat.name())
        {
            for d in domains {
                let trimmed = d.trim().to_lowercase();
                if !trimmed.is_empty() {
                    blocked_rules.push(trimmed);
                }
            }
        }
    }

    blocked_rules.sort();
    blocked_rules.dedup();

    let payload = serde_json::json!({
        "version": "0.1.0",
        "rules": blocked_rules,
    });

    let json_bytes = serde_json::to_vec_pretty(&payload)?;

    for path in shield_rules_paths() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, &json_bytes);
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// Steam VDF Launch Options Integration (AppID 438100)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamPaths {
    pub root: PathBuf,
    pub config_vdf: PathBuf,
    pub localconfig_vdf: PathBuf,
}

impl SteamPaths {
    pub fn discover() -> Result<Self> {
        let home = directories::BaseDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/root"));

        let roots = [
            home.join(".local/share/Steam"),
            home.join(".steam/steam"),
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
            PathBuf::from("/run/media/system/Data/Games/Steam"),
        ];

        let root = roots
            .into_iter()
            .find(|p| p.join("config/config.vdf").is_file())
            .ok_or_else(|| anyhow!("Could not locate Steam installation directory"))?;

        let config_vdf = root.join("config/config.vdf");
        let localconfig_vdf = find_vrc_localconfig(&root)?;

        Ok(Self {
            root,
            config_vdf,
            localconfig_vdf,
        })
    }
}

fn find_vrc_localconfig(root: &Path) -> Result<PathBuf> {
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

    let needle = format!("\"{VRCHAT_STEAM_APPID}\"");
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

/// Read the current Steam LaunchOptions string configured for VRChat.
pub fn read_steam_launch_options() -> Result<String> {
    let paths = SteamPaths::discover()?;
    let text = fs::read_to_string(&paths.localconfig_vdf)?;
    let val = find_value(&text, &["apps", VRCHAT_STEAM_APPID, "LaunchOptions"])
        .unwrap_or_default();
    Ok(val)
}

/// Checks whether `liblvr_dns_shield.so` is present in VRChat's launch options.
pub fn is_shield_in_launch_options(launch_opts: &str) -> bool {
    launch_opts.contains("liblvr_dns_shield.so")
}

/// Formats the recommended full launch string with DNS shield enabled.
pub fn recommended_launch_options(existing: &str) -> String {
    let shield_path = installed_shield_path();
    let shield_str = shield_path.to_string_lossy();
    let preload_flag = format!("LD_PRELOAD=\"{shield_str}\"");

    if existing.trim().is_empty() {
        return format!("{preload_flag} %command%");
    }

    if existing.contains("liblvr_dns_shield.so") {
        return existing.trim().to_string();
    }

    if let Some(pos) = existing.find("%command%") {
        let before = existing[..pos].trim();
        let after = existing[pos..].trim();
        if before.is_empty() {
            format!("{preload_flag} {after}")
        } else {
            format!("{before} {preload_flag} {after}")
        }
    } else {
        format!("{preload_flag} {existing} %command%")
    }
}

/// Removes DNS shield LD_PRELOAD from a launch options string.
pub fn remove_shield_from_launch_options(existing: &str) -> String {
    let mut parts: Vec<&str> = existing.split_whitespace().collect();
    parts.retain(|part| !part.contains("liblvr_dns_shield.so") && !part.starts_with("LD_PRELOAD="));
    parts.join(" ")
}

/// Updates Steam's `localconfig.vdf` for VRChat with new launch options.
pub fn write_steam_launch_options(new_options: &str) -> Result<()> {
    let paths = SteamPaths::discover()?;
    edit_vdf(
        &paths.localconfig_vdf,
        &["apps", VRCHAT_STEAM_APPID, "LaunchOptions"],
        new_options,
    )
}

/// Shut Steam down cleanly if running, wait for it to exit, then execute modification.
pub async fn set_steam_shield_enabled(enable: bool) -> Result<()> {
    let current = read_steam_launch_options().unwrap_or_default();
    let updated = if enable {
        deploy_shield_library()?;
        recommended_launch_options(&current)
    } else {
        remove_shield_from_launch_options(&current)
    };

    if current.trim() == updated.trim() {
        return Ok(());
    }

    // Shut down Steam if running so it doesn't overwrite VDF on exit
    let was_running = steam_running();
    if was_running {
        shutdown_steam().await?;
    }

    write_steam_launch_options(&updated)?;

    if was_running {
        start_steam().await?;
    }

    Ok(())
}

pub fn steam_running() -> bool {
    let mut scanner = procs::ProcessScanner::new();
    let snapshot = scanner.scan();
    snapshot.any_matching(&["steam.sh".into(), "/steam ".into(), "steamwebhelper".into()], &[])
}

pub async fn shutdown_steam() -> Result<()> {
    if !steam_running() {
        return Ok(());
    }
    let _ = procs::run_command_line("steam -shutdown").await;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if !steam_running() {
            tokio::time::sleep(Duration::from_millis(1000)).await;
            return Ok(());
        }
    }
    bail!("Steam is still running after shutdown command");
}

pub async fn start_steam() -> Result<()> {
    for cmd in ["bazzite-steam", "steam"] {
        if procs::which(cmd).is_some() {
            let _ = procs::spawn_command_line(cmd);
            return Ok(());
        }
    }
    procs::spawn_command_line("steam").map(|_| ())
}

// -----------------------------------------------------------------------------
// VDF parsing and atomic in-place value replacement
// -----------------------------------------------------------------------------

fn edit_vdf(path: &Path, key_path: &[&str], value: &str) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let updated = replace_value(&text, key_path, value)
        .ok_or_else(|| anyhow!("{} has no {} entry", path.display(), key_path.join(" / ")))?;
    if updated == text {
        return Ok(());
    }
    let backup = path.with_extension("vdf.lvr.bak");
    let _ = fs::write(&backup, &text);
    let tmp = path.with_extension("vdf.lvr.tmp");
    fs::write(&tmp, &updated).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

fn value_span(text: &str, key_path: &[&str]) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    let mut stack: Vec<usize> = Vec::new();
    let mut matched = 0usize;
    let mut pending_key: Option<String> = None;

    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                let (token, start, end) = read_string(text, index)?;
                index = end;
                match pending_key.take() {
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

fn read_string(text: &str, index: usize) -> Option<(String, usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => {
                cursor += 1;
                if cursor >= bytes.len() {
                    return None;
                }
                match bytes[cursor] {
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'\\' => out.push('\\'),
                    b'"' => out.push('"'),
                    other => {
                        out.push('\\');
                        out.push(other as char);
                    }
                }
                cursor += 1;
            }
            b'"' => return Some((out, index, cursor + 1)),
            byte => {
                out.push(byte as char);
                cursor += 1;
            }
        }
    }
    None
}

fn find_value(text: &str, key_path: &[&str]) -> Option<String> {
    let (start, end) = value_span(text, key_path)?;
    Some(unescape(&text[start..end]))
}

fn replace_value(text: &str, key_path: &[&str], value: &str) -> Option<String> {
    if let Some((start, end)) = value_span(text, key_path) {
        let mut out = String::with_capacity(text.len() + value.len());
        out.push_str(&text[..start]);
        out.push_str(&escape(value));
        out.push_str(&text[end..]);
        Some(out)
    } else {
        None
    }
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommended_launch_options_appends_or_prepends_preload() {
        let existing = "--enable-avpro-in-proton %command% --foo";
        let rec = recommended_launch_options(existing);
        assert!(rec.contains("liblvr_dns_shield.so"));
        assert!(rec.contains("--enable-avpro-in-proton"));
        assert!(rec.contains("%command% --foo"));
    }

    #[test]
    fn remove_shield_strips_preload_flag() {
        let current = "LD_PRELOAD=\"/path/to/liblvr_dns_shield.so\" %command% --foo";
        let cleaned = remove_shield_from_launch_options(current);
        assert!(!cleaned.contains("liblvr_dns_shield.so"));
        assert_eq!(cleaned, "%command% --foo");
    }
}
