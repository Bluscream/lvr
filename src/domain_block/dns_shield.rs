//! DNS Shield management and Steam Launch Options integration for VRChat.
//!
//! Provides:
//! 1. Active rules file sync (prefix `hosts` and `hosts.d/` drop-ins).
//! 2. Installation and deployment of `liblvr_dns_shield.so`.
//! 3. Steam launch options formatting and 1-click install/remove via `crate::steam`.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

use crate::steam;

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

/// Install the single audited Rust resolver, replacing its inode atomically.
pub fn deploy_shield_library() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().context("cannot locate home directory")?;
    let installed = installed_shield_path();
    let source = std::env::var_os("LVR_DNS_LIBRARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let default = base.home_dir().join(".local/lib/libgetaddrinfo.so");
            if default.is_file() {
                default
            } else {
                installed.clone()
            }
        });
    let bytes = fs::read(&source).with_context(|| {
        format!(
            "reading {}; install getaddrinfo-rs with scripts/build.sh --deploy first",
            source.display()
        )
    })?;
    anyhow::ensure!(
        bytes.starts_with(b"\x7fELF"),
        "{} is not an ELF library",
        source.display()
    );
    let target = installed_shield_path();
    crate::files::atomic_write(&target, &bytes)?;
    Ok(target)
}

// -----------------------------------------------------------------------------
// Steam Launch Options formatting & integration
// -----------------------------------------------------------------------------

/// Shell-word spans preserve all unrelated quoting, wrappers and game arguments.
fn word_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            start.get_or_insert(index);
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            start.get_or_insert(index);
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if let Some(begin) = start.take() {
                spans.push((begin, index));
            }
        } else {
            start.get_or_insert(index);
        }
    }
    if let Some(begin) = start {
        spans.push((begin, text.len()));
    }
    spans
}

fn preload_word(word: &str) -> Option<String> {
    let words = shell_words::split(word).ok()?;
    words
        .first()?
        .strip_prefix("LD_PRELOAD=")
        .map(str::to_owned)
}

fn is_our_library(path: &str) -> bool {
    std::path::Path::new(path)
        .file_name()
        .is_some_and(|n| n == "liblvr_dns_shield.so")
}

pub fn is_shield_in_launch_options(options: &str) -> bool {
    word_spans(options)
        .into_iter()
        .take_while(|(a, b)| &options[*a..*b] != "%command%")
        .filter_map(|(a, b)| preload_word(&options[a..b]))
        .any(|value| value.split([':', ' ']).any(is_our_library))
}

/// Add our library without discarding another interceptor or changing game arguments.
pub fn recommended_launch_options(existing: &str) -> String {
    if is_shield_in_launch_options(existing) {
        return existing.trim().to_string();
    }
    let path = installed_shield_path().to_string_lossy().into_owned();
    for (a, b) in word_spans(existing) {
        if &existing[a..b] == "%command%" {
            break;
        }
        if let Some(value) = preload_word(&existing[a..b]) {
            let value = if value.is_empty() {
                path
            } else {
                format!("{value}:{path}")
            };
            return format!(
                "{}LD_PRELOAD={}{}",
                &existing[..a],
                shell_words::quote(&value),
                &existing[b..]
            );
        }
    }
    let assignment = format!("LD_PRELOAD={}", shell_words::quote(&path));
    if let Some(pos) = existing.find("%command%") {
        format!("{}{} {}", &existing[..pos], assignment, &existing[pos..])
            .trim()
            .to_string()
    } else {
        if existing.trim().is_empty() {
            format!("{assignment} %command%")
        } else {
            format!("{assignment} {} %command%", existing.trim())
        }
    }
}

/// Remove only our library from pre-command LD_PRELOAD assignments.
pub fn remove_shield_from_launch_options(existing: &str) -> String {
    let mut result = existing.to_string();
    let spans: Vec<_> = word_spans(existing)
        .into_iter()
        .take_while(|(a, b)| &existing[*a..*b] != "%command%")
        .collect();
    for (a, b) in spans.into_iter().rev() {
        if let Some(value) = preload_word(&existing[a..b]) {
            let parts: Vec<_> = value.split([':', ' ']).filter(|v| !v.is_empty()).collect();
            if !parts.iter().any(|p| is_our_library(p)) {
                continue;
            }
            let kept: Vec<_> = parts.into_iter().filter(|p| !is_our_library(p)).collect();
            let replacement = if kept.is_empty() {
                String::new()
            } else {
                format!("LD_PRELOAD={}", shell_words::quote(&kept.join(":")))
            };
            result.replace_range(a..b, &replacement);
        }
    }
    result.trim().to_string()
}

/// Read the current Steam LaunchOptions string configured for VRChat.
pub fn read_steam_launch_options() -> Result<String> {
    steam::read_vrchat_launch_options()
}

/// Updates Steam's `localconfig.vdf` for VRChat with new launch options.
pub fn write_steam_launch_options(new_options: &str) -> Result<()> {
    steam::write_vrchat_launch_options(new_options)
}

/// Shut Steam down cleanly if running, wait for it to exit, then execute modification.
pub async fn set_steam_shield_enabled(enable: bool) -> Result<()> {
    let current = read_steam_launch_options()?;
    shell_words::split(&current).context("Steam launch options have unmatched quotes")?;
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
    let was_running = steam::steam_running();
    if was_running {
        steam::shutdown_steam().await?;
    }

    // Steam may flush a newer localconfig while shutting down. Re-read before editing.
    let write_result = (|| -> Result<()> {
        let current = read_steam_launch_options()?;
        let updated = if enable {
            recommended_launch_options(&current)
        } else {
            remove_shield_from_launch_options(&current)
        };
        write_steam_launch_options(&updated)
    })();
    let restart_result = if was_running {
        steam::start_steam().await
    } else {
        Ok(())
    };
    if let Err(err) = &restart_result {
        tracing::error!("Restarting Steam failed: {err:#}");
    }
    write_result?;
    restart_result
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

#[cfg(test)]
mod preservation_tests {
    use super::*;
    #[test]
    fn keeps_other_preloads_and_quoted_arguments() {
        let input = "LD_PRELOAD=\"/other.so:/x/liblvr_dns_shield.so\" gamemoderun %command% --note \"two words\"";
        let out = remove_shield_from_launch_options(input);
        assert!(out.contains("/other.so"));
        assert!(out.ends_with("--note \"two words\""));
        assert!(!is_shield_in_launch_options(&out));
        let added = recommended_launch_options(&out);
        assert!(is_shield_in_launch_options(&added));
        assert!(added.contains("/other.so:"));
    }
    #[test]
    fn unrelated_mentions_are_not_installations() {
        let input = "%command% --note=liblvr_dns_shield.so";
        assert!(!is_shield_in_launch_options(input));
        assert_eq!(remove_shield_from_launch_options(input), input);
        assert_eq!(
            remove_shield_from_launch_options("LD_PRELOAD=/other.so %command%"),
            "LD_PRELOAD=/other.so %command%"
        );
    }
}
