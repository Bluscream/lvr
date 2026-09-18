#!/usr/bin/env python3
"""Timing and resource accounting for the build/test flow, with regression detection.

Every phase of `scripts/build.sh` runs through `metrics.py run`, which records
wall time, CPU time and peak RSS. `metrics.py report` then diffs the whole run
against the previous one so a slowdown, a memory blow-up or a binary-size jump
is visible immediately instead of being noticed months later.

    metrics.py begin                     start a run (clears the scratch file)
    metrics.py run LABEL -- CMD...       time CMD, record it, propagate its exit
    metrics.py record KEY=VALUE ...      record a bare number
    metrics.py size PATH                 record total and .text size of a binary
    metrics.py probe --binary PATH       measure the built binary's idle cost
    metrics.py report                    diff against the last run and store it

Every build phase is labelled `cold` (it compiled something) or `warm` (fully
cached). Timing and memory are only compared between runs in the same state --
a warm phase is seconds faster than a cold one no matter what the code does, and
reporting that as a 90% improvement is worse than reporting nothing.

State lives in `.metrics/`: `current.json` for the run in progress, `last.json`
for the previous completed run, `history.jsonl` for everything before that. It
is machine-specific and gitignored -- comparisons are only meaningful against
your own previous run on your own hardware.
"""

from __future__ import annotations

import json
import os
import resource
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
METRICS_DIR = ROOT / ".metrics"
CURRENT = METRICS_DIR / "current.json"
LAST = METRICS_DIR / "last.json"
HISTORY = METRICS_DIR / "history.jsonl"

# Only flag differences that are both relatively and absolutely meaningful, so
# ordinary machine noise does not cry wolf on every run.
RELATIVE_THRESHOLD = 0.15
ABSOLUTE_FLOOR = {
    "wall_s": 1.0,
    "cpu_s": 1.0,
    "max_rss_mb": 32.0,
    "binary_size_mb": 0.2,
    "text_size_mb": 0.1,
    "idle_cpu_pct": 0.2,
    "idle_cpu_gui_pct": 0.15,
    "idle_cpu_supervisor_pct": 0.15,
    "idle_cpu_tokio_pct": 0.15,
    "idle_child_cpu_pct": 0.15,
    "idle_rss_mb": 8.0,
    "idle_reads_per_s": 200.0,
    "idle_writes_per_s": 20.0,
    "idle_read_mb_per_s": 0.1,
    "idle_ctx_switches_per_s": 15.0,
    "threads": 2.0,
    "open_fds": 40.0,
    "startup_ready_s": 0.3,
}
# Metrics where a larger number is better. Everything here is a cost, so none.
LOWER_IS_BETTER = True

UNITS = {
    "wall_s": "s",
    "cpu_s": "s",
    "max_rss_mb": "MB",
    "binary_size_mb": "MB",
    "text_size_mb": "MB",
    "idle_cpu_pct": "%",
    "idle_cpu_gui_pct": "%",
    "idle_cpu_supervisor_pct": "%",
    "idle_cpu_tokio_pct": "%",
    "idle_child_cpu_pct": "%",
    "idle_rss_mb": "MB",
    "idle_reads_per_s": "/s",
    "idle_writes_per_s": "/s",
    "idle_read_mb_per_s": "MB/s",
    "idle_ctx_switches_per_s": "/s",
    "startup_ready_s": "s",
}


def load(path: Path) -> dict:
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return {}


