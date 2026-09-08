#!/usr/bin/env python3
"""
Build and compile pre-filtered, mutually exclusive hosts files on the build box / CI.

Pipeline:
1. Fetches official VRChat remote config from https://api.vrchat.cloud/api/1/config
   (falling back to assets/vrchat_config_fallback.json if network is unavailable).
2. Optionally invokes scripts/extract_urls.py if local databases or logs are present.
3. Invokes scripts/convert_urls.py to process raw URLs into .references/domains and update community.json.
4. Reads assets/lists/community.json.
5. Ingests all domains across Official and Community categories:
   - "urlList" -> Video
   - "imageHostUrlList" -> Images
   - "stringHostUrlList" -> Strings
   - Any custom community keys (e.g. "Analytics") -> Custom category name
6. Filters out protected core VRChat domains and whiteListedAssetUrls.
7. Enforces mutual exclusivity: any domain that appears in >= 2 categories is removed from
   those categories and assigned exclusively to the "Shared" category.
8. Writes out clean, individual category hosts files into assets/lists/hosts/:
   - Video.hosts
   - Images.hosts
   - Strings.hosts
   - Analytics.hosts (and any other custom categories)
   - Shared.hosts
   Each host entry includes its www.* equivalent and a trailing comment `# <sources>`.
"""

import csv
import json
import os
import re
import subprocess
import sys
import urllib.parse
import urllib.request
from collections import defaultdict
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parent.parent
LISTS_DIR = PROJECT_ROOT / "assets" / "lists"
HOSTS_OUT_DIR = LISTS_DIR / "hosts"
COMMUNITY_JSON_PATH = LISTS_DIR / "community.json"
FALLBACK_CONFIG_PATH = PROJECT_ROOT / "assets" / "vrchat_config_fallback.json"
SCRIPTS_DIR = PROJECT_ROOT / "scripts"

VRC_CONFIG_URL = "https://api.vrchat.cloud/api/1/config"
USER_AGENT = "lvr/0.1.0"

PROTECTED_CORE_DOMAINS = {
    "assets.vrchat.com",
    "vrchat.com",
    "vrchat.cloud",
    "dbinj8iahsbec.cloudfront.net",
}


def clean_domain(raw: str) -> str:
    s = raw.strip()
    if "://" in s:
        s = s.split("://", 1)[1]
    if "/" in s:
        s = s.split("/", 1)[0]
    if ":" in s:
        s = s.split(":", 1)[0]
    return s.strip().lower()


def canonical_domain_key(domain: str) -> str:
    cleaned = clean_domain(domain)
    bare = cleaned.lstrip("*.")
    if bare.startswith("www.") and "." in bare[4:]:
        return bare[4:]
    return bare


def is_valid_domain_or_glob(entry: str) -> bool:
    cleaned = clean_domain(entry)
    if not cleaned or cleaned == "localhost":
        return False
    # Check IP
    parts = cleaned.split(".")
    if len(parts) == 4 and all(p.isdigit() and 0 <= int(p) <= 255 for p in parts):
        return True

    bare = cleaned.lstrip("*.")
    if not bare or bare.startswith(".") or bare.endswith(".") or ".." in bare or "." not in bare:
        return False

    # Disallow C# code patterns and PascalCase
    if any(bare.startswith(p) for p in ("vrc.", "unityengine.", "system.")) or ".components." in bare or ".sdk" in bare:
        return False

    labels = bare.split(".")
    if len(labels) < 2:
        return False

    for l in labels:
        if not l or l.startswith("-") or l.endswith("-"):
            return False
        if not all(c.isalnum() or c == "-" or c == "_" for c in l):
            return False

    tld = labels[-1]
    if tld.startswith("xn--"):
        return len(tld) >= 6 and all(c.isalnum() or c == "-" for c in tld)
    return 2 <= len(tld) <= 18 and tld.isalpha()


