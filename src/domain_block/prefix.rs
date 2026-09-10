//! Prefix discovery and recoverable hosts/media updates.
use super::*;

/// Resolve the VRChat Proton prefix directory.
pub fn detect_vrc_prefix(configured_prefix: &str) -> Option<PathBuf> {
    if !configured_prefix.trim().is_empty() {
        let p = PathBuf::from(expand_tilde(configured_prefix.trim()));
        return p.is_dir().then_some(p);
    }

    for root in crate::steam::paths::library_roots() {
        let prefix = root.join("steamapps/compatdata/438100");
        if prefix.is_dir() {
            return Some(prefix);
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
    if prefix
        .join("pfx/drive_c/windows/system32/drivers/etc/hosts")
        .exists()
        || prefix.join("pfx/drive_c").exists()
    {
        prefix.join("pfx/drive_c/windows/system32/drivers/etc/hosts")
    } else {
        prefix.join("drive_c/windows/system32/drivers/etc/hosts")
    }
}

pub fn vrc_tools_dir(prefix: &Path) -> PathBuf {
    let pfx = if prefix.join("pfx/drive_c").is_dir() {
        prefix.join("pfx")
    } else {
        prefix.to_path_buf()
    };
    pfx.join("drive_c/users/steamuser/AppData/LocalLow/VRChat/VRChat/Tools")
}

/// Read current blocking state of all categories from prefix hosts file.
pub fn read_block_state(prefix: &Path) -> BlockState {
    let hosts_path = prefix_hosts_path(prefix);
    let content = fs::read_to_string(&hosts_path).unwrap_or_default();
    let mut state = BlockState::default();

    for line in content.lines() {
        if let Some(cat) = BlockCategory::from_tag(line.trim()) {
            state.set_blocked(&cat, true);
        }
    }

    // Also check prefix hosts.d/ snippets
    if let Some(parent) = hosts_path.parent() {
        let hosts_d = parent.join("hosts.d");
        for cat in BlockCategory::all_standard() {
            let snippet = hosts_d.join(format!("lvr_{}.hosts", cat.name().to_lowercase()));
            if snippet.is_file() {
                state.set_blocked(&cat, true);
            }
        }
    }

    state
}

/// Updates the prefix hosts file using `hostsfile::HostsBuilder`.
pub fn update_hosts_file(prefix: &Path, state: &BlockState) -> Result<()> {
    let hosts_path = prefix_hosts_path(prefix);
    if let Some(parent) = hosts_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let original = match fs::read(&hosts_path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            b"127.0.0.1 localhost\n::1 localhost\n".to_vec()
        }
        Err(err) => return Err(err).with_context(|| format!("reading {}", hosts_path.display())),
    };
    // All category edits happen in a private file; readers see one complete replacement.
    let staged =
        tempfile::NamedTempFile::new_in(hosts_path.parent().context("hosts parent missing")?)?;
    fs::write(staged.path(), &original)?;
    let lists = active_domains();
    let zero_ip: IpAddr = "0.0.0.0".parse()?;

    let mut categories = lists.all_categories();
    for name in state.custom_blocked.keys() {
        categories.push(BlockCategory::from_name(name));
    }
    for line in String::from_utf8_lossy(&original).lines() {
        if let Some(cat) = BlockCategory::from_tag(line) {
            categories.push(cat);
        }
    }
    categories.sort();
    categories.dedup();
    for cat in categories {
        let mut builder = hostsfile::HostsBuilder::new(cat.tag_name());
        if state.is_blocked(&cat)
            && let Some(domain_list) = lists.domains.get(cat.name())
        {
            let valid_hostnames: Vec<&String> = domain_list
                .iter()
                .filter(|d| {
                    d.parse::<IpAddr>().is_err()
                        && !d.trim_start_matches("*.").parse::<IpAddr>().is_ok()
                })
                .collect();
            if !valid_hostnames.is_empty() {
                builder.add_hostnames(zero_ip, valid_hostnames);
            }
        }

        builder
            .write_to(staged.path())
            .map_err(|e| anyhow::anyhow!("Failed writing hosts file: {e}"))?;
    }

    let updated = fs::read(staged.path())?;
    if updated != original || !hosts_path.is_file() {
        crate::files::atomic_write(&hosts_path, &updated)?;
    }
    // Remove only our named snippets after their rules have been consolidated.
    if let Some(parent) = hosts_path.parent() {
        for cat in BlockCategory::all_standard() {
            let snippet = parent
                .join("hosts.d")
                .join(format!("lvr_{}.hosts", cat.name().to_lowercase()));
            match fs::remove_file(&snippet) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!("removing managed snippet {}", snippet.display())
                    });
                }
            }
        }
    }

    Ok(())
}

