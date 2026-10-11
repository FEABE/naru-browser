#!/usr/bin/env python3
"""Check navigation readiness through an already running app's public CLI.

Example:
  python3 tests/navigation_smoke.py --cli target/debug/lightbrowser \
      --data-dir /private/tmp/light-browser-dev

Only localhost fixtures and tabs created here are used. The script does not
launch the app, create profiles, or change existing tabs.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor, TimeoutError as FutureTimeout
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import sys
import threading
import time
from urllib.parse import urlsplit

from smoke import BrowserCli, CheckFailed, ROOT, require


RESPONSIVE_SECONDS = 2
MARKER = "Main document is readable before the iframe finishes"


class Gates:
    def __init__(self):
        self.main_requested = threading.Event()
        self.main_release = threading.Event()
        self.frame_requested = threading.Event()
        self.frame_release = threading.Event()

    def release(self):
        self.main_release.set()
        self.frame_release.set()


def make_handler(gates):
    class NavigationHandler(BaseHTTPRequestHandler):
        def do_GET(self):
            path = urlsplit(self.path).path
            if path == "/main":
                gates.main_requested.set()
                released = gates.main_release.wait(30)
                body = (f'<!doctype html><html><head><title>Navigation fixture</title></head>'
                        f'<body><h1>{MARKER}</h1><iframe src="/frame"></iframe></body></html>').encode()
            elif path == "/frame":
                gates.frame_requested.set()
                released = gates.frame_release.wait(30)
                body = b"<!doctype html><html><body>Iframe finished</body></html>"
            else:
                self.send_response(204)
                self.end_headers()
                return
            try:
                self.send_response(200 if released else 504)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                pass  # A failed test may close its own tab before a gate is released.

        def log_message(self, _format, *_args):
            pass

    return NavigationHandler


def tab_state(value, tab):
    require(isinstance(value, dict) and value.get("tab") == tab, "Missing navigation tab ID")
    require(isinstance(value.get("url"), str), "Tab URL is missing")
    require(isinstance(value.get("loading"), bool), "Tab loading flag is missing")
    navigation = value.get("navigation")
    require(isinstance(navigation, dict), "Tab navigation state is missing")
    for name in ("committed", "documentAvailable", "mainLoadComplete"):
        require(isinstance(navigation.get(name), bool), f"navigation.{name} is missing")
    require("error" in navigation, "navigation.error is missing")
    return navigation


def find_tab(cli, tab):
    matches = [state for state in cli.success("tabs")["tabs"] if state.get("tab") == tab]
    require(len(matches) == 1, "Navigation tab disappeared or has a duplicate ID")
    tab_state(matches[0], tab)
    return matches[0]


def error_details(response, code):
    error = response.get("error", {})
    require(not response["ok"] and error.get("code") == code,
            f"Expected {code}, received {error.get('code', 'success')}")
    details = error.get("details")
    require(isinstance(details, dict), f"{code}: details are missing")
    return details


def completed(future, seconds, description):
    try:
        return future.result(timeout=seconds)
    except FutureTimeout as error:
        raise CheckFailed(f"{description}: did not respond within {seconds} seconds") from error


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    checks = []
    report = {"ok": False, "scope": "local gated navigation through real CEF CLI", "checks": checks}
    owned_tabs = []
    initial_tabs = set()
    gates = Gates()
    pool = ThreadPoolExecutor(max_workers=3)
    server = server_thread = refused_socket = None
    try:
        initial_status = cli.success("status")
        require(initial_status.get("engine") == "CEF/Chromium", "Expected running CEF app")
        baseline_waits = initial_status.get("pendingWaits")
        require(type(baseline_waits) is int and baseline_waits >= 0, "status: pendingWaits count is missing")
        report["initialPendingWaits"] = baseline_waits
        initial_tabs = {state["tab"] for state in cli.success("tabs")["tabs"]}
        require(initial_tabs, "Keep the app's initial tab open before running this test")
        server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(gates))
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        url = f"http://127.0.0.1:{server.server_port}/main"

        def open_owned(address):
            opened = cli.success("open", address)
            tab = opened.get("tab")
            require(type(tab) is int and tab not in initial_tabs, "open: missing or reused tab ID")
            owned_tabs.append(tab)
            return opened

        started = time.monotonic()
        opened = completed(pool.submit(open_owned, url), RESPONSIVE_SECONDS, "open with a gated response")
        report["openMs"] = round((time.monotonic() - started) * 1000)
        tab = opened["tab"]
        require(not tab_state(opened, tab)["documentAvailable"], "open unexpectedly waited for a document")
        require(gates.main_requested.wait(10), "Browser did not request the local main document")
        state = find_tab(cli, tab)
        require(state["loading"] and not state["navigation"]["committed"] and
                not state["navigation"]["documentAvailable"], "Gated response already has a committed document")
        cli.error("PAGE_LOADING", "read", tab)
        checks.append("open returns a usable tab ID before response headers; read reports PAGE_LOADING")

        pending = pool.submit(cli.success, "wait", tab)  # Default dom / 10000 ms.
        deadline = time.monotonic() + RESPONSIVE_SECONDS
        while True:
            remaining = deadline - time.monotonic()
            require(remaining > 0, "DOM wait was not registered within the response deadline")
            status = completed(pool.submit(cli.success, "status"), remaining, "status while registering DOM wait")
            count = status.get("pendingWaits")
            require(type(count) is int and count >= 0, "status: pendingWaits count is missing")
            require(not pending.done(), "DOM wait completed without a document")
            if count > baseline_waits:
                report["registeredPendingWaits"] = count
                break
        started = time.monotonic()
        status, state = completed(
            pool.submit(lambda: (cli.success("status"), find_tab(cli, tab))),
            RESPONSIVE_SECONDS, "status and tabs during a registered wait")
        elapsed = time.monotonic() - started
        report["statusAndTabsDuringWaitMs"] = round(elapsed * 1000)
        require(elapsed < RESPONSIVE_SECONDS, "A pending wait blocked status/tabs")
        require(status.get("pendingWaits", 0) > baseline_waits and not pending.done() and
                not state["navigation"]["documentAvailable"],
                "DOM wait did not remain pending while the main response was gated")
        checks.append("server-registered pending wait keeps status and tabs responsive")

        gates.main_release.set()
        require(gates.frame_requested.wait(10), "Browser did not request the delayed iframe")
        dom = completed(pending, 10, "DOM readiness")
        navigation = tab_state(dom, tab)
        require(navigation["committed"] and navigation["documentAvailable"] and
                navigation["error"] is None and dom["loading"], "DOM wait did not finish during iframe loading")
        if "readyState" in dom:
            require(dom["readyState"] in ("interactive", "complete"), "DOM wait returned a loading document")
        page = cli.success("read", tab)["page"]
        require(MARKER in page.get("text", ""), "Main document is unreadable while iframe loads")
        require(find_tab(cli, tab)["loading"], "Delayed iframe stopped holding the load open")
        checks.append("default DOM wait and read succeed while the delayed iframe keeps loading true")

        timeout_ms = 250
        details = error_details(cli.request("wait", tab, "load", timeout_ms), "WAIT_TIMEOUT")
        require(details.get("tab") == tab and details.get("until") == "load" and
                details.get("timeoutMs") == timeout_ms and details.get("navigationContinues") is True,
                "WAIT_TIMEOUT does not describe the continuing navigation")
        require(isinstance(details.get("elapsedMs"), (int, float)) and details["elapsedMs"] >= timeout_ms,
                "WAIT_TIMEOUT elapsedMs is missing or shorter than its deadline")
        navigation = tab_state(details.get("state"), tab)
        require(details["state"]["loading"] and navigation["documentAvailable"] and navigation["error"] is None,
                "WAIT_TIMEOUT lost the live navigation state")
        require(find_tab(cli, tab)["loading"], "Timeout stopped or removed the navigation")
        gates.frame_release.set()
        loaded = cli.success("wait", tab, "load", 10000)
        navigation = tab_state(loaded, tab)
        require(not loaded["loading"] and navigation["committed"] and navigation["documentAvailable"] and
                navigation["mainLoadComplete"] and navigation["error"] is None,
                "Load wait returned before the main navigation completed")
        require(loaded["url"] == url, "Completed navigation URL changed")
        checks.append("load timeout preserves the tab and state; later load wait succeeds after iframe release")

        # Reserve a local TCP port without listening: connections are refused,
        # and another test cannot acquire the port between allocation and open.
        refused_socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        refused_socket.bind(("127.0.0.1", 0))
        failed_url = f"http://127.0.0.1:{refused_socket.getsockname()[1]}/refused"
        failed = completed(pool.submit(open_owned, failed_url), RESPONSIVE_SECONDS, "open to a refused local port")
        failed_tab = failed["tab"]
        details = error_details(cli.request("wait", failed_tab, "load", 10000), "NAVIGATION_FAILED")
        failure = tab_state(details.get("state"), failed_tab)["error"]
        require(isinstance(failure, dict) and type(failure.get("code")) is int and failure["code"] < 0 and
                isinstance(failure.get("text"), str) and failure["text"] and failure.get("failedUrl") == failed_url,
                "Navigation failure is missing Chromium code, text, or requested URL")
        require(find_tab(cli, failed_tab)["navigation"]["error"] == failure,
                "tabs did not retain the failed navigation and its tab ID")
        repeated = error_details(cli.request("wait", failed_tab, "dom", 1000), "NAVIGATION_FAILED")
        require(tab_state(repeated.get("state"), failed_tab)["error"] == failure,
                "A later wait lost the navigation error")
        checks.append("refused localhost navigation retains tab ID and Chromium error in wait and tabs")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
        report["error"] = str(error)
    finally:
        # Release before joining pending commands so failure cleanup cannot keep
        # a legacy load-blocking open or an active wait trapped behind the fixture.
        gates.release()
        pool.shutdown(wait=True, cancel_futures=True)
        cleanup_errors = []
        for tab in owned_tabs:
            try:
                cli.success("close", tab)
            except CheckFailed as error:
                cleanup_errors.append(str(error))
        if owned_tabs:
            try:
                deadline = time.monotonic() + 5
                while True:
                    remaining = {state["tab"] for state in cli.success("tabs")["tabs"]}
                    require(initial_tabs <= remaining, "cleanup: an initial user tab is missing")
                    if not remaining.intersection(owned_tabs):
                        checks.append("only test-owned tabs closed; initial tabs preserved")
                        break
                    require(time.monotonic() < deadline, "cleanup: test-owned tabs did not close")
                    threading.Event().wait(0.05)
            except (CheckFailed, KeyError, TypeError) as error:
                cleanup_errors.append(str(error))
        if cleanup_errors:
            report["ok"] = False
            report["cleanupErrors"] = cleanup_errors
        if refused_socket:
            refused_socket.close()
        if server:
            server.shutdown()
            server.server_close()
        if server_thread:
            server_thread.join(timeout=2)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--cli", type=Path, default=ROOT / "target/debug/lightbrowser")
    parser.add_argument("--data-dir", type=Path, default=os.environ.get("LIGHT_BROWSER_DATA_DIR"),
                        help="Data directory of the already running app (or LIGHT_BROWSER_DATA_DIR)")
    args = parser.parse_args()
    if args.data_dir is None:
        parser.error("--data-dir or LIGHT_BROWSER_DATA_DIR is required")
    report = run(args)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
