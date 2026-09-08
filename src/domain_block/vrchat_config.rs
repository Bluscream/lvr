use std::collections::{BTreeMap, HashSet};
use anyhow::Result;

/// Official VRChat remote config endpoint.
pub const VRCHAT_CONFIG_URL: &str = "https://api.vrchat.cloud/api/1/config";

/// Embedded copy of VRChat remote config (`https://api.vrchat.cloud/api/1/config`)
/// captured at compile time as a fallback whenever the network is unavailable.
pub const EMBEDDED_FALLBACK_CONFIG: &str =
    include_str!(concat!(env!("OUT_DIR"), "/vrchat_config_fallback.json"));

/// Core protected domains that must never be blocked under any category.
pub const PROTECTED_CORE_DOMAINS: &[&str] = &[
    "vrchat.com",
    "vrchat.cloud",
    "vrchat.net",
    "api.vrchat.cloud",
    "assets.vrchat.com",
    "d348imysud55la.cloudfront.net",
    "files.vrchat.cloud",
    "pipeline.vrchat.cloud",
    "static.vrchat.com",
    "vrchat.com",
    "websocket.vrchat.com",
];

/// Fetches the live official VRChat configuration from its server.
pub async fn fetch_vrchat_remote_config() -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent("lvr/0.1.0")
        .build()?;

    let response = client.get(VRCHAT_CONFIG_URL).send().await?;

    if !response.status().is_success() {
        anyhow::bail!(
            "VRChat config request failed with HTTP status {}",
            response.status()
        );
    }

    Ok(response.text().await?)
}

/// Returns the fallback VRChat configuration embedded at compile-time.
#[allow(dead_code)]
pub fn get_embedded_fallback_config() -> &'static str {
    EMBEDDED_FALLBACK_CONFIG
}

/// Helper to canonicalize a domain name for deduplication / collision checks.
pub fn canonical_domain_key(domain: &str) -> String {
    let s = domain.trim().trim_start_matches("*.");
    let s = s.strip_prefix("www.").unwrap_or(s);
    s.to_lowercase()
}

/// Cleans raw domain or URL entries down to pure hostnames.
pub fn clean_domain(raw: &str) -> String {
    let mut s = raw.trim();
    if let Some(pos) = s.find("://") {
        s = &s[pos + 3..];
    }
    if let Some(pos) = s.find('/') {
        s = &s[..pos];
    }
    if let Some(pos) = s.find(':') {
        s = &s[..pos];
    }
    s.trim_end_matches('.').to_lowercase()
}

fn is_valid_domain(d: &str) -> bool {
    let bare = d.trim_start_matches("*.");
    if bare.is_empty()
        || bare.starts_with('.')
        || bare.ends_with('.')
        || bare.contains("..")
        || !bare.contains('.')
    {
        return false;
    }
    if bare.starts_with("vrc.") || bare.starts_with("unityengine.") || bare.starts_with("system.") {
        return false;
    }
    true
}

fn is_protected(domain: &str, protected_set: &HashSet<String>) -> bool {
    let bare = domain.trim_start_matches("*.");
    if protected_set.contains(bare) || protected_set.contains(domain) {
        return true;
    }
    for core in PROTECTED_CORE_DOMAINS {
        if bare == *core || bare.ends_with(&format!(".{core}")) {
            return true;
        }
    }
    false
}

/// Parses the VRChat configuration JSON string (official or fallback) into categorized domain lists:
/// - Video: from `urlList`
/// - Images: from `imageHostUrlList`
/// - Strings: from `stringHostUrlList`
/// - Rest: Any domain appearing in >1 category
///
/// Automatically expands wildcards `*.domain.tld` into `domain.tld` and `www.domain.tld`.
pub fn parse_vrchat_config_to_categories(json_str: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let root: serde_json::Value = serde_json::from_str(json_str)?;

    // 1. Gather protected assets
    let mut protected: HashSet<String> = PROTECTED_CORE_DOMAINS.iter().map(|s| s.to_string()).collect();
    if let Some(assets) = root.get("whiteListedAssetUrls").and_then(|v| v.as_array()) {
        for item in assets {
            if let Some(s) = item.as_str() {
                let cleaned = clean_domain(s);
                if !cleaned.is_empty() {
                    protected.insert(cleaned.trim_start_matches("*.").to_string());
                    protected.insert(cleaned);
                }
            }
        }
    }

    // 2. Extract domains for the three official categories
    let mappings = [
        ("urlList", "Video"),
        ("imageHostUrlList", "Images"),
        ("stringHostUrlList", "Strings"),
    ];

    let mut domains_by_cat: BTreeMap<&str, HashSet<String>> = BTreeMap::new();
    for (key, cat) in mappings {
        let mut set = HashSet::new();
        if let Some(arr) = root.get(key).and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(s) = item.as_str() {
                    let cleaned = clean_domain(s);
                    if is_valid_domain(&cleaned) && !is_protected(&cleaned, &protected) {
                        set.insert(cleaned);
                    }
                }
            }
        }
        domains_by_cat.insert(cat, set);
    }

    // 3. Expand wildcard domains (*.domain.tld -> domain.tld + www.domain.tld)
    let mut expanded_by_cat: BTreeMap<&str, HashSet<String>> = BTreeMap::new();
    for (cat, d_set) in domains_by_cat {
        let mut expanded = HashSet::new();
        for d in d_set {
            if d.starts_with("*.") {
                let bare = d.trim_start_matches("*.");
                if !bare.is_empty() && bare != "localhost" {
                    expanded.insert(bare.to_string());
                    expanded.insert(format!("www.{bare}"));
                }
            } else {
                expanded.insert(d);
            }
        }
        expanded_by_cat.insert(cat, expanded);
    }

    // 4. Mutual Exclusivity: Move any domain in >= 2 categories to "Rest"
    let mut cat_by_key: BTreeMap<String, HashSet<&str>> = BTreeMap::new();
    for (cat, d_set) in &expanded_by_cat {
        for d in d_set {
            let key = canonical_domain_key(d);
            cat_by_key.entry(key).or_default().insert(*cat);
        }
    }

    let multi_keys: HashSet<String> = cat_by_key
        .into_iter()
        .filter(|(_, cats)| cats.len() >= 2)
        .map(|(k, _)| k)
        .collect();

    let mut result: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut rest_set: HashSet<String> = HashSet::new();

    for (cat, d_set) in expanded_by_cat {
        let mut pure = Vec::new();
        for d in d_set {
            let key = canonical_domain_key(&d);
            if multi_keys.contains(&key) {
                rest_set.insert(d);
            } else {
                pure.push(d);
            }
        }
        pure.sort();
        result.insert(cat.to_string(), pure);
    }

    let mut rest_list: Vec<String> = rest_set.into_iter().collect();
    rest_list.sort();
    result.insert("Rest".to_string(), rest_list);

    Ok(result)
}

/// Returns the fallback domain categories using the embedded VRChat config.
pub fn get_embedded_fallback_categories() -> BTreeMap<String, Vec<String>> {
    parse_vrchat_config_to_categories(EMBEDDED_FALLBACK_CONFIG)
        .expect("embedded fallback VRChat config must be valid JSON")
}
