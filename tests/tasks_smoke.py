#!/usr/bin/env python3
"""Verify task ownership through an already running app and localhost fixtures.

Does not launch the app or create/delete profiles. Only this test's tasks and
tabs are changed. Keep the app's initial tab open throughout the test. Run
without other concurrent CLI tests so pendingWaits identifies this test's wait.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import sys
import threading
import time
from urllib.parse import urlsplit

from smoke import BrowserCli, CheckFailed, ROOT, element, require


def poll(read, ready, message, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = read()
        if ready(value):
            return value
        time.sleep(0.025)
    raise CheckFailed(message)


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {"ok": False, "scope": "real CEF task spaces through local CLI fixtures", "checks": []}
    initial = set()
    owned_tabs = set()
    tasks = {}
    last_response = None
    requested, release = threading.Event(), threading.Event()
    server = server_thread = executor = None

    def call(*command, expected=None):
        nonlocal last_response
        last_response = cli.request(*command)
        if expected:
            require(not last_response["ok"] and last_response.get("error", {}).get("code") == expected,
                    f"Expected {expected}, received {json.dumps(last_response, ensure_ascii=False)}")
            return last_response
        require(last_response["ok"], f"CLI request failed: {json.dumps(last_response, ensure_ascii=False)}")
        return last_response["result"]

    def manage(action, task, *extra):
        before = task["revision"]
        result = call("task", action, task["id"], before, *extra)
        current = result["task"]
        expected = before + (action in ("handoff", "resume", "finish"))
        require(current["revision"] == expected, f"Unexpected {action} revision")
        tasks[current["id"]] = current
        return current, result

    def command(task, *operation, expected=None):
        return call("task", "run", task["id"], task["revision"], *operation, expected=expected)

    def info(task):
        return call("task", "info", task["id"])["task"]

    def tab_ids():
        return {tab["tab"] for tab in call("tabs")["tabs"]}

    def open_tab(url, task=None):
        result = command(task, "open", url) if task else call("open", url)
        tab = result["tab"]
        require(tab not in initial and tab not in owned_tabs, "Reused tab ID")
        owned_tabs.add(tab)
        if task:
            command(task, "wait", tab, "load", 10000)
        else:
            call("wait", tab, "load", 10000)
        return tab

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if urlsplit(self.path).path == "/slow":
                requested.set()
                release.wait(20)
            body = b'''<!doctype html><html><head><title>Task fixture</title></head>
<body><h1>Local task fixture</h1>
<button onclick="this.textContent='Clicked'">Task action</button></body></html>'''
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
        require(call("status")["engine"] == "CEF/Chromium", "Expected real CEF app")
        initial = tab_ids()
        require(initial, "Keep an initial tab open before running this test")
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        origin = f"http://127.0.0.1:{server.server_port}"
        for name in ("Task fixture A", "Task fixture B"):
            task = call("task", "create", name)["task"]
            require(task["status"] == "active" and task["revision"] == 1 and not task["tabs"],
                    "New task must be active with no tabs and revision 1")
            tasks[task["id"]] = task
        a, b = list(tasks.values())
        require({a["id"], b["id"]} <= {task["id"] for task in call("task", "list")["tasks"]},
                "Tasks must survive separate CLI connections")
        tab_a, tab_b = open_tab(origin + "/a", a), open_tab(origin + "/b", b)
        require({tab["tab"] for tab in command(a, "tabs")["tabs"]} == {tab_a}, "Task tabs leaked another task")
        call("read", tab_a, expected="TASK_REQUIRED")
        command(b, "read", tab_a, expected="TASK_TAB_MISMATCH")
        report["checks"].append("task state survives CLI exit; scoped tabs and foreign/unscoped access checks")

        old_a = a.copy()
        reference = element(command(a, "read", tab_a)["page"], "Task action")["ref"]
        a, _ = manage("handoff", a)
        command(a, "read", tab_a, expected="TASK_PAUSED")
        a, _ = manage("resume", a)
        command(old_a, "read", tab_a, expected="TASK_STALE_REVISION")
        # Before another read, so the native control revision must expire this ref.
        command(a, "click", tab_a, reference, expected="STALE_REF")
        fresh = element(command(a, "read", tab_a)["page"], "Task action")["ref"]
        command(a, "click", tab_a, fresh)
        element(command(a, "read", tab_a)["page"], "Clicked")
        report["checks"].append("handoff/resume rejects old revision and refs; fresh observation permits action")

        borrowed = open_tab(origin + "/borrowed")
        a, _ = manage("adopt", a, borrowed)
        require(any(tab == {"tab": borrowed, "origin": "borrowed"} for tab in a["tabs"]), "Missing borrowed origin")
        call("read", borrowed, expected="TASK_REQUIRED")
        a, _ = manage("release", a, borrowed)
        call("read", borrowed)
        a, _ = manage("adopt", a, borrowed)
        close_a = open_tab(origin + "/close", a)
        before = info(a)
        call("task", "finish", a["id"], a["revision"], "--keep", tab_b, expected="INVALID_KEEP")
        require(info(a) == before, "Invalid keep partially changed a task")
        report["checks"].append("adopt/release preserves the borrowed tab; invalid finish is atomic")

        baseline = call("status")["pendingWaits"]
        command(b, "navigate", tab_b, origin + "/slow")
        require(requested.wait(5), "Browser did not request the delayed fixture")
        executor = ThreadPoolExecutor(max_workers=1)
        pending = executor.submit(cli.request, "task", "run", b["id"], b["revision"], "wait", tab_b, "load", 10000)
        poll(lambda: call("status")["pendingWaits"], lambda count: count > baseline, "Wait was not registered")
        b, _ = manage("handoff", b)
        cancelled = pending.result(timeout=3)
        require(not cancelled["ok"] and cancelled["error"]["code"] == "TASK_CONTROL_LOST", "Handoff did not cancel wait")
        require(cancelled["error"]["details"]["outcomeUnknown"] is False, "Read-only wait is not a dispatched mutation")
        release.set()
        b, _ = manage("resume", b)
        command(b, "wait", tab_b, "load", 10000)
        report["checks"].append("registered wait cancels on handoff; navigation can finish after resume")

        a, finish = manage("finish", a, "--keep", tab_a)
        require(set(finish["closeRequested"]) == {close_a}, "Finish requested closing a protected tab")
        require(set(a["releasedTabs"]) == {tab_a, borrowed}, "Finish did not preserve keep and borrowed tabs")
        a = poll(lambda: info(a), lambda task: task["status"] == "finished", "Task A never finished")
        require(close_a not in tab_ids() and {tab_a, borrowed} <= tab_ids(), "Close callback and preserved tabs disagree")
        call("read", tab_a)
        call("read", borrowed)
        b, _ = manage("finish", b)
        b = poll(lambda: info(b), lambda task: task["status"] == "finished", "Task B never finished")
        require(tab_b not in tab_ids(), "Finished task retained a created tab")
        repeated = call("task", "finish", b["id"], b["revision"])
        require(repeated["task"]["revision"] == b["revision"] and not repeated["closeRequested"], "Repeated finish changed a completed plan")
        report["checks"].append("finish preserves borrowed/keep tabs and completes only after created tabs close")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, TimeoutError) as error:
        report["error"] = str(error)
        report["lastResponse"] = last_response
        try:
            report["ownedTabStates"] = [tab for tab in cli.success("tabs")["tabs"] if tab["tab"] in owned_tabs]
        except (CheckFailed, OSError):
            pass
    finally:
        release.set()
        if executor:
            executor.shutdown(wait=True, cancel_futures=True)
        cleanup = []
        for task in tasks.values():
            try:
                current = info(task)
                if current["status"] != "finished":
                    call("task", "finish", current["id"], current["revision"])
                    poll(lambda: info(current), lambda value: value["status"] == "finished", "Cleanup could not finish its task")
            except (CheckFailed, OSError, KeyError, TypeError) as error:
                cleanup.append(str(error))
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
            require(initial <= final, "An initial tab disappeared")
            report["checks"].append("only test-owned tasks and fixture tabs cleaned up; initial tabs preserved")
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
    args = parser.parse_args()
    if args.data_dir is None:
        parser.error("--data-dir or LIGHT_BROWSER_DATA_DIR is required")
    report = run(args)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
