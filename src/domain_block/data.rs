//! Loading, validating, and caching domain lists.
use super::*;

/// Primary file path where downloaded `domains.json` is cached locally.
pub fn domains_cache_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "lvr")
        .map(|dirs| dirs.cache_dir().join("domains.json"))
        .unwrap_or_else(|| PathBuf::from("assets/lists/domains.json"))
}

pub fn is_file_fresh(path: &Path, max_age: std::time::Duration) -> bool {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .map(|elapsed| elapsed < max_age)
        .unwrap_or(false)
}

/// Fetches the pre-compiled `domains.json` from GitHub.
pub async fn fetch_domains_json_remote() -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent("lvr/0.1.0")
        .build()?;

    let response = client.get(DOMAINS_JSON_URL).send().await?;
    if !response.status().is_success() {
        anyhow::bail!("Failed to fetch domains.json: HTTP {}", response.status());
    }
    Ok(response.text().await?)
}

/// Filters out any direct IP addresses (IPv4 or IPv6) from domain lists.
pub fn sanitize_domain_map(raw: BTreeMap<String, Vec<String>>) -> BTreeMap<String, Vec<String>> {
    let protected = vrchat_config::PROTECTED_CORE_DOMAINS
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut result = BTreeMap::<String, Vec<String>>::new();
    let mut owners = BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    for (cat, domains) in raw {
        if cat.is_empty() || cat.trim() != cat || cat.chars().any(char::is_control) {
            continue;
        }
        let mut domains: Vec<_> = domains
            .into_iter()
            .map(|d| vrchat_config::clean_domain(&d))
            .filter(|d| {
                vrchat_config::is_valid_domain(d) && !vrchat_config::is_protected(d, &protected)
            })
            .collect();
        domains.sort();
        domains.dedup();
        for d in &domains {
            owners
                .entry(vrchat_config::canonical_domain_key(d))
                .or_default()
                .insert(cat.clone());
        }
        result.insert(cat, domains);
    }
    let mut shared = result.remove("Shared").unwrap_or_default();
    for domains in result.values_mut() {
        domains.retain(|d| {
            if owners[&vrchat_config::canonical_domain_key(d)].len() > 1 {
                shared.push(d.clone());
                false
            } else {
                true
            }
        });
    }
    shared.sort();
    shared.dedup();
    result.insert("Shared".into(), shared);
    result
}

fn parse_bundle(body: &str) -> Result<DomainMap> {
    let map = sanitize_domain_map(serde_json::from_str(body)?);
    anyhow::ensure!(
        map.values().any(|v| !v.is_empty()),
        "domain bundle contains no usable rules"
    );
    Ok(map)
}

/// Loads domains into memory with priority:
/// 1. Fresh local cached `domains.json` (< 1 hour old)
/// 2. Remote GitHub fetch of `domains.json`
/// 3. Bundled categorized domain list (retains community rules offline)
pub async fn load_domains(force: bool) -> BTreeMap<String, Vec<String>> {
    let cache_file = domains_cache_path();

    // 1. Fresh local cache of domains.json
    if !force
        && is_file_fresh(&cache_file, DOMAINS_CACHE_MAX_AGE)
        && let Ok(content) = fs::read_to_string(&cache_file)
        && let Ok(parsed) = parse_bundle(&content)
    {
        tracing::debug!(
            "Loaded domains from fresh local cache at {}",
            cache_file.display()
        );
        return sanitize_domain_map(parsed);
    }

    // 2. Fetch pre-compiled domains.json from GitHub
    match fetch_domains_json_remote().await {
        Ok(body) => {
            if let Ok(parsed) = parse_bundle(&body) {
                if let Err(err) = crate::files::atomic_write(&cache_file, body.as_bytes()) {
                    tracing::warn!("Caching domain list failed: {err:#}");
                }
                tracing::info!("Loaded domains.json from GitHub");
                return sanitize_domain_map(parsed);
            }
        }
        Err(err) => {
            tracing::warn!(
                "Failed to fetch remote domains.json ({err:#}); falling back to bundled lists"
            );
        }
    }

    tracing::warn!("Using bundled domain lists because the remote bundle is unavailable");
    sanitize_domain_map(
        serde_json::from_str(include_str!("../../assets/lists/domains.json"))
            .expect("bundled domain lists must be valid"),
    )
}

