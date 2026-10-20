#!/usr/bin/env python3
"""Verify `wait TAB url PREFIX` and `wait TAB text SUBSTR` through a running app.

Example:
  python3 tests/wait_smoke.py --cli target/release/lightbrowser \
      --data-dir "$LIGHT_BROWSER_DATA_DIR"

Does not launch the app or create profiles. Only localhost fixtures and the tabs
and task created here are used; the app's initial tab must stay open. Run without
other concurrent CLI tests so pendingWaits identifies this test's waits.
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

from smoke import BrowserCli, CheckFailed, ROOT, require


PAGES = {
    # Redirects after a delay: the native URL changes without any CLI action.
    "/redirect": ("Redirect fixture", "Redirecting soon",
                  "setTimeout(()=>location.replace('/landed?ok=1#frag'),700);"),
    "/landed": ("Landed", "Landed page", ""),
    # Hidden text appears first and must never satisfy a text wait.
    "/late-text": ("Late text", "Waiting for text",
                   "setTimeout(()=>{const h=document.createElement('p');h.style.display='none';"
                   "h.textContent='SECRET_HIDDEN_LATE';document.body.append(h);},200);"
                   "setTimeout(()=>{const p=document.createElement('p');"
                   "p.textContent='Late   text arrived';document.body.append(p);},700);"),
    # Navigates while a text wait may be probing; the wait must continue on the new document.
    "/nav-a": ("Nav A", "Page A without the target", "setTimeout(()=>location.href='/nav-b',600);"),
    "/nav-b": ("Nav B", "Page B loading",
               "setTimeout(()=>{const p=document.createElement('p');"
               "p.textContent='Arrived after navigation';document.body.append(p);},400);"),
}


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        title, text, script = PAGES.get(urlsplit(self.path).path, ("Missing", "Missing", ""))
        body = (f"<!doctype html><html><head><title>{title}</title></head><body><h1>{text}</h1>"
                f"<script>{script}</script></body></html>").encode()
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


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    checks = []
    report = {"ok": False, "scope": "wait url/text through real CEF CLI and localhost fixtures", "checks": checks}
    initial, owned_tabs = set(), set()
    server = server_thread = executor = None

    def tab_ids():
        return {state["tab"] for state in cli.success("tabs")["tabs"]}

    def tab_state(tab):
        matches = [state for state in cli.success("tabs")["tabs"] if state["tab"] == tab]
        require(len(matches) == 1, "Fixture tab disappeared")
        return matches[0]

    def open_tab(url):
        tab = cli.success("open", url)["tab"]
        require(tab not in initial and tab not in owned_tabs, "Reused tab ID")
        owned_tabs.add(tab)
        return tab

    def timed(*command):
        started = time.monotonic()
        response = cli.request(*command)
        return response, time.monotonic() - started

    def timeout_details(response, until, expected):
        error = response.get("error", {})
        require(not response["ok"] and error.get("code") == "WAIT_TIMEOUT",
                f"Expected WAIT_TIMEOUT, received {json.dumps(response, ensure_ascii=False)}")
        details = error.get("details", {})
        require(details.get("until") == until and details.get("expected") == expected and
                details.get("navigationContinues") is True and isinstance(details.get("elapsedMs"), (int, float)),
                f"WAIT_TIMEOUT details are incomplete: {details}")
        return details

    try:
        status = cli.success("status")
        require(status.get("engine") == "CEF/Chromium", "Expected the real CEF app")
        baseline = status["pendingWaits"]
        initial = tab_ids()
        require(initial, "Keep an initial tab open before running this test")
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        origin = f"http://127.0.0.1:{server.server_port}"

        # url: delayed redirect, prefix matching, immediate match.
        tab = open_tab(origin + "/redirect")
        response, seconds = timed("wait", tab, "url", origin + "/landed", 8000)
        require(response["ok"], f"url wait failed: {response}")
        result = response["result"]
        require(result.get("matched") == "url" and result["url"] == origin + "/landed?ok=1#frag",
                f"url wait returned an unexpected tab state: {result.get('url')}")
        require(seconds >= 0.4, f"url wait returned before the redirect ({seconds:.2f}s)")
        response, seconds = timed("wait", tab, "url", origin + "/landed?ok=1", 1000)
        require(response["ok"] and seconds < 2, "An already matching URL must return at once")
        checks.append(f"url wait returns on a delayed redirect; re-check is immediate ({seconds:.2f}s)")

        response, _ = timed("wait", tab, "url", "/landed", 300)
        timeout_details(response, "url", "/landed")
        require(tab_state(tab)["url"].startswith(origin + "/landed"), "Timeout changed the page")
        checks.append("url wait is a prefix match (not substring) and times out with WAIT_TIMEOUT, tab intact")

        # text: late visible text, hidden text ignored, case-sensitive, whitespace-collapsed.
        tab = open_tab(origin + "/late-text")
        response, seconds = timed("wait", tab, "text", "Late text arrived", 8000)
        require(response["ok"] and response["result"].get("matched") == "text", f"text wait failed: {response}")
        require(seconds >= 0.4, f"text wait returned before the text appeared ({seconds:.2f}s)")
        require("readyState" not in response["result"], "text wait must not report readyState")
        page = cli.success("read", tab)["page"]
        require("SECRET_HIDDEN_LATE" not in page["text"], "Hidden fixture text leaked into read")
        response, seconds = timed("wait", tab, "text", "SECRET_HIDDEN_LATE", 500)
        timeout_details(response, "text", "SECRET_HIDDEN_LATE")
        response, _ = timed("wait", tab, "text", "late text arrived", 400)
        timeout_details(response, "text", "late text arrived")
        checks.append("text wait sees late visible text, ignores hidden text, is case-sensitive")

        # text wait across a navigation restarts on the new document.
        nav_tab = open_tab(origin + "/nav-a")
        cli.success("wait", nav_tab, "load", 8000)
        response, _ = timed("wait", nav_tab, "text", "Arrived after navigation", 10000)
        require(response["ok"] and response["result"]["url"] == origin + "/nav-b",
                f"text wait did not follow the navigation: {response}")
        checks.append("a navigation during a text wait continues probing the new document")

        # Waiter bookkeeping is released after finish and timeout.
        deadline = time.monotonic() + 3
        while cli.success("status")["pendingWaits"] != baseline:
            require(time.monotonic() < deadline, "pendingWaits did not return to baseline")
            time.sleep(0.05)
        checks.append("pendingWaits returns to baseline after success and timeout")

        # task run: ownership, scoped wait, handoff cancels a registered text wait.
        task = cli.success("task", "create", "Wait smoke")["task"]

        def command(*operation):
            return cli.request("task", "run", task["id"], task["revision"], *operation)

        opened = command("open", origin + "/late-text")
        require(opened["ok"], f"task open failed: {opened}")
        task_tab = opened["result"]["tab"]
        owned_tabs.add(task_tab)
        response = command("wait", task_tab, "text", "Late text arrived", 8000)
        require(response["ok"] and response["result"].get("matched") == "text", f"task text wait failed: {response}")
        response = command("wait", task_tab, "url", origin + "/late-text", 2000)
        require(response["ok"] and response["result"].get("matched") == "url", f"task url wait failed: {response}")
        response = cli.request("wait", task_tab, "text", "x", 300)
        require(not response["ok"] and response["error"]["code"] == "TASK_REQUIRED", f"Unscoped wait: {response}")
        response = command("wait", tab, "url", origin, 300)
        require(not response["ok"] and response["error"]["code"] == "TASK_TAB_MISMATCH", f"Foreign wait: {response}")
        checks.append("task run url/text waits succeed on owned tabs; unscoped and foreign access are rejected")

        base = cli.success("status")["pendingWaits"]
        executor = ThreadPoolExecutor(max_workers=1)
        pending = executor.submit(command, "wait", task_tab, "text", "never appears", 20000)
        deadline = time.monotonic() + 5
        while cli.success("status")["pendingWaits"] <= base:
            require(time.monotonic() < deadline, "Text wait was not registered")
            time.sleep(0.025)
        handed = cli.success("task", "handoff", task["id"], task["revision"])["task"]
        cancelled = pending.result(timeout=5)
        require(not cancelled["ok"] and cancelled["error"]["code"] == "TASK_CONTROL_LOST",
                f"Handoff did not cancel the text wait: {cancelled}")
        checks.append("handoff cancels a registered text wait with TASK_CONTROL_LOST")
        task = cli.success("task", "resume", handed["id"], handed["revision"])["task"]
        cli.success("task", "finish", task["id"], task["revision"])
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
        report["error"] = str(error)
    finally:
        if executor:
            executor.shutdown(wait=True, cancel_futures=True)
        cleanup = []
        for tab in sorted(owned_tabs):
            try:
                cli.request("close", tab)
            except CheckFailed as error:
                cleanup.append(str(error))
        if owned_tabs:
            try:
                deadline = time.monotonic() + 5
                while owned_tabs & tab_ids():
                    require(time.monotonic() < deadline, "cleanup: test tabs did not close")
                    time.sleep(0.05)
                require(initial <= tab_ids(), "cleanup: an initial tab is missing")
            except (CheckFailed, KeyError, TypeError) as error:
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
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
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
