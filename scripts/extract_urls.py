#!/usr/bin/env python3
"""Extract URLs from explicitly selected local SQLite databases and VRChat logs."""
import argparse
import csv
import glob
import re
import sqlite3
from contextlib import closing
from pathlib import Path

QUERIES = (
    ("SELECT video_url FROM gamelog_video_play WHERE video_url IS NOT NULL", "video"),
    ("SELECT message FROM events WHERE type = 'video_url' AND message IS NOT NULL", "video"),
    ("SELECT resource_url, resource_type FROM gamelog_resource_load WHERE resource_url IS NOT NULL", "resource"),
)
PATTERNS = {
    "video": [
        re.compile(r"\[Video Playback\] (?:Attempting to resolve URL|URL) '([^']+)'"),
        re.compile(r"\[AVProVideo\] Opening (\S+) \(offset"),
        re.compile(r"Unsupported URL:\s*(https?://\S+)", re.I),
    ],
    "image": [re.compile(r"\[Image Download\] Attempting to load image from URL '([^']+)'")],
    "string": [re.compile(r"\[String Download\] Attempting to load String from URL '([^']+)'")],
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--database", action="append", default=[], type=Path)
    parser.add_argument("--log", action="append", default=[], help="Log filename or glob")
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parent.parent / ".references/urls")
    args = parser.parse_args()
    if not args.database and not args.log:
        print("No local sources supplied; existing URL lists were left unchanged.")
        return
    urls = {category: set() for category in PATTERNS}

    def add(category, value):
        if isinstance(value, str) and value.strip().startswith(("http://", "https://", "rtsp://", "rtmp://")):
            urls[category].add(value.strip())

    for database in args.database:
        # URI read-only mode neither creates a missing database nor takes a write lock.
        with closing(sqlite3.connect(database.resolve().as_uri() + "?mode=ro", uri=True)) as connection:
            for query, category in QUERIES:
                try:
                    for row in connection.execute(query):
                        if category == "resource":
                            kind = {"ImageLoad": "image", "StringLoad": "string"}.get(row[1])
                            if kind:
                                add(kind, row[0])
                        else:
                            add(category, row[0])
                except sqlite3.OperationalError as error:
                    if "no such table" not in str(error):
                        raise
                    print(f"Skipping unavailable table in {database.name}: {error}")
    for pattern in args.log:
        matches = sorted(glob.glob(pattern))
        if not matches:
            raise FileNotFoundError(f"No logs matched {pattern}")
        for filename in matches:
            with open(filename, encoding="utf-8", errors="replace") as source:
                for line in source:
                    for category, patterns in PATTERNS.items():
                        for regex in patterns:
                            if match := regex.search(line):
                                add(category, match.group(1))
    args.output.mkdir(parents=True, exist_ok=True)
    for category, values in urls.items():
        path = args.output / f"{category}.csv"
        temporary = path.with_suffix(".csv.tmp")
        with temporary.open("w", newline="", encoding="utf-8") as output:
            writer = csv.writer(output)
            writer.writerow(["url"])
            writer.writerows([value] for value in sorted(values))
        temporary.replace(path)
        print(f"Wrote {len(values)} {category} URLs to {path}")


if __name__ == "__main__":
    main()
