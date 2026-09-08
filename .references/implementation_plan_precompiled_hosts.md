# Client Architecture Simplification: Pre-compiled GitHub Hosts Files

## Overview

Currently, the client binary (`lvr`) performs multi-source JSON fetching, dynamic JSON parsing, wildcard expansion, mutual exclusivity partitioning, and comment construction on the local machine at runtime.

This document outlines the implementation plan for simplifying the client to fetch pre-compiled, pre-filtered hosts files directly from GitHub (e.g. via GitHub Contents API or Raw URLs) and assemble the hosts file using simple block concatenation and basic deduplication.

---

## 1. Background & Motivation

- **Build box / CI Pipeline**: [`scripts/build_hosts_files.py`](file:///run/media/system/Data/Projects/lvr/scripts/build_hosts_files.py) already executes on CI (GitHub Actions) or locally:
  - Fetches the official VRChat remote config.
  - Extracts and converts logged community URLs.
  - Ingests `community.json`.
  - Filters core VRChat protected domains.
  - Partitions multi-category collisions strictly into `Rest`.
  - Generates category hosts files in `assets/lists/hosts/`:
    - `Video.hosts`
    - `Images.hosts`
    - `Strings.hosts`
    - `Analytics.hosts` (and any new custom categories)
    - `Rest.hosts`
- **Client Benefits**:
  - Eliminates heavy JSON parsing, schema validation, and collision sorting on the client.
  - No need to maintain fallback VRChat JSON config copies in Rust binaries.
  - Adding new community categories or sources becomes automatic without changing Rust code: any new `<Category>.hosts` file in the directory is automatically discovered.
  - Client binary becomes significantly leaner and faster to start and update.

---

## 2. GitHub API Discovery & Download Flow

### A. Discovering Available Categories via GitHub API
The client queries the GitHub repository contents API:
```http
GET https://api.github.com/repos/Bluscream/lvr/contents/assets/lists/hosts
User-Agent: lvr/<version>
Accept: application/vnd.github.v3+json
```

The response is a JSON array of file objects:
```json
[
  {
    "name": "Analytics.hosts",
    "download_url": "https://raw.githubusercontent.com/Bluscream/lvr/main/assets/lists/hosts/Analytics.hosts"
  },
  {
    "name": "Images.hosts",
    "download_url": "https://raw.githubusercontent.com/Bluscream/lvr/main/assets/lists/hosts/Images.hosts"
  },
  {
    "name": "Rest.hosts",
    "download_url": "https://raw.githubusercontent.com/Bluscream/lvr/main/assets/lists/hosts/Rest.hosts"
  },
  {
    "name": "Strings.hosts",
    "download_url": "https://raw.githubusercontent.com/Bluscream/lvr/main/assets/lists/hosts/Strings.hosts"
  },
  {
    "name": "Video.hosts",
    "download_url": "https://raw.githubusercontent.com/Bluscream/lvr/main/assets/lists/hosts/Video.hosts"
  }
]
```

- Any file ending in `.hosts` is treated as a category:
  `Category = filename.strip_suffix(".hosts")`
- This dynamically identifies all categories: `Video`, `Images`, `Strings`, `Rest`, plus any custom category like `Analytics`.

### B. Caching Strategy
- Store downloaded category hosts files in the local cache directory:
  `$XDG_CACHE_HOME/lvr/hosts/<Category>.hosts` (e.g. `~/.cache/lvr/hosts/Video.hosts`).
- Cache max age: 1 hour (same as current `COMMUNITY_CACHE_MAX_AGE`).
- If GitHub API is unreachable, fall back to:
  1. Cached `.hosts` files in `$XDG_CACHE_HOME/lvr/hosts/`.
  2. Packaged/offline `.hosts` files embedded or shipped in `assets/lists/hosts/`.

---

## 3. Simplified Client Hosts File Assembly

When the user enables or disables categories in the GUI or CLI:

```rust
pub fn apply_hosts_blocks(
    prefix: &Path,
    active_categories: &[String], // e.g. ["Video", "Images", "Analytics", "Rest"]
) -> Result<()> {
    let hosts_path = prefix_hosts_path(prefix);
    let existing = fs::read_to_string(&hosts_path).unwrap_or_default();

    // 1. Strip existing LVR blocks
    let base_lines = strip_existing_lvr_blocks(&existing);
    let mut result = base_lines.join("\n");
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }

    let mut emitted_hosts = HashSet::new();

    // 2. Track non-LVR existing hosts
    for line in &base_lines {
        let without_comment = line.split('#').next().unwrap_or("").trim();
        if without_comment.starts_with("0.0.0.0 ") || without_comment.starts_with("127.0.0.1 ") {
            for part in without_comment.split_whitespace().skip(1) {
                emitted_hosts.insert(part.to_lowercase());
            }
        }
    }

    // 3. For each active category, read the pre-compiled <Category>.hosts file
    for cat in active_categories {
        let content = load_category_hosts_content(cat)?; // memory / cache / embedded
        let header = format!("# ----- BEGIN LVR {} BLOCK -----", cat.to_uppercase());
        let footer = format!("# ----- END LVR {} BLOCK -----", cat.to_uppercase());

        let mut block_lines = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            // Parse hostname: "0.0.0.0 hostname  # Source"
            let without_comment = trimmed.split('#').next().unwrap_or("").trim();
            if let Some(host) = without_comment.strip_prefix("0.0.0.0 ") {
                let host_lower = host.trim().to_lowercase();
                // Simple deduplication check
                if emitted_hosts.insert(host_lower) {
                    block_lines.push(trimmed);
                }
            }
        }

        if !block_lines.is_empty() {
            result.push_str(&header);
            result.push('\n');
            result.push_str(&format!("# Blocked by LinuxVR (lvr): {}\n", cat));
            for bline in block_lines {
                result.push_str(bline);
                result.push('\n');
            }
            result.push_str(&footer);
            result.push('\n');
        }
    }

    // Atomic write
    let tmp = hosts_path.with_extension("lvr_tmp");
    fs::write(&tmp, result)?;
    fs::rename(tmp, hosts_path)?;
    Ok(())
}
```

---

## 4. Summary of Planned Client Edits

| Component | Current Implementation | Simplified New Implementation |
|---|---|---|
| **VRChat Config Fetching** | Client downloads `api.vrchat.cloud/api/1/config` & falls back to compile-time JSON | Client does not query VRChat servers at all. Build box does this and commits `.hosts` |
| **Category Parsing & Partitioning** | Complex Rust algorithm in `parser.rs` computing exclusivity & canonical keys | Removed from runtime. Done on build box / CI in Python |
| **Category List** | Fixed enum `BlockCategory` with `Custom(String)` | Dynamic string categories discovered from `.hosts` filenames |
| **Network Payload** | Multi-KB complex JSON configurations | Pre-formatted plaintext `.hosts` files |
| **Hosts Writing** | Synthesizes 0.0.0.0 and comments from scratch | Direct stream copy of `.hosts` entries with simple `HashSet` dedupe |
