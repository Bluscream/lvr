#!/usr/bin/env python3
import csv
import json
import os
import re
import urllib.parse
from pathlib import Path

# Base paths
PROJECT_ROOT = Path(__file__).resolve().parent.parent
URLS_DIR = PROJECT_ROOT / ".references" / "urls"
REFERENCES_DOMAINS_DIR = PROJECT_ROOT / ".references" / "domains"
LISTS_DIR = PROJECT_ROOT / "assets" / "lists"
CONFIG_JSON_PATH = LISTS_DIR / "community.json"

# Blacklist of internal VRChat domains that shouldn't be included
BLACKLIST_DOMAINS = {
    "assets.vrchat.com",
    "vrchat.com",
    "vrchat.cloud",
    "dbinj8iahsbec.cloudfront.net"
}

IP_REGEX = re.compile(r"^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}$")


def is_blacklisted(domain: str) -> bool:
    d = domain.lower().strip()
    for b in BLACKLIST_DOMAINS:
        if d == b or d.endswith("." + b):
            return True
    return False


def extract_domain(raw_url: str) -> str | None:
    if not raw_url:
        return None
    raw_url = raw_url.strip()
    try:
        parsed = urllib.parse.urlparse(raw_url)
        netloc = parsed.netloc.strip().lower()
        if not netloc:
            return None
        # Remove port if present (e.g. 1.2.3.4:8080 or example.com:8443)
        if ":" in netloc:
            netloc = netloc.split(":")[0]
        netloc = netloc.rstrip(".")
        if not netloc:
            return None
        # Never treat IP addresses as blockable domains
        if IP_REGEX.match(netloc):
            return None
        return netloc
    except Exception:
        return None


def read_urls_from_csv(csv_path: Path) -> list[str]:
    urls = []
    if not csv_path.exists():
        print(f"[WARN] File not found: {csv_path}")
        return urls

    with open(csv_path, "r", encoding="utf-8", errors="ignore") as f:
        reader = csv.reader(f)
        header = next(reader, None)
        for row in reader:
            if row and row[0]:
                urls.append(row[0].strip())
    return urls


def write_reference_domain_csv(csv_path: Path, domains: list[str]) -> None:
    csv_path.parent.mkdir(parents=True, exist_ok=True)
    with open(csv_path, "w", newline="", encoding="utf-8") as f:
        writer = csv.writer(f)
        writer.writerow(["domain"])
        for d in domains:
            writer.writerow([d])


def update_config_json(data: dict[str, list[str]], config_path: Path) -> None:
    config_path.parent.mkdir(parents=True, exist_ok=True)

    config = {}
    if config_path.exists():
        try:
            with open(config_path, "r", encoding="utf-8") as f:
                config = json.load(f)
        except Exception:
            config = {}

    # urlList represents video streaming domains
    config["imageHostUrlList"] = data["image"]
    config["stringHostUrlList"] = data["string"]
    config["urlList"] = data["video"]

    with open(config_path, "w", encoding="utf-8") as f:
        json.dump(config, f, indent=2)
        f.write("\n")

    print(f"Updated config JSON at: {config_path} (urlList: {len(data['video'])}, image: {len(data['image'])}, string: {len(data['string'])})")


def main():
    print(f"Reading extracted URLs from: {URLS_DIR}")

    domain_data = {}
    categories = ["video", "image", "string"]

    for cat in categories:
        csv_file = URLS_DIR / f"{cat}.csv"
        urls = read_urls_from_csv(csv_file)

        domains = set()
        for u in urls:
            d = extract_domain(u)
            if d and not is_blacklisted(d):
                domains.add(d)

        sorted_domains = sorted(domains)
        domain_data[cat] = sorted_domains
        print(f"[{cat}] Processed {len(urls)} URLs -> {len(sorted_domains)} unique domains (blacklisted removed)")

        # Update .references/domains/<cat>.csv
        ref_csv = REFERENCES_DOMAINS_DIR / f"{cat}.csv"
        write_reference_domain_csv(ref_csv, sorted_domains)

    # Output assets/lists/community.json
    update_config_json(domain_data, CONFIG_JSON_PATH)


if __name__ == "__main__":
    main()