def save(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n")


def git_describe() -> str:
    try:
        out = subprocess.run(
            ["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        sha = out.stdout.strip() or "unknown"
    except (OSError, subprocess.SubprocessError):
        return "unknown"
    try:
        dirty = subprocess.run(
            ["git", "-C", str(ROOT), "status", "--porcelain"],
            capture_output=True,
            text=True,
            timeout=10,
        ).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        dirty = ""
    return sha + ("-dirty" if dirty else "")


def cmd_begin() -> int:
    save(CURRENT, {"started": time.time(), "git": git_describe(), "metrics": {}})
    return 0


def current_run() -> dict:
    run = load(CURRENT)
    if not run:
        run = {"started": time.time(), "git": git_describe(), "metrics": {}}
    run.setdefault("metrics", {})
    run.setdefault("cache", {})
    return run


# Flat directories cargo writes compiled units into. Scanning these is cheap,
# unlike walking all of target/, and is enough to tell whether a phase actually
# compiled anything.
DEPS_DIRS = ("target/debug/deps", "target/release/deps")


def compiled_since(start: float) -> bool:
    """Did cargo write any compilation output after `start` (a time.time())?"""
    for relative in DEPS_DIRS:
        directory = ROOT / relative
        if not directory.is_dir():
            continue
        try:
            for entry in os.scandir(directory):
                try:
                    if entry.stat(follow_symlinks=False).st_mtime > start:
                        return True
                except OSError:
                    continue
        except OSError:
            continue
    return False


def cmd_run(label: str, argv: list[str]) -> int:
    """Run argv to completion, recording what it cost. Output is never captured.

    Build output goes straight to the terminal: warnings and notes must stay
    visible, so this deliberately does not intercept or filter any of it.
    """
    if not argv:
        print("metrics: nothing to run", file=sys.stderr)
        return 2
    wall_start = time.time()
    started = time.monotonic()
    proc = subprocess.Popen(argv)
    _, status, usage = os.wait4(proc.pid, 0)
    elapsed = time.monotonic() - started

    run = current_run()
    run["metrics"][label] = {
        "wall_s": round(elapsed, 2),
        "cpu_s": round(usage.ru_utime + usage.ru_stime, 2),
        # ru_maxrss is kilobytes on Linux.
        "max_rss_mb": round(usage.ru_maxrss / 1024, 1),
    }
    # Timings only mean anything between phases that did comparable work. A
    # fully cached cargo phase is seconds faster than one that compiled, which
    # otherwise reads as a spectacular improvement.
    run["cache"][label] = "cold" if compiled_since(wall_start) else "warm"
    save(CURRENT, run)

    if os.WIFSIGNALED(status):
        return 128 + os.WTERMSIG(status)
    return os.WEXITSTATUS(status)


def cmd_record(pairs: list[str]) -> int:
    run = current_run()
    for pair in pairs:
        if "=" not in pair:
            print(f"metrics: expected KEY=VALUE, got {pair!r}", file=sys.stderr)
            return 2
        key, _, raw = pair.partition("=")
        try:
            value = float(raw)
        except ValueError:
            print(f"metrics: {key} is not a number: {raw!r}", file=sys.stderr)
            return 2
        group, _, metric = key.partition(".")
        if not metric:
            group, metric = "artifacts", group
        run["metrics"].setdefault(group, {})[metric] = round(value, 3)
    save(CURRENT, run)
    return 0


def cmd_size(binary: str) -> int:
    """Record the binary's total size and, separately, its executable code.

    Total size moves for uninteresting reasons -- debug data, embedded assets.
    `.text` is the code itself, so the two moving apart says which one grew.
    """
    path = Path(binary)
    if not path.is_file():
        print(f"metrics: no binary at {path}, skipping size", file=sys.stderr)
        return 0
    values = [f"artifacts.binary_size_mb={path.stat().st_size / 1048576:.3f}"]
    try:
        # Section headers: name, type, address, offset, size (hex), ...
        out = subprocess.run(
            ["readelf", "-S", "-W", str(path)],
            capture_output=True,
            text=True,
            timeout=60,
        )
        for line in out.stdout.splitlines():
            parts = line.replace("[", " ").replace("]", " ").split()
            if len(parts) > 6 and parts[1] == ".text":
                values.append(f"artifacts.text_size_mb={int(parts[5], 16) / 1048576:.3f}")
                break
    except (OSError, subprocess.SubprocessError, ValueError):
        pass  # readelf is optional; total size alone is still useful.
    return cmd_record(values)


PROBE_CONFIG = """\
# Generated by scripts/metrics.py for an idle-cost measurement. Disposable.
#
# autostart MUST stay empty and MUST stay above every table header: Config is
# #[serde(default)], so omitting it substitutes the bundled example entries and
# this throwaway instance would start and stop the real companion apps.
autostart = []

[general]
poll_interval_ms = {poll_interval_ms}
start_hidden = true
close_to_tray = true
vrchat_match = ["vrchat.exe"]
log_capacity = 1000

[wivrn]
watchdog = false
start_command = ""

[audio]
enabled = false

[virtual_display]
create_on_startup = false
create_on_last_display_unplugged = false

[domain_block]
prefix = "{prefix}"
load_community_blocklists = false
community_sources = []

[domain_block.blocked]
video_blocked = false
image_blocked = false
string_blocked = false
shared_blocked = false
custom_blocked = {{}}
"""

# Sockets the app needs to reach from inside its private runtime directory.
PASSTHROUGH_SOCKETS = ("wayland-0", "wayland-0.lock", "pipewire-0", "bus")


# Thread names are truncated to 15 characters by the kernel, so match on
# prefixes. Everything the app does lands in one of these buckets, and keeping
# them apart is what makes a regression legible: an aggregate number cannot say
# whether the GUI started repainting or the supervisor started polling harder.
THREAD_BUCKETS = (
    ("lvr-supervisor", "supervisor"),
    ("lvr-ipc", "other"),
    ("lvr:", "other"),
    ("tokio-rt", "tokio"),
    ("lvr", "gui"),
)


def thread_bucket(comm: str) -> str:
    for prefix, bucket in THREAD_BUCKETS:
        if comm.startswith(prefix):
            return bucket
    return "other"


def sample_process(pid: int) -> dict:
    """One reading of everything the probe tracks, straight from /proc."""
    stat = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    sample = {
        # Fields 14-17 (1-based, after comm): utime, stime, cutime, cstime.
        "ticks": int(stat[11]) + int(stat[12]),
        # Children's CPU is counted only once they are reaped, which is exactly
        # what a fork-per-tick regression looks like: cheap for us, expensive
        # for the machine. This is the metric that makes such a cost visible.
        "child_ticks": int(stat[13]) + int(stat[14]),
        "syscr": 0,
        "syscw": 0,
        "rchar": 0,
        "ctx": 0,
        "threads": 0,
        "by_bucket": {},
    }
    for line in Path(f"/proc/{pid}/io").read_text().splitlines():
        key, _, value = line.partition(":")
        if key in ("syscr", "syscw", "rchar"):
            sample[key] = int(value)
    sample["rss_kb"] = int(Path(f"/proc/{pid}/statm").read_text().split()[1]) * (
        os.sysconf("SC_PAGE_SIZE") // 1024
    )
    try:
        sample["fds"] = len(os.listdir(f"/proc/{pid}/fd"))
    except OSError:
        sample["fds"] = 0

    tasks = os.listdir(f"/proc/{pid}/task")
    sample["threads"] = len(tasks)
    for tid in tasks:
        try:
            raw = Path(f"/proc/{pid}/task/{tid}/stat").read_text()
            comm = raw.split("(", 1)[1].rsplit(")", 1)[0]
            fields = raw.rsplit(")", 1)[1].split()
            bucket = thread_bucket(comm)
            ticks = int(fields[11]) + int(fields[12])
            sample["by_bucket"][bucket] = sample["by_bucket"].get(bucket, 0) + ticks
            for line in Path(f"/proc/{pid}/task/{tid}/status").read_text().splitlines():
                if line.startswith(("voluntary_ctxt_switches", "nonvoluntary_ctxt")):
                    sample["ctx"] += int(line.split(":")[1])
        except (OSError, IndexError, ValueError):
            continue
    return sample


def cmd_probe(binary: str, seconds: float, poll_interval_ms: int) -> int:
    """Launch the built binary hidden, in isolation, and measure what it costs idle.

    This is the only check that catches a runtime CPU regression -- the kind
    where the build gets no slower but the app burns a core in the tray. The
    instance is fully isolated: its own runtime directory (so it cannot touch
    the real single-instance socket), and a config with no autostart entries and
    no domain blocking, so it manages nothing on the live system.
    """
    binary_path = Path(binary)
    if not binary_path.is_file():
        print(f"metrics: no binary at {binary_path}, skipping probe", file=sys.stderr)
        return 0
    host_runtime = os.environ.get("XDG_RUNTIME_DIR")
    if not host_runtime or not Path(host_runtime, "wayland-0").exists():
        print("metrics: no Wayland session, skipping idle probe", file=sys.stderr)
        return 0

    # Keep the directory short: the IPC socket path must fit in sockaddr_un.
    runtime = Path(tempfile.mkdtemp(prefix="lvrm", dir="/tmp"))
    workdir = Path(tempfile.mkdtemp(prefix="lvrmcfg"))
    proc = None
    try:
        for name in PASSTHROUGH_SOCKETS:
            source = Path(host_runtime, name)
            if source.exists():
                os.symlink(source, runtime / name)
        prefix = workdir / "prefix"
        (prefix / "pfx/drive_c/windows/system32/drivers/etc").mkdir(parents=True)
        config = workdir / "config.toml"
        config.write_text(
            PROBE_CONFIG.format(prefix=prefix, poll_interval_ms=poll_interval_ms)
        )

        env = dict(os.environ)
        env["XDG_RUNTIME_DIR"] = str(runtime)
        env["LVR_CONFIG"] = str(config)
        env["LVR_LOG"] = "warn"
        proc = subprocess.Popen(
            [str(binary_path), "--hidden"],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )

        # Time to usable: the IPC socket is bound once the instance is live.
        socket_path = runtime / "lvr.sock"
        launched = time.monotonic()
        ready = None
        while time.monotonic() - launched < 30:
            if socket_path.exists():
                ready = time.monotonic() - launched
                break
            if proc.poll() is not None:
                break
            time.sleep(0.05)

        time.sleep(15.0)
        if proc.poll() is not None:
            print("metrics: probe instance exited early, skipping", file=sys.stderr)
            return 0

        start = sample_process(proc.pid)
        time.sleep(seconds)
        end = sample_process(proc.pid)

        hz = os.sysconf("SC_CLK_TCK")

        def rate(key: str) -> float:
            return (end[key] - start[key]) / seconds

        def pct(ticks: float) -> float:
            return ticks / hz / seconds * 100

        values = [
            f"idle.idle_cpu_pct={pct(end['ticks'] - start['ticks']):.3f}",
            # Split by thread so a regression names its own culprit.
            f"idle.idle_cpu_gui_pct={pct(end['by_bucket'].get('gui', 0) - start['by_bucket'].get('gui', 0)):.3f}",
            f"idle.idle_cpu_supervisor_pct={pct(end['by_bucket'].get('supervisor', 0) - start['by_bucket'].get('supervisor', 0)):.3f}",
            f"idle.idle_cpu_tokio_pct={pct(end['by_bucket'].get('tokio', 0) - start['by_bucket'].get('tokio', 0)):.3f}",
            # Subprocesses we fork and reap: a per-tick `kscreen-doctor` shows
            # up here and nowhere else, because the cost is not ours.
            f"idle.idle_child_cpu_pct={pct(end['child_ticks'] - start['child_ticks']):.3f}",
            f"idle.idle_rss_mb={end['rss_kb'] / 1024:.1f}",
            f"idle.idle_reads_per_s={rate('syscr'):.0f}",
            f"idle.idle_writes_per_s={rate('syscw'):.1f}",
            f"idle.idle_read_mb_per_s={rate('rchar') / 1048576:.3f}",
            # High switches with low CPU means blocking; high both means a spin.
            f"idle.idle_ctx_switches_per_s={rate('ctx'):.0f}",
            f"idle.threads={end['threads']}",
            f"idle.open_fds={end['fds']}",
        ]
        if ready is not None:
            values.append(f"idle.startup_ready_s={ready:.2f}")
        return cmd_record(values)
    except (OSError, ValueError) as err:
        print(f"metrics: idle probe failed ({err}), skipping", file=sys.stderr)
        return 0
    finally:
        if proc is not None and proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
        shutil.rmtree(runtime, ignore_errors=True)
        shutil.rmtree(workdir, ignore_errors=True)


# Metrics that depend on how much the phase actually had to compile.
CACHE_SENSITIVE = {"wall_s", "cpu_s", "max_rss_mb"}


def classify(metric: str, old: float, new: float) -> str:
    delta = new - old
    floor = ABSOLUTE_FLOOR.get(metric, 0.0)
    if abs(delta) < floor:
        return "same"
    if old > 0 and abs(delta) / old < RELATIVE_THRESHOLD:
        return "same"
    worse = delta > 0 if LOWER_IS_BETTER else delta < 0
    return "worse" if worse else "better"


def cmd_report(fail_on_regression: bool) -> int:
    run = load(CURRENT)
    if not run or not run.get("metrics"):
        print("metrics: nothing recorded for this run", file=sys.stderr)
        return 0
    run["finished"] = time.time()
    previous = load(LAST)
    prev_metrics = previous.get("metrics", {})

    print()
    print("=== build metrics ===")
    if previous:
        when = time.strftime("%Y-%m-%d %H:%M", time.localtime(previous.get("finished", 0)))
        print(f"comparing {run.get('git', '?')} against {previous.get('git', '?')} ({when})")
    else:
        print(f"{run.get('git', '?')} -- first run, nothing to compare against yet")

    regressions = []
    skipped = 0
    cache_now = run.get("cache", {})
    cache_then = previous.get("cache", {})
    width = max((len(g) for g in run["metrics"]), default=8)
    name_width = max(
        (len(m) for group in run["metrics"].values() for m in group), default=18
    )
    for group in sorted(run["metrics"]):
        state_now = cache_now.get(group)
        state_then = cache_then.get(group)
        # "cold" means the phase compiled something, "warm" means it was fully
        # cached. Shown on every line so a fast run is never mistaken for a win.
        tag = f" [{state_now}]" if state_now else ""
        for metric in sorted(run["metrics"][group]):
            new = run["metrics"][group][metric]
            unit = UNITS.get(metric, "")
            old = prev_metrics.get(group, {}).get(metric)
            line = f"  {group:<{width}}  {metric:<{name_width}} {new:>9.2f}{unit:<4}{tag}"
            if old is None:
                print(f"{line}   (new)")
                continue
            was = f"  was {old:>9.2f}{unit:<4}"
            if state_then:
                was += f" [{state_then}]"
            mismatch = (
                metric in CACHE_SENSITIVE
                and state_now is not None
                and state_then is not None
                and state_now != state_then
            )
            if mismatch:
                # A cold phase against a warm one says nothing about the code.
                print(f"{line}{was}   not comparable ({state_then} -> {state_now})")
                skipped += 1
                continue
            verdict = classify(metric, old, new)
            delta = new - old
            pct = f"{delta / old * 100:+.0f}%" if old else "n/a"
            mark = {"same": "", "worse": "  REGRESSION", "better": "  improved"}[verdict]
            print(f"{line}{was}  {pct:>6}{mark}")
            if verdict == "worse":
                regressions.append(f"{group}.{metric} {old:.2f} -> {new:.2f}{unit}")

    if skipped:
        print()
        print(
            f"{skipped} timing/memory comparison(s) skipped: the build cache state "
            "differed between runs. For comparable build timings, run both with a "
            "warm cache, or `cargo clean` before each."
        )
    if regressions:
        print()
        print(f"{len(regressions)} regression(s) past the {RELATIVE_THRESHOLD:.0%} threshold:")
        for item in regressions:
            print(f"  - {item}")
    print()

    METRICS_DIR.mkdir(parents=True, exist_ok=True)
    with HISTORY.open("a") as handle:
        handle.write(json.dumps(run, sort_keys=True) + "\n")
    save(LAST, run)
    CURRENT.unlink(missing_ok=True)

    return 1 if (regressions and fail_on_regression) else 0


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__)
        return 2
    command, rest = argv[0], argv[1:]
    if command == "begin":
        return cmd_begin()
    if command == "run":
        if "--" not in rest:
            print("metrics: usage: metrics.py run LABEL -- CMD...", file=sys.stderr)
            return 2
        split = rest.index("--")
        labels = rest[:split]
        if len(labels) != 1:
            print("metrics: exactly one LABEL is required", file=sys.stderr)
            return 2
        return cmd_run(labels[0], rest[split + 1 :])
    if command == "record":
        return cmd_record(rest)
    if command == "size":
        if len(rest) != 1:
            print("metrics: usage: metrics.py size PATH", file=sys.stderr)
            return 2
        return cmd_size(rest[0])
    if command == "probe":
        binary = ROOT / "target/release/lvr"
        seconds, poll = 20.0, 2000
        index = 0
        while index < len(rest):
            if rest[index] == "--binary" and index + 1 < len(rest):
                binary = Path(rest[index + 1])
                index += 2
            elif rest[index] == "--seconds" and index + 1 < len(rest):
                seconds = float(rest[index + 1])
                index += 2
            elif rest[index] == "--poll-interval-ms" and index + 1 < len(rest):
                poll = int(rest[index + 1])
                index += 2
            else:
                print(f"metrics: unknown probe option {rest[index]!r}", file=sys.stderr)
                return 2
        return cmd_probe(str(binary), seconds, poll)
    if command == "report":
        return cmd_report("--fail-on-regression" in rest)
    print(f"metrics: unknown command {command!r}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
