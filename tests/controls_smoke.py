#!/usr/bin/env python3
"""Exercise browser navigation and scrolling through an already running CLI.

Example:
  python3 tests/controls_smoke.py --cli target/release/lightbrowser \
      --data-dir /private/tmp/naru-dev

Uses a local HTTP fixture and closes only its own tab. The script does not
launch the app, create or delete profiles, or navigate existing user tabs.
"""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import sys
import threading
import time
from urllib.parse import urlsplit

from smoke import BrowserCli, CheckFailed, ROOT, element, require


class Gate:
    def __init__(self):
        self.requested = threading.Event()
        self.release = threading.Event()


def fixture(page):
    return f"""<!doctype html><html lang="en"><head><meta charset="utf-8">
<title>Controls {page}</title><style>
html {{ scroll-behavior: auto; }}
body {{ margin: 0; width: 16000px; min-height: 14000px; }}
button {{ position: fixed; left: 16px; top: 16px; }}
h1 {{ margin: 80px 16px 0; }} #section {{ margin-top: 1600px; }}
</style></head><body><h1>Controls page {page}</h1>
<button type="button" onclick="this.textContent='Clicked'">Page {page} action</button>
<p id="section">Same document anchor</p></body></html>""".encode("utf-8")


def make_handler(gate):
    class ControlsHandler(BaseHTTPRequestHandler):
        def do_GET(self):
            path = urlsplit(self.path).path
            if path == "/slow":
                gate.requested.set()
                released = gate.release.wait(30)
                body = fixture("Slow")
            elif path in ("/a", "/b"):
                released = True
                body = fixture(path[-1].upper())
            elif path == "/no-content":
                self.send_response(204)
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                return
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
                pass  # stop or cleanup may already have cancelled this response.

        def log_message(self, _format, *_args):
            pass

    return ControlsHandler


def find_tab(cli, tab):
    matches = [value for value in cli.success("tabs")["tabs"] if value.get("tab") == tab]
    require(len(matches) == 1, "Expected exactly one existing control-test tab")
    state = matches[0]
    for name in ("loading", "canGoBack", "canGoForward"):
        require(isinstance(state.get(name), bool), f"tabs: {name} flag is missing")
    return state


def request_navigation(cli, action, tab, *args):
    result = cli.success(action, tab, *args)
    require(result.get("tab") == tab and result.get("navigationRequested") is True and
            result.get("action") == action, f"{action}: expected accepted navigation on the same tab")


def wait_navigation_stopped(cli, tab):
    # A failed readiness wait reports OnLoadError immediately. Chromium's
    # overall loading=false callback may arrive later; do not race that barrier.
    deadline = time.monotonic() + 5
    while True:
        state = find_tab(cli, tab)
        if not state["loading"] and not state["navigation"]["provisional"]:
            return
        require(time.monotonic() < deadline, "Failed navigation did not stop loading")
        threading.Event().wait(0.02)


def wait_page(cli, tab, url, stale_ref=None):
    state = cli.success("wait", tab, "load", 10000)
    require(state.get("tab") == tab and state.get("url") == url and state.get("loading") is False,
            "wait: navigation finished on the wrong tab or URL")
    require(state.get("navigation", {}).get("error") is None, "wait: successful navigation retained an error")
    if stale_ref is not None:
        # Check before read, which would independently invalidate prior refs.
        cli.error("STALE_REF", "click", tab, stale_ref)
    page = cli.success("read", tab)["page"]
    require(page.get("url") == url and page.get("documentId"), "read: missing expected document")
    return page


def viewport(page):
    value = page.get("viewport")
    require(isinstance(value, dict), "read: viewport is missing")
    for name in ("x", "y", "width", "height"):
        require(type(value.get(name)) in (int, float), f"read: viewport.{name} is missing")
    require(value["width"] > 0 and value["height"] > 0, "read: viewport is empty")
    return value