/// Apply hosts atomically, then update the media executable with a recoverable backup.
pub fn sync_all(prefix: &Path, state: &BlockState) -> Result<()> {
    update_hosts_file(prefix, state)?;
    update_ytdlp_file(prefix, state.video_blocked)?;
    Ok(())
}

fn update_ytdlp_file(prefix: &Path, block_video: bool) -> Result<()> {
    let tools_dir = vrc_tools_dir(prefix);
    let target = tools_dir.join("yt-dlp.exe");
    let backup = tools_dir.join("yt-dlp.exe.lvr_orig");
    const STUB: &[u8] = b"MZ\x00\x00ERROR: Video disabled by lvr\r\n";
    if block_video {
        if !target.is_file() {
            return Ok(());
        }
        let original = fs::read(&target)?;
        if original == STUB {
            return Ok(());
        }
        // Never destroy the executable unless its backup was saved successfully.
        if !backup.is_file() {
            crate::files::atomic_write(&backup, &original)?;
        }
        crate::files::atomic_write(&target, STUB)?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o444))?;
    } else if backup.is_file() {
        let bytes = fs::read(&backup)?;
        crate::files::atomic_write(&target, &bytes)?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
        fs::remove_file(&backup)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_backup_does_not_destroy_video_tool() {
        let root = tempfile::tempdir().unwrap();
        let tools = vrc_tools_dir(root.path());
        fs::create_dir_all(&tools).unwrap();
        let target = tools.join("yt-dlp.exe");
        fs::write(&target, b"original executable").unwrap();
        fs::create_dir(tools.join("yt-dlp.exe.lvr_orig")).unwrap();
        assert!(update_ytdlp_file(root.path(), true).is_err());
        assert_eq!(fs::read(target).unwrap(), b"original executable");
    }
    #[test]
    fn block_unblock_is_repeatable_and_preserves_unmanaged_hosts() {
        let root = tempfile::tempdir().unwrap();
        let hosts = prefix_hosts_path(root.path());
        fs::create_dir_all(hosts.parent().unwrap()).unwrap();
        fs::write(&hosts, "127.0.0.1 localhost\n127.0.0.9 personal.local\n").unwrap();
        let tools = vrc_tools_dir(root.path());
        fs::create_dir_all(&tools).unwrap();
        fs::write(tools.join("yt-dlp.exe"), b"original executable").unwrap();
        let state = BlockState {
            video_blocked: true,
            ..Default::default()
        };
        sync_all(root.path(), &state).unwrap();
        assert!(read_block_state(root.path()).video_blocked);
        if let Some(library) = std::env::var_os("LVR_TEST_DNS_LIBRARY") {
            let domain = active_domains().domains["Videos"][0]
                .trim_start_matches("*.")
                .to_string();
            let output = std::process::Command::new("python3")
                .args(["-c", "import socket,sys; assert socket.getaddrinfo(sys.argv[1],80,socket.AF_INET)[0][4][0] == '0.0.0.0'", &domain])
                .env("LD_PRELOAD", library).env("WINEPREFIX", root.path()).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let once = fs::read(&hosts).unwrap();
        sync_all(root.path(), &state).unwrap();
        assert_eq!(fs::read(&hosts).unwrap(), once);
        sync_all(root.path(), &BlockState::default()).unwrap();
        assert!(!read_block_state(root.path()).video_blocked);
        assert!(
            fs::read_to_string(&hosts)
                .unwrap()
                .contains("127.0.0.9 personal.local")
        );
        assert_eq!(
            fs::read(tools.join("yt-dlp.exe")).unwrap(),
            b"original executable"
        );
        assert!(!tools.join("yt-dlp.exe.lvr_orig").exists());
    }
    #[test]
    fn explicit_missing_prefix_never_falls_back_to_another_game() {
        assert!(detect_vrc_prefix("/definitely-missing-lvr-prefix").is_none());
    }
}