/// Returns currently active domain lists in memory.
pub fn active_domains() -> DomainLists {
    if let Ok(guard) = ACTIVE_DOMAIN_MAP.read()
        && let Some(arc) = guard.as_ref()
    {
        return DomainLists {
            domains: (**arc).clone(),
        };
    }

    let default = Arc::new(vrchat_config::get_embedded_fallback_categories());
    if let Ok(mut guard) = ACTIVE_DOMAIN_MAP.write() {
        let active = guard.get_or_insert_with(|| default.clone()).clone();
        return DomainLists {
            domains: (*active).clone(),
        };
    }
    DomainLists {
        domains: (*default).clone(),
    }
}

async fn load_configured_domains(cfg: &crate::config::DomainBlockConfig, force: bool) -> DomainMap {
    let use_bundle = cfg.load_community_blocklists
        && cfg
            .community_sources
            .iter()
            .any(|s| s.enabled && s.url == crate::config::DEFAULT_COMMUNITY_CONFIG_URL);
    let mut domains = if use_bundle {
        load_domains(force).await
    } else {
        match vrchat_config::fetch_vrchat_remote_config()
            .await
            .and_then(|body| vrchat_config::parse_vrchat_config_to_categories(&body))
        {
            Ok(domains) => domains,
            Err(err) => {
                tracing::warn!("Official domain list unavailable: {err:#}");
                vrchat_config::get_embedded_fallback_categories()
            }
        }
    };
    if cfg.load_community_blocklists {
        for source in cfg
            .community_sources
            .iter()
            .filter(|s| s.enabled && s.url != crate::config::DEFAULT_COMMUNITY_CONFIG_URL)
        {
            let fetched = async {
                let body = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(5))
                    .build()?
                    .get(&source.url)
                    .send()
                    .await?
                    .error_for_status()?
                    .text()
                    .await?;
                let value: serde_json::Value = serde_json::from_str(&body)?;
                let mut extra = vrchat_config::parse_vrchat_config_to_categories(&body)?;
                if let Some(object) = value.as_object() {
                    for (name, values) in object {
                        if name.starts_with('$')
                            || matches!(
                                name.as_str(),
                                "urlList"
                                    | "imageHostUrlList"
                                    | "stringHostUrlList"
                                    | "whiteListedAssetUrls"
                            )
                        {
                            continue;
                        }
                        if let Ok(entries) = serde_json::from_value::<Vec<String>>(values.clone()) {
                            extra.entry(name.clone()).or_default().extend(entries);
                        }
                    }
                }
                let protected: std::collections::HashSet<String> = value
                    .get("whiteListedAssetUrls")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str())
                    .map(vrchat_config::clean_domain)
                    .collect();
                for entries in extra.values_mut() {
                    entries.retain(|domain| {
                        !vrchat_config::is_protected(
                            &vrchat_config::clean_domain(domain),
                            &protected,
                        )
                    });
                }
                Ok::<_, anyhow::Error>(extra)
            }
            .await;
            match fetched {
                Ok(extra) => {
                    for (cat, values) in extra {
                        domains.entry(cat).or_default().extend(values);
                    }
                }
                Err(err) => tracing::warn!("Community list {} failed: {err:#}", source.name),
            }
        }
    }
    sanitize_domain_map(domains)
}

pub async fn init_from_remote_or_fallback_with_config(cfg: &crate::config::DomainBlockConfig) {
    let loaded = load_configured_domains(cfg, false).await;
    *ACTIVE_DOMAIN_MAP.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(loaded));
}

pub async fn reload_domain_lists_with_config(
    cfg: &crate::config::DomainBlockConfig,
) -> DomainLists {
    let loaded = load_configured_domains(cfg, true).await;
    *ACTIVE_DOMAIN_MAP.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(loaded));
    active_domains()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_or_invalid_remote_bundle_is_not_accepted() {
        assert!(parse_bundle("{}").is_err());
        assert!(parse_bundle(r#"{"Videos":["127.0.0.1","api.vrchat.cloud"]}"#).is_err());
        assert!(parse_bundle(r#"{"Videos":["example.com"]}"#).is_ok());
    }
}
