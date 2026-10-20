#!/usr/bin/env python3
"""e2e-v17 CLI helpers. Refuses to run unless LIGHT_BROWSER_DATA_DIR is under .local-data/e2e-v17-cli
and NARU_MOCK_KEYCHAIN=1. Subcommands print JSON and append to artifacts/e2e-v17/cli/transcript.jsonl.

  step CMD ARG...     run the CLI once, record response
  json-field ...      (not provided; use jq/python on the transcript)
"""
import json, os, subprocess, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = os.environ.get("LIGHT_BROWSER_DATA_DIR", "")
assert DATA == str(ROOT / ".local-data/e2e-v17-cli") and os.environ.get("NARU_MOCK_KEYCHAIN") == "1", "refusing: wrong env"
CLI = str(ROOT / "dist/Light Browser.app/Contents/MacOS/lightbrowser")
OUT = ROOT / "artifacts/e2e-v17/cli"


def cli(*args, label=None, timeout=55):
    t0 = time.time()
    p = subprocess.run([CLI, *map(str, args)], capture_output=True, text=True, timeout=timeout)
    try:
        r = json.loads(p.stdout)
    except ValueError:
        r = {"ok": False, "raw": p.stdout, "stderr": p.stderr}
    with open(OUT / "transcript.jsonl", "a") as f:
        f.write(json.dumps({"label": label, "argv": [str(a) for a in args], "rc": p.returncode,
                            "ms": int((time.time() - t0) * 1000), "resp": r}, ensure_ascii=False) + "\n")
    return r


def code(r):
    return "ok" if r.get("ok") else r.get("error", {}).get("code", "?")
