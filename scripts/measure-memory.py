#!/usr/bin/env python3
"""Read-only macOS memory samples for one caller-selected browser PID and descendants.

The caller must select the correct Naru main PID. No app is launched or stopped.
RSS sums double-count shared mappings and are not unique physical RAM usage.
"""

import argparse
from dataclasses import dataclass
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import time


class MeasurementError(RuntimeError):
    pass


@dataclass(frozen=True)
class Process:
    pid: int
    ppid: int
    rss_kib: int
    started: str
    role: str


def role(command):
    match = re.search(r"(?:^|\s)--type=([^\s]+)", command)
    kind = match.group(1) if match else ""
    if kind in ("renderer", "gpu-process"):
        return {"renderer": "renderer", "gpu-process": "gpu"}[kind]
    if kind == "utility":
        if re.search(r"(?:^|\s)--utility-sub-type=network\.", command):
            return "network"
        if re.search(r"(?:^|\s)--utility-sub-type=storage\.", command):
            return "storage"
        return "utility"
    return "other"


def parse_ps(text):
    processes = {}
    for line in text.splitlines():
        if not line.strip():
            continue
        parts = line.split(None, 8)
        try:
            pid, ppid, rss = map(int, parts[:3])
            if len(parts) != 9 or pid < 0 or ppid < 0 or rss < 0 or pid in processes:
                raise ValueError
        except (ValueError, TypeError):
            raise MeasurementError("invalid_ps_output") from None
        processes[pid] = Process(pid, ppid, rss, " ".join(parts[3:8]), role(parts[8]))
    return processes


def read_processes():
    try:
        result = subprocess.run(
            ["/bin/ps", "-ww", "-axo", "pid=,ppid=,rss=,lstart=,args="],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, errors="replace",
            timeout=5, check=False, env=dict(os.environ, LC_ALL="C"),
        )
    except (OSError, subprocess.TimeoutExpired):
        raise MeasurementError("ps_unavailable") from None
    if result.returncode:
        raise MeasurementError("ps_unavailable")
    return parse_ps(result.stdout)


def descendants(processes, root):
    children = {}
    for process in processes.values():
        children.setdefault(process.ppid, []).append(process.pid)
    selected, pending = set(), [root]
    while pending:
        pid = pending.pop()
        if pid in selected or pid not in processes:
            continue
        selected.add(pid)
        pending.extend(children.get(pid, []))
    return [processes[pid] for pid in sorted(selected)]


def check_root(processes, pid, started=None):
    if pid not in processes:
        raise MeasurementError("main_process_exited")
    current = processes[pid].started
    if started is not None and current != started:
        raise MeasurementError("main_pid_reused")
    return current


def parse_footprint(text):
    match = re.search(r"^Physical footprint:\s+([\d.]+)([KMG])\s*$", text, re.MULTILINE)
    if not match:
        raise MeasurementError("footprint_unavailable")
    try:
        value = float(match.group(1)) * {"K": 1 / 1024, "M": 1, "G": 1024}[match.group(2)]
    except ValueError:
        raise MeasurementError("footprint_unavailable") from None
    if not math.isfinite(value):
        raise MeasurementError("footprint_unavailable")
    return round(value, 3)


def read_footprint(pid, timeout):
    try:
        result = subprocess.run(
            ["/usr/bin/vmmap", "-summary", str(pid)], stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, errors="replace", timeout=timeout, check=False,
            env=dict(os.environ, LC_ALL="C"),
        )
    except subprocess.TimeoutExpired:
        raise MeasurementError("footprint_timeout") from None
    except OSError:
        raise MeasurementError("footprint_unavailable") from None
    if result.returncode:
        raise MeasurementError("footprint_unavailable")
    if result.stderr.strip():
        raise MeasurementError("footprint_warning")
    return parse_footprint(result.stdout)


def collect(pid, samples, interval, footprint=False, reader=read_processes,
            sleeper=time.sleep, clock=time.monotonic, footprint_reader=read_footprint):
    report = {"ok": True, "rootPid": pid, "source": "macOS ps RSS (KiB)", "samples": [],
              "rssNote": "Shared mappings can be counted repeatedly; sums are not unique RAM usage."}
    began, identity = clock(), None
    for index in range(samples):
        if index:
            sleeper(interval)
        processes = reader()
        identity = check_root(processes, pid, identity)
        members = descendants(processes, pid)
        rows = [{"pid": p.pid, "ppid": p.ppid, "role": "browser" if p.pid == pid else p.role,
                 "rssMiB": round(p.rss_kib / 1024, 3)} for p in members]
        report["samples"].append({"elapsedSeconds": round(clock() - began, 3), "processes": rows,
                                  "rssSumMiB": round(sum(p.rss_kib for p in members) / 1024, 3)})
    report["peakObservedRssSumMiB"] = max(sample["rssSumMiB"] for sample in report["samples"])
    if footprint:
        deadline = clock() + 30
        for process, row in zip(members, rows):
            row["physicalFootprintMiB"] = None
            try:
                remaining = deadline - clock()
                if remaining <= 0:
                    raise MeasurementError("footprint_budget_exhausted")
                before = reader()
                check_root(before, pid, identity)
                if not any(p.pid == process.pid and p.started == process.started for p in descendants(before, pid)):
                    raise MeasurementError("footprint_process_changed")
                remaining = deadline - clock()
                if remaining <= 0:
                    raise MeasurementError("footprint_budget_exhausted")
                value = footprint_reader(process.pid, min(8, remaining))
                after = reader()
                check_root(after, pid, identity)
                if not any(p.pid == process.pid and p.started == process.started for p in descendants(after, pid)):
                    raise MeasurementError("footprint_process_changed")
                row["physicalFootprintMiB"] = value
            except MeasurementError as error:
                if str(error) in ("main_process_exited", "main_pid_reused"):
                    raise
                row["footprintWarning"] = str(error)
        check_root(reader(), pid, identity)
        values = [row["physicalFootprintMiB"] for row in rows]
        report["footprintSumMiB"] = round(sum(values), 3) if all(v is not None for v in values) else None
        report["footprintNote"] = "Final-sample processes measured sequentially, not at one instant; no per-process peaks are summed."
    return report


def bounded(kind, minimum, maximum):
    def parse(value):
        try:
            result = kind(value)
            if not math.isfinite(result) or not minimum <= result <= maximum:
                raise ValueError
            return result
        except (ValueError, OverflowError):
            raise argparse.ArgumentTypeError(f"Use a value from {minimum} to {maximum}.") from None
    return parse


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", required=True, type=bounded(int, 1, 2147483647))
    parser.add_argument("--samples", default=3, type=bounded(int, 1, 10))
    parser.add_argument("--interval", default=1.0, type=bounded(float, 0.1, 5))
    parser.add_argument("--footprint", action="store_true", help="Also sample vmmap (8s per PID, 30s sampling window plus bounded identity checks).")
    parser.add_argument("--output", type=Path, help="Write to a new file; never overwrite an existing file.")
    args = parser.parse_args()
    try:
        if sys.platform != "darwin":
            raise MeasurementError("macOS_required")
        if args.output is not None and os.path.lexists(args.output):
            raise MeasurementError("output_already_exists")
        report = collect(args.pid, args.samples, args.interval, args.footprint)
        text = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
        if args.output is not None:
            with os.fdopen(os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
                output.write(text)
        print(text, end="")
        return 0
    except (MeasurementError, OSError) as error:
        print(json.dumps({"ok": False, "error": str(error) if isinstance(error, MeasurementError) else "output_write_failed"}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
