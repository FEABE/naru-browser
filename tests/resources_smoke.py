#!/usr/bin/env python3
"""Verify resource limits through an already running CEF app and a local fixture.

Run without concurrent CLI tests or tab creation. Requires one to four existing
tabs; none of those tabs are changed. Only this test's task and tabs are closed.
Does not launch the app or create/delete profiles. Screenshot files are new PNGs
under --output-dir and remain available as evidence after cleanup.
"""

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import sys
import threading
import time
import uuid

from smoke import BrowserCli, CheckFailed, ROOT, require


def poll(read, ready, message):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        value = read()
        if ready(value):
            return value
        time.sleep(0.025)
    raise CheckFailed(message)


def file_digest(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(65536), b""):
            digest.update(block)
    return digest.hexdigest()


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {"ok": False, "scope": "real CEF resource limits through local CLI fixtures", "checks": []}
    initial = set()
    owned_tabs = set()
    task_tabs = set()
    task = None
    last_response = None
    server = server_thread = None

    def call(*command, expected=None):
        nonlocal last_response
        last_response = cli.request(*command)
        if expected:
            require(not last_response["ok"] and last_response.get("error", {}).get("code") == expected,
                    f"Expected {expected}, received {json.dumps(last_response, ensure_ascii=False)}")
            return last_response
        require(last_response["ok"], f"CLI request failed: {json.dumps(last_response, ensure_ascii=False)}")
        return last_response["result"]

    def command(*operation, expected=None):
        return call("task", "run", task["id"], task["revision"], *operation, expected=expected)

    def info():
        return call("task", "info", task["id"])["task"]

    def tab_ids():
        return {tab["tab"] for tab in call("tabs")["tabs"]}

    def resources():
        return call("status")["resources"]

    def idle_resources():
        return poll(resources, lambda value: value["pendingTabCreations"] == 0 and value["capturesInFlight"] == 0,
                    "Tab reservations or screenshot captures did not return to zero")

    def remember_tab(result, scoped):
        tab = result["tab"]
        require(isinstance(tab, int) and tab not in initial and tab not in owned_tabs, "Open reused a tab ID")
        owned_tabs.add(tab)
        if scoped:
            task_tabs.add(tab)
        return tab

    def open_tab(url, scoped=False):
        tab = remember_tab(command("open", url) if scoped else call("open", url), scoped)
        if scoped:
            command("wait", tab, "load", 5000)
        else:
            call("wait", tab, "load", 5000)
        return tab

    def reject_open(url, expected, scoped=False):
        nonlocal last_response
        arguments = ("task", "run", task["id"], task["revision"], "open", url) if scoped else ("open", url)
        last_response = cli.request(*arguments)
        # A broken admission limit may create a tab; still record and clean it up.
        if last_response["ok"]:
            remember_tab(last_response["result"], scoped)
        require(not last_response["ok"] and last_response.get("error", {}).get("code") == expected,
                f"Expected {expected}, received {json.dumps(last_response, ensure_ascii=False)}")

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            body = b'''<!doctype html><html><head><title>Resource fixture</title>
<style>body{font:18px system-ui;padding:32px}td,th{border:1px solid #777;padding:12px}</style>
</head><body><h1>Local resource fixture</h1>
<table><tr><th>Resource</th><th>Limit</th></tr>
<tr><td>Automation tabs</td><td>8</td></tr>
<tr><td>Task tabs</td><td>4</td></tr>
<tr><td>Screenshot captures</td><td>1</td></tr></table></body></html>'''
            try:
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                pass

        def log_message(self, *_args):
            pass

    try:
        status = call("status")
        require(status["engine"] == "CEF/Chromium", "Expected a running real CEF app")
        limits = status["resources"]
        require(limits["automationTabLimit"] == 8 and limits["taskTabLimit"] == 4 and limits["captureLimit"] == 1,
                "Expected automation/task/capture limits of 8/4/1")
        initial = tab_ids()
        require(1 <= len(initial) <= 4,
                f"Resource smoke requires 1..4 existing tabs; found {len(initial)}. Existing tabs were not changed.")
        require(limits["pendingTabCreations"] == 0 and limits["capturesInFlight"] == 0,
                "Run this test with no other pending tab creation or screenshot capture")
        require(status["tabs"] == len(initial), "Tab count changed during preflight; run without concurrent tab activity")
        report["initialTabs"] = sorted(initial)
        report["limits"] = {key: limits[key] for key in ("automationTabLimit", "taskTabLimit", "captureLimit")}
        report["checks"].append("running CEF app reports 8/4/1 limits; existing tabs recorded before fixture startup")

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        url = f"http://127.0.0.1:{server.server_port}/fixture"
        task = call("task", "create", "Resource fixture A")["task"]
        require(task["status"] == "active" and task["revision"] == 1 and not task["tabs"],
                "New task must be active, empty, and at revision 1")
        for _ in range(4):
            open_tab(url, scoped=True)
        require({tab["tab"] for tab in info()["tabs"]} == task_tabs, "Task did not retain its four created tabs")
        reject_open(url, "TASK_TAB_LIMIT", scoped=True)
        require(tab_ids() == initial | owned_tabs, "Rejected task open changed the tab set")
        idle_resources()
        report["checks"].append("task accepts four tabs and rejects the fifth without leaking a reservation")

        for _ in range(8 - len(initial) - len(task_tabs)):
            open_tab(url)
        require(len(tab_ids()) == 8, "Browser did not reach eight total tabs")
        reject_open(url, "TAB_LIMIT")
        require(tab_ids() == initial | owned_tabs, "Rejected global open changed the tab set")
        idle_resources()
        report["checks"].append("global admission counts existing tabs and rejects a ninth tab")

        captured_tab = min(task_tabs)
        output_dir = args.output_dir.resolve()
        output_dir.mkdir(parents=True, exist_ok=True)
        screenshot = output_dir / f"resources-{uuid.uuid4().hex}.png"
        require(not screenshot.exists(), "Screenshot evidence path already exists")
        captured = command("screenshot", captured_tab, screenshot)
        require(captured["path"] == str(screenshot), "Screenshot path differs from the requested new path")
        with screenshot.open("rb") as source:
            require(source.read(8) == b"\x89PNG\r\n\x1a\n", "Screenshot does not contain a PNG signature")
        size = screenshot.stat().st_size
        require(size > 100 and captured["bytes"] == size, "Screenshot byte count does not match the PNG file")
        digest = file_digest(screenshot)
        idle_resources()
        command("screenshot", captured_tab, screenshot, expected="INVALID_PATH")
        require(screenshot.stat().st_size == size and file_digest(screenshot) == digest,
                "Rejected screenshot overwrite changed the existing evidence")
        idle_resources()
        report["screenshot"] = {"path": str(screenshot), "bytes": size, "sha256": digest}
        report["checks"].append("new PNG has matching bytes; existing path is rejected unchanged; capture count returns to zero")

        # Use our task tab even when four initial tabs leave no unscoped fixture slot.
        command("close", captured_tab)
        poll(tab_ids, lambda tabs: captured_tab not in tabs, "Closed fixture tab did not receive its close callback")
        require({tab["tab"] for tab in info()["tabs"]} == task_tabs - {captured_tab},
                "Task membership did not follow the close callback")
        task_tabs.remove(captured_tab)
        idle_resources()
        reopened = open_tab(url)
        require(reopened != captured_tab and len(tab_ids()) == 8, "A released slot could not be reused by a new tab")
        idle_resources()
        report["checks"].append("closing a fixture tab releases its slot after callback; a new unscoped open succeeds")

        finished = call("task", "finish", task["id"], task["revision"])
        task = finished["task"]
        require(set(finished["closeRequested"]) == task_tabs, "Task finish requested closing a non-task tab")
        task = poll(info, lambda value: value["status"] == "finished", "Task did not finish after closing its tabs")
        require(not task["tabs"] and not task["closingTabs"] and not task["releasedTabs"], "Finished task retained tabs")
        require(not (tab_ids() & task_tabs), "Finished task tabs remain open")
        report["checks"].append("task finish closes only its remaining created tabs and completes after callbacks")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError) as error:
        report["error"] = str(error)
        report["lastResponse"] = last_response
    finally:
        cleanup = []
        if task is not None:
            try:
                task = info()
                if task["status"] != "finished":
                    task = call("task", "finish", task["id"], task["revision"])["task"]
                    task = poll(info, lambda value: value["status"] == "finished", "Cleanup could not finish its task")
            except (CheckFailed, OSError, KeyError, TypeError) as error:
                cleanup.append(str(error))
        if owned_tabs:
            try:
                remaining = owned_tabs & tab_ids()
            except (CheckFailed, OSError, KeyError, TypeError) as error:
                remaining = set()
                cleanup.append(str(error))
            for tab in sorted(remaining):
                try:
                    require(tab not in initial, "Refusing to close an initial tab")
                    call("close", tab)
                except (CheckFailed, OSError, KeyError, TypeError) as error:
                    cleanup.append(str(error))
            try:
                final = poll(tab_ids, lambda tabs: not (tabs & owned_tabs), "Fixture tabs did not close")
                require(initial <= final, "An initial user tab disappeared")
                report["finalResources"] = idle_resources()
                report["checks"].append("all test-owned tabs closed; initial tabs preserved; reservations and captures are zero")
            except (CheckFailed, OSError, KeyError, TypeError) as error:
                cleanup.append(str(error))
        if cleanup:
            report["ok"] = False
            report["cleanupErrors"] = cleanup
        if server:
            server.shutdown()
            server.server_close()
        if server_thread:
            server_thread.join(timeout=2)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, default=ROOT / "target/release/lightbrowser")
    parser.add_argument("--data-dir", type=Path, default=os.environ.get("LIGHT_BROWSER_DATA_DIR"))
    parser.add_argument("--output-dir", type=Path, default=ROOT / "artifacts/resources",
                        help="Directory for new PNG evidence files; existing files are preserved")
    args = parser.parse_args()
    if args.data_dir is None:
        parser.error("--data-dir or LIGHT_BROWSER_DATA_DIR is required")
    report = run(args)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