def fetch_vrchat_config() -> dict:
    print(f"Fetching VRChat remote config from {VRC_CONFIG_URL}...")
    try:
        req = urllib.request.Request(VRC_CONFIG_URL, headers={"User-Agent": USER_AGENT})
        with urllib.request.urlopen(req, timeout=10) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            print("Successfully fetched live VRChat remote config.")
            return data
    except Exception as e:
        print(f"[WARN] Failed to fetch live VRChat config ({e}); using fallback config at {FALLBACK_CONFIG_PATH}")
        if FALLBACK_CONFIG_PATH.exists():
            with open(FALLBACK_CONFIG_PATH, "r", encoding="utf-8") as f:
                return json.load(f)
        return {}


def is_protected(domain: str, protected_set: set[str]) -> bool:
    bare = domain.lstrip("*.")
    if bare in protected_set or f"*.{bare}" in protected_set or domain in protected_set:
        return True
    for core in PROTECTED_CORE_DOMAINS:
        if bare == core or bare.endswith(f".{core}"):
            return True
    return False


def run_helper_scripts():
    extract_py = SCRIPTS_DIR / "extract_urls.py"
    convert_py = SCRIPTS_DIR / "convert_urls.py"

    # Only run extract_urls if extract script exists and there are local DBs / logs to scan
    if extract_py.exists():
        try:
            print("Running extract_urls.py...")
            subprocess.run([sys.executable, str(extract_py)], check=True, cwd=str(PROJECT_ROOT))
        except Exception as e:
            print(f"[WARN] extract_urls.py step skipped or encountered error: {e}")

    if convert_py.exists():
        try:
            print("Running convert_urls.py...")
            subprocess.run([sys.executable, str(convert_py)], check=True, cwd=str(PROJECT_ROOT))
        except Exception as e:
            print(f"[WARN] convert_urls.py step encountered error: {e}")


