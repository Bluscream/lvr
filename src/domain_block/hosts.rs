use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::cache::active_domains;
use super::types::{BlockCategory, BlockState};

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
    let mut state = BlockState::default();

    for line in content.lines() {
        if let Some(cat) = BlockCategory::from_tag(line.trim()) {
            state.set_blocked(&cat, true);
        }
    }

    state
}

/// Set the blocking state for a given category.
pub fn set_category_blocked(prefix: &Path, category: &BlockCategory, block: bool) -> Result<()> {
    let mut state = read_block_state(prefix);
    state.set_blocked(category, block);
    sync_all(prefix, &state)?;
    Ok(())
}

/// Apply full block state (hosts + yt-dlp) cleanly in one atomic operation.
pub fn sync_all(prefix: &Path, state: &BlockState) -> Result<()> {
    update_hosts_file(prefix, state)?;
    update_ytdlp_file(prefix, state.video_blocked)?;
    Ok(())
}

fn update_hosts_file(prefix: &Path, state: &BlockState) -> Result<()> {
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
    let mut inside_lvr_block = false;

    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("# ----- BEGIN LVR ") && trimmed.ends_with(" BLOCK -----") {
            inside_lvr_block = true;
            continue;
        }
        if inside_lvr_block {
            if trimmed.starts_with("# ----- END LVR ") && trimmed.ends_with(" BLOCK -----") {
                inside_lvr_block = false;
            }
            continue;
        }
        cleaned.push(line);
    }

    let mut result = cleaned.join("\n");
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }

    // Keep track of hostnames already emitted in the file to guarantee zero duplicates.
    let mut emitted_hosts = HashSet::new();

    // Track existing non-LVR blocked hosts so we don't duplicate them either
    for line in &cleaned {
        let trimmed = line.trim();
        if trimmed.starts_with("0.0.0.0 ") || trimmed.starts_with("127.0.0.1 ") {
            for part in trimmed.split_whitespace().skip(1) {
                emitted_hosts.insert(part.to_lowercase());
            }
        }
    }

    // Append blocks for whichever categories are enabled
    let lists = active_domains();
    for cat in lists.all_categories() {
        if state.is_blocked(&cat) {
            let mut category_lines = Vec::new();

            for domain in lists.domains_for_category(&cat) {
                let bare = domain.trim_start_matches("*.");
                if bare.is_empty() || bare == "localhost" {
                    continue;
                }
                // Skip raw IP addresses in hosts file domain entries
                if bare.parse::<std::net::IpAddr>().is_ok() {
                    continue;
                }

                // Determine both base domain and www equivalent
                let (base, www) = if let Some(stripped) = bare.strip_prefix("www.") {
                    (stripped, bare)
                } else {
                    (bare, "")
                };

                // Emit base domain
                let base_lower = base.to_lowercase();
                if emitted_hosts.insert(base_lower.clone()) {
                    category_lines.push(format!("0.0.0.0 {base_lower}"));
                }

                // Always emit www equivalent as well, whether source had *. or not
                let www_lower = if www.is_empty() {
                    format!("www.{base_lower}")
                } else {
                    www.to_lowercase()
                };
                if emitted_hosts.insert(www_lower.clone()) {
                    category_lines.push(format!("0.0.0.0 {www_lower}"));
                }
            }

            if !category_lines.is_empty() {
                result.push_str(&cat.header_tag());
                result.push('\n');
                result.push_str(&format!(
                    "# Blocked by LinuxVR (lvr): {}\n",
                    cat.label()
                ));
                for line in category_lines {
                    result.push_str(&line);
                    result.push('\n');
                }
                result.push_str(&cat.footer_tag());
                result.push('\n');
            }
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
