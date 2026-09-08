import csv
import glob
import os
import re
import sqlite3

def main():
    out_dir = "/run/media/system/Data/Projects/lvr/.references/urls"
    os.makedirs(out_dir, exist_ok=True)

    video_urls = set()
    image_urls = set()
    string_urls = set()

    # 1. Databases to scan
    dbs = [
        '/home/blu/.config/VRCX-0/VRCX-0.sqlite3',
        '/home/blu/.config/VRCNext/VRCNData.db',
        '/run/media/system/Data/OneDrive/Games/VRChat/_TOOLS/VRCX/AppData/VRCX/VRCX.sqlite3',
        '/run/media/system/Data/OneDrive/Games/VRChat/_TOOLS/VRCX/AppData/VRCX/VRCX - Copy.sqlite3',
        '/run/media/system/Data/OneDrive/Games/VRChat/_TOOLS/VRCX/AppData/VRCX/VRCX - Copy (2).sqlite3',
        '/run/media/system/Data/OneDrive/Games/VRChat/_TOOLS/VRCX/AppData/VRCX/VRCX - Copy (3).sqlite3',
        '/run/media/system/Data/OneDrive/Games/VRChat/_TOOLS/VRCX/AppData/VRCX/VRCX-Bluscream-PC - Copy.sqlite3',
        '/run/media/system/Data/OneDrive/Games/VRChat/_TOOLS/VRCX/AppData/VRCX/VRCX-Bluscream-PC-2.sqlite3',
        '/run/media/system/Data/OneDrive/Games/VRChat/_TOOLS/VRCX/AppData/VRCX/VRCX-Bluscream-PC-3.sqlite3',
        '/run/media/system/Data/OneDrive/Games/VRChat/_TOOLS/VRCX/AppData/VRCX-0/VRCX-0.sqlite3'
    ]

    for db_path in dbs:
        if not os.path.exists(db_path):
            continue
        print(f"Reading DB: {db_path}")
        try:
            conn = sqlite3.connect(db_path)
            c = conn.cursor()

            # VRCX video table
            try:
                for row in c.execute("SELECT video_url FROM gamelog_video_play WHERE video_url IS NOT NULL AND video_url != ''"):
                    u = row[0].strip()
                    if u.startswith(('http://', 'https://', 'rtsp://', 'rtmp://')):
                        video_urls.add(u)
            except Exception:
                pass

            # VRCNext events table for video_url
            try:
                for row in c.execute("SELECT message FROM events WHERE type = 'video_url' AND message IS NOT NULL AND message != ''"):
                    u = row[0].strip()
                    if u.startswith(('http://', 'https://', 'rtsp://', 'rtmp://')):
                        video_urls.add(u)
            except Exception:
                pass

            # VRCX resource table (Udon string & image loads)
            try:
                for row in c.execute("SELECT resource_url, resource_type FROM gamelog_resource_load WHERE resource_url IS NOT NULL AND resource_url != ''"):
                    u, rtype = row[0].strip(), (row[1] or '').strip()
                    if not u.startswith(('http://', 'https://')):
                        continue
                    if rtype == 'ImageLoad':
                        image_urls.add(u)
                    elif rtype == 'StringLoad':
                        string_urls.add(u)
            except Exception:
                pass

            conn.close()
        except Exception as e:
            print(f"Error reading DB {db_path}: {e}")

    # 2. VRChat logs to scan
    log_globs = [
        '/run/media/system/Data/Games/Steam/steamapps/compatdata/438100/pfx/drive_c/users/steamuser/AppData/LocalLow/VRChat/VRChat/output_log*.txt',
        '/run/media/system/Data/Users/Bluscream/AppData/LocalLow/VRChat/vrchat/logs/output_log*.txt',
        '/run/media/system/Data/Users/Bluscream/AppData/LocalLow/VRChat/vrchat/output_log*.txt'
    ]

    log_files = []
    for g in log_globs:
        log_files.extend(glob.glob(g))

    print(f"Scanning {len(log_files)} log files...")

    video_patterns = [
        re.compile(r'\[Video Playback\] Attempting to resolve URL \'([^\']+)\''),
        re.compile(r'\[Video Playback\] URL \'([^\']+)\' resolved to'),
        re.compile(r'\[AVProVideo\] Opening ([^\s]+) \(offset'),
        re.compile(r'User .+ (?:played|loaded|started) video:?\s*(https?://[^\s\'"]+)', re.I),
        re.compile(r'Could not send HEAD request to (https?://[^\s:]+)', re.I),
        re.compile(r'Unsupported URL:\s*(https?://[^\s]+)', re.I),
    ]

    str_pattern = re.compile(r'\[String Download\] Attempting to load String from URL \'([^\']+)\'')
    img_pattern = re.compile(r'\[Image Download\] Attempting to load image from URL \'([^\']+)\'')

    for log_file in log_files:
        try:
            with open(log_file, 'r', encoding='utf-8', errors='ignore') as f:
                for line in f:
                    for vp in video_patterns:
                        m = vp.search(line)
                        if m:
                            u = m.group(1).strip()
                            if u.startswith(('http://', 'https://')):
                                video_urls.add(u)
                    ms = str_pattern.search(line)
                    if ms:
                        u = ms.group(1).strip()
                        if u.startswith(('http://', 'https://')):
                            string_urls.add(u)
                    mi = img_pattern.search(line)
                    if mi:
                        u = mi.group(1).strip()
                        if u.startswith(('http://', 'https://')):
                            image_urls.add(u)
        except Exception as e:
            print(f"Error reading log {log_file}: {e}")

    # 3. Write output CSVs
    def write_csv(filepath, url_set):
        sorted_urls = sorted(url_set)
        with open(filepath, 'w', newline='', encoding='utf-8') as f:
            writer = csv.writer(f)
            writer.writerow(['url'])
            for u in sorted_urls:
                writer.writerow([u])
        print(f"Wrote {len(sorted_urls)} unique URLs to {filepath}")

    write_csv(os.path.join(out_dir, "video.csv"), video_urls)
    write_csv(os.path.join(out_dir, "image.csv"), image_urls)
    write_csv(os.path.join(out_dir, "string.csv"), string_urls)

if __name__ == '__main__':
    main()
