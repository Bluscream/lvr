#!/usr/bin/env python3
"""Build the public categorized domain bundle from official and community inputs.

Local URL extraction/conversion must be invoked explicitly; publishing never scans
personal history. Protected domains are filtered and shared families are separated.
"""

import ipaddress
import json
import urllib.request
from collections import defaultdict
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parent.parent
LISTS_DIR = PROJECT_ROOT / "assets" / "lists"
COMMUNITY_JSON_PATH = LISTS_DIR / "community.json"
FALLBACK_CONFIG_PATH = PROJECT_ROOT / "assets" / "vrchat_config_fallback.json"
SCRIPTS_DIR = PROJECT_ROOT / "scripts"

VRC_CONFIG_URL = "https://api.vrchat.cloud/api/1/config"
USER_AGENT = "lvr/0.1.0"

PROTECTED_CORE_DOMAINS = {
    "assets.vrchat.com",
    "vrchat.com",
    "vrchat.cloud",
    "vrchat.net",
    "d348imysud55la.cloudfront.net",
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
    # Never allow IP addresses (IPv4 or IPv6) as blockable hostnames
    bare = cleaned.lstrip("*.")
    try:
        ipaddress.ip_address(bare)
        return False
    except ValueError:
        pass
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
        raise RuntimeError("Neither live nor bundled VRChat configuration is available")


def is_protected(domain: str, protected_set: set[str]) -> bool:
    bare = domain.lstrip("*.")
    if bare in protected_set or f"*.{bare}" in protected_set or domain in protected_set:
        return True
    for core in protected_set:
        if bare == core or bare.endswith(f".{core}") or (domain.startswith("*.") and core.endswith(f".{bare}")):
            return True
    return False


def main():
    # Local history extraction is explicitly invoked by the user, never by publishing CI.
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
        ("urlList", "Videos"),
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
        if key in ("urlList", "Videos"):
            cat = "Videos"
        elif key in ("imageHostUrlList", "Images"):
            cat = "Images"
        elif key in ("stringHostUrlList", "Strings"):
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

    # 6. Preserve wildcards as first-class entries (do NOT force-expand to www.)
    expanded_domains_by_cat = defaultdict(set)
    for cat, d_set in domains_by_cat.items():
        for d in d_set:
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

    # 7. Output assets/lists/domains.json containing all blocklists in a single flat JSON:
    # {
    #   "Analytics": [...],
    #   "Video": [...]
    # }
    domains_json_path = LISTS_DIR / "domains.json"
    domains_json_data = {
        cat: sorted(set(final_categories[cat]))
        for cat in sorted(final_categories.keys())
    }

    temporary = domains_json_path.with_suffix(".json.tmp")
    with open(temporary, "w", encoding="utf-8") as f:
        json.dump(domains_json_data, f, indent=2)
        f.write("\n")
    temporary.replace(domains_json_path)

    print(f"\nGenerated unified domains bundle at: {domains_json_path} ({len(domains_json_data)} categories)")
    for cat, d_list in sorted(domains_json_data.items()):
        print(f"  - {cat}: {len(d_list)} domains")


if __name__ == "__main__":
    main()