def main():
    # 1. Run local extraction & conversion scripts if available
    run_helper_scripts()

    # 2. Fetch official config
    vrc_config = fetch_vrchat_config()

    # 3. Read community config
    community_config = {}
    if COMMUNITY_JSON_PATH.exists():
        print(f"Reading community config from {COMMUNITY_JSON_PATH}...")
        with open(COMMUNITY_JSON_PATH, "r", encoding="utf-8") as f:
            community_config = json.load(f)

    # 4. Gather protected assets from official and community lists
    protected = set(PROTECTED_CORE_DOMAINS)
    for cfg in (vrc_config, community_config):
        for item in cfg.get("whiteListedAssetUrls", []):
            if isinstance(item, str):
                cleaned = clean_domain(item)
                if cleaned:
                    protected.add(cleaned)
                    protected.add(cleaned.lstrip("*."))

    # 5. Ingest domain entries and track sources
    # Map: category -> set of domain strings
    domains_by_cat = defaultdict(set)
    # Map: canonical_key -> set of list sources (e.g. "Official", "Community")
    domain_sources = defaultdict(set)

    # Ingest Official: ONLY urlList, imageHostUrlList, stringHostUrlList
    official_mappings = [
        ("urlList", "Video"),
        ("imageHostUrlList", "Images"),
        ("stringHostUrlList", "Strings"),
    ]
    for key, cat in official_mappings:
        for item in vrc_config.get(key, []):
            if isinstance(item, str) and is_valid_domain_or_glob(item):
                cleaned = clean_domain(item)
                if cleaned and not is_protected(cleaned, protected):
                    domains_by_cat[cat].add(cleaned)
                    domain_sources[canonical_domain_key(cleaned)].add("Official")

    # Ingest Community
    for key, val in community_config.items():
        if key.startswith("$") or not isinstance(val, list):
            continue
        if key in ("urlList", "Videos", "video", "videos"):
            cat = "Video"
        elif key in ("imageHostUrlList", "Images", "image", "images"):
            cat = "Images"
        elif key in ("stringHostUrlList", "Strings", "string", "strings"):
            cat = "Strings"
        elif key in ("whiteListedAssetUrls",):
            continue
        else:
            cat = key.strip()

        for item in val:
            if isinstance(item, str) and is_valid_domain_or_glob(item):
                cleaned = clean_domain(item)
                if cleaned and not is_protected(cleaned, protected):
                    domains_by_cat[cat].add(cleaned)
                    domain_sources[canonical_domain_key(cleaned)].add("Community")

    # 6. Expand wildcard domains (*.domain.tld -> domain.tld + www.domain.tld)
    expanded_domains_by_cat = defaultdict(set)
    for cat, d_set in domains_by_cat.items():
        for d in d_set:
            if d.startswith("*."):
                bare = d.lstrip("*.")
                if bare and bare != "localhost":
                    expanded_domains_by_cat[cat].add(bare)
                    expanded_domains_by_cat[cat].add(f"www.{bare}")
                    # Inherit sources
                    srcs = domain_sources.get(canonical_domain_key(d), set())
                    domain_sources[canonical_domain_key(bare)].update(srcs)
                    domain_sources[canonical_domain_key(f"www.{bare}")].update(srcs)
            else:
                expanded_domains_by_cat[cat].add(d)

    # 7. Enforce Category Mutual Exclusivity
    # Any canonical key present in >= 2 categories moves to "Shared"
    cat_by_key = defaultdict(set)
    for cat, d_set in expanded_domains_by_cat.items():
        for d in d_set:
            cat_by_key[canonical_domain_key(d)].add(cat)

    multi_keys = {k for k, cats in cat_by_key.items() if len(cats) >= 2}

    final_categories = defaultdict(list)
    shared_set = set()

    for cat, d_set in expanded_domains_by_cat.items():
        for d in d_set:
            ckey = canonical_domain_key(d)
            if ckey in multi_keys:
                shared_set.add(d)
            else:
                final_categories[cat].append(d)

    final_categories["Shared"] = list(shared_set)

    # 7. Write out category .hosts files to assets/lists/hosts/
    HOSTS_OUT_DIR.mkdir(parents=True, exist_ok=True)

    summary = {}
    for cat, d_list in sorted(final_categories.items()):
        file_path = HOSTS_OUT_DIR / f"{cat}.hosts"
        emitted_hosts = set()
        lines = [
            f"# ==============================================================================",
            f"# LVR Blocklist: {cat} Category",
            f"# Total base domains: {len(d_list)}",
            f"# ==============================================================================\n",
        ]

        # Sort domains
        sorted_domains = sorted(set(d_list))
        for d in sorted_domains:
            # If the entry starts with *. (e.g. *.facebook.com), strip *. and write normal domain (facebook.com).
            # Only include www. if the source entry actually had www. in front (e.g. www.facebook.com).
            bare = d.lstrip("*.")
            if not bare or bare == "localhost":
                continue
            # Skip IP addresses in hosts file
            parts = bare.split(".")
            if len(parts) == 4 and all(p.isdigit() and 0 <= int(p) <= 255 for p in parts):
                continue

            sources = sorted(domain_sources.get(canonical_domain_key(bare), ["Community"]))
            comment = f"  # {', '.join(sources)}" if sources else ""

            host_lower = bare.lower()
            if host_lower not in emitted_hosts:
                emitted_hosts.add(host_lower)
                lines.append(f"0.0.0.0 {host_lower}{comment}")

        with open(file_path, "w", encoding="utf-8") as f:
            f.write("\n".join(lines))
            f.write("\n")

        summary[cat] = len(sorted_domains)
        print(f"Generated {file_path} ({len(sorted_domains)} base domains, {len(emitted_hosts)} hosts)")

    # 8. Output assets/lists/domains.json containing all blocklists in a single flat JSON:
    # {
    #   "Analytics": [...],
    #   "Video": [...]
    # }
    domains_json_path = LISTS_DIR / "domains.json"
    domains_json_data = {
        cat: sorted(set(final_categories[cat]))
        for cat in sorted(final_categories.keys())
    }

    with open(domains_json_path, "w", encoding="utf-8") as f:
        json.dump(domains_json_data, f, indent=2)
        f.write("\n")

    print(f"\nGenerated unified domains bundle at: {domains_json_path} ({len(domains_json_data)} categories)")

    print("\nPre-computed hosts files generated successfully:")
    for cat, count in summary.items():
        print(f"  - {cat}.hosts: {count} domains")


if __name__ == "__main__":
    main()