def scroll_and_check(cli, tab, dx, dy, expected_x, expected_y):
    result = cli.success("scroll", tab, dx, dy)
    page = result.get("page", {})
    require(result.get("tab") == tab and page.get("ok") is True and page.get("action") == "scroll",
            "scroll: action result is missing")
    current = viewport(cli.success("read", tab)["page"])
    for name, expected in (("x", expected_x), ("y", expected_y)):
        require(type(page.get(name)) in (int, float) and abs(page[name] - expected) <= 1,
                f"scroll: {name} did not move to the expected CSS pixel offset")
        require(abs(current[name] - page[name]) <= 1, f"scroll: result and read.viewport.{name} disagree")


def details_for(response, code):
    error = response.get("error", {})
    require(not response["ok"] and error.get("code") == code,
            f"Expected {code}, got {error.get('code', 'success')}")
    details = error.get("details")
    require(isinstance(details, dict), f"{code}: details are missing")
    return details


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    checks = []
    report = {"ok": False, "scope": "real CEF controls through local CLI fixtures", "checks": checks}
    initial_tabs = set()
    owned_tabs = []
    gate = Gate()
    server = server_thread = refused_socket = None
    try:
        require(cli.success("status").get("engine") == "CEF/Chromium", "Expected running CEF app")
        initial_tabs = {value["tab"] for value in cli.success("tabs")["tabs"]}
        require(initial_tabs, "Keep the app's initial tab open before running this test")
        server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(gate))
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        origin = f"http://127.0.0.1:{server.server_port}"
        url_a, url_b = origin + "/a", origin + "/b"
        opened = cli.success("open", url_a)
        tab = opened.get("tab")
        require(type(tab) is int and tab not in initial_tabs, "open: missing or reused tab ID")
        owned_tabs.append(tab)
        page_a = wait_page(cli, tab, url_a)
        old_a_ref = element(page_a, "Page A action")["ref"]
        state = find_tab(cli, tab)
        require(not state["canGoBack"] and not state["canGoForward"], "New tab unexpectedly has history")
        cli.error("NO_HISTORY", "back", tab)
        cli.error("NO_HISTORY", "forward", tab)
        checks.append("new tab history flags and NO_HISTORY responses")

        request_navigation(cli, "navigate", tab, url_b)
        page_b = wait_page(cli, tab, url_b, stale_ref=old_a_ref)
        old_b_ref = element(page_b, "Page B action")["ref"]
        require(find_tab(cli, tab)["canGoBack"], "A to B navigation did not create history")
        request_navigation(cli, "back", tab)
        page_a = wait_page(cli, tab, url_a, stale_ref=old_a_ref)
        state = find_tab(cli, tab)
        require(not state["canGoBack"] and state["canGoForward"], "Back did not restore history flags")
        request_navigation(cli, "forward", tab)
        page_b = wait_page(cli, tab, url_b, stale_ref=old_b_ref)
        state = find_tab(cli, tab)
        require(state["canGoBack"] and not state["canGoForward"], "Forward did not restore history flags")
        before_reload = page_b["documentId"]
        old_b_ref = element(page_b, "Page B action")["ref"]
        request_navigation(cli, "reload", tab)
        page_b = wait_page(cli, tab, url_b, stale_ref=old_b_ref)
        require(page_b["documentId"] != before_reload, "Reload reused the previous document ID")
        checks.append("same-tab navigate/back/forward/reload and ref expiry before a new read")

        hash_url = url_b + "#section"
        request_navigation(cli, "navigate", tab, hash_url)
        dom = cli.success("wait", tab, "dom", 5000)
        require(dom.get("tab") == tab and dom.get("url") == hash_url, "DOM wait missed same-document navigation")
        page_b = wait_page(cli, tab, hash_url)
        require(page_b.get("title") == "Controls B", "Hash navigation lost the current page")
        scroll_and_check(cli, tab, -10000, -10000, 0, 0)
        scroll_and_check(cli, tab, 160, 480, 160, 480)
        scroll_and_check(cli, tab, -80, -240, 80, 240)
        checks.append("same-document dom/load waits and real horizontal/vertical viewport scrolling")

        refused_socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        refused_socket.bind(("127.0.0.1", 0))
        failed_url = f"http://127.0.0.1:{refused_socket.getsockname()[1]}/refused"
        request_navigation(cli, "navigate", tab, failed_url)
        details = details_for(cli.request("wait", tab, "load", 10000), "NAVIGATION_FAILED")
        failed_state = details.get("state", {})
        failure = failed_state.get("navigation", {}).get("error")
        require(failed_state.get("tab") == tab and isinstance(failure, dict) and failure.get("failedUrl") == failed_url,
                "Failed navigation lost its tab or requested URL")
        wait_navigation_stopped(cli, tab)
        request_navigation(cli, "navigate", tab, url_a)
        page_a = wait_page(cli, tab, url_a)
        require(page_a.get("title") == "Controls A", "Navigation did not recover from the connection failure")
        checks.append("same-tab recovery from a refused localhost navigation")

        request_navigation(cli, "navigate", tab, origin + "/no-content")
        no_content = cli.request("wait", tab, "load", 10000)
        code = no_content.get("error", {}).get("code")
        require(not no_content["ok"] and code in ("NAVIGATION_INCOMPLETE", "NAVIGATION_FAILED"),
                "A 204 navigation incorrectly succeeded using the previous document")
        details = details_for(no_content, code)
        state = details.get("state", {})
        require(state.get("tab") == tab, "204 failure lost the existing tab")
        if code == "NAVIGATION_INCOMPLETE":
            require(state.get("navigation", {}).get("incomplete") is True, "204 incomplete state was not retained")
        wait_navigation_stopped(cli, tab)
        page_a = cli.success("read", tab)["page"]
        require(page_a.get("title") == "Controls A" and page_a.get("url") == url_a,
                "204 response made the previous committed DOM unreadable")
        request_navigation(cli, "navigate", tab, url_a)
        require(wait_page(cli, tab, url_a).get("title") == "Controls A", "Navigation did not recover after 204")
        checks.append("204 response cannot satisfy wait with the old page; old DOM remains readable and navigation recovers")

        request_navigation(cli, "navigate", tab, origin + "/slow")
        require(gate.requested.wait(10), "Browser did not request the gated main response")
        timeout = details_for(cli.request("wait", tab, "dom", 250), "WAIT_TIMEOUT")
        require(timeout.get("tab") == tab and timeout.get("until") == "dom" and
                timeout.get("navigationContinues") is True and timeout.get("state", {}).get("tab") == tab,
                "Gated navigation did not retain its pending state")
        cli.error("PAGE_LOADING", "read", tab)
        cli.error("NAVIGATION_BUSY", "navigate", tab, url_b)
        stopped = cli.success("stop", tab)
        require(stopped.get("tab") == tab and stopped.get("stopRequested") is True,
                "stop: request was not accepted on the existing tab")
        deadline = time.monotonic() + 5
        while find_tab(cli, tab)["loading"]:
            require(time.monotonic() < deadline, "stop: tab remained loading")
            threading.Event().wait(0.02)
        stopped_wait = details_for(cli.request("wait", tab, "dom", 1000), "NAVIGATION_STOPPED")
        require(stopped_wait.get("state", {}).get("navigation", {}).get("stopped") is True,
                "Explicit stop did not retain its navigation state")
        page_a = cli.success("read", tab)["page"]
        require(page_a.get("title") == "Controls A" and page_a.get("url") == url_a and
                "Controls page A" in page_a.get("text", ""),
                "stop: previous committed DOM is not readable")
        request_navigation(cli, "navigate", tab, url_a)
        require(wait_page(cli, tab, url_a).get("title") == "Controls A", "Navigation did not recover after stop")
        checks.append("gated navigation times out and rejects overlap; stop reports stopped, restores old DOM, and allows recovery")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
        report["error"] = str(error)
    finally:
        gate.release.set()
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
                    remaining = {value["tab"] for value in cli.success("tabs")["tabs"]}
                    require(initial_tabs <= remaining, "cleanup: an initial user tab is missing")
                    if not remaining.intersection(owned_tabs):
                        checks.append("only test-owned tabs closed; initial tabs preserved")
                        break
                    require(time.monotonic() < deadline, "cleanup: test-owned tab did not close")
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
    parser.add_argument("--cli", type=Path, default=ROOT / "target/release/lightbrowser")
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
