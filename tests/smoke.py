#!/usr/bin/env python3
"""Exercise a running Light Browser through its public CLI against a local fixture.

Example:
  python3 tests/smoke.py --cli target/debug/lightbrowser --data-dir /private/tmp/light-browser-dev

The app must already be running with the same LIGHT_BROWSER_DATA_DIR. Only tabs
created by this script are closed. This does not test third-party compatibility.
"""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time
from urllib.parse import urlsplit
import uuid


ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "tests" / "fixtures" / "browser.html"
FAKE_PASSWORD = "FAKE_PASSWORD_FOR_TEST_ONLY"
TEST_QUERY = "light browser local test 한글"


class CheckFailed(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise CheckFailed(message)


def make_handler(content):
    class FixtureHandler(BaseHTTPRequestHandler):
        def do_GET(self):
            path = urlsplit(self.path).path
            if path == "/favicon.ico":
                self.send_response(204)
                self.end_headers()
                return
            if path not in ("/fixture", "/search"):
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(content)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(content)

        def log_message(self, _format, *_args):
            # Deliberately avoid logging query parameters or browser headers.
            pass

    return FixtureHandler


class BrowserCli:
    def __init__(self, executable, data_dir):
        self.executable = str(executable)
        self.environment = dict(os.environ, LIGHT_BROWSER_DATA_DIR=str(data_dir))

    def request(self, command, *args):
        try:
            process = subprocess.run(
                [self.executable, command, *(str(arg) for arg in args)],
                env=self.environment,
                capture_output=True,
                text=True,
                timeout=55,
                check=False,
            )
        except subprocess.TimeoutExpired as error:
            raise CheckFailed(f"{command}: timed out; the action will not be retried") from error
        except OSError as error:
            raise CheckFailed(f"{command}: could not execute the CLI") from error
        try:
            response = json.loads(process.stdout)
        except (json.JSONDecodeError, UnicodeDecodeError) as error:
            raise CheckFailed(f"{command}: CLI did not return one JSON response") from error
        require(isinstance(response, dict) and isinstance(response.get("ok"), bool),
                f"{command}: invalid response shape")
        require((process.returncode == 0) == response["ok"],
                f"{command}: exit code disagrees with response.ok")
        return response

    def success(self, command, *args):
        response = self.request(command, *args)
        if not response["ok"]:
            code = response.get("error", {}).get("code", "UNKNOWN_ERROR")
            raise CheckFailed(f"{command}: {code}")
        require("result" in response, f"{command}: result is missing")
        return response["result"]

    def error(self, expected, command, *args):
        response = self.request(command, *args)
        actual = response.get("error", {}).get("code")
        require(not response["ok"] and actual == expected,
                f"{command}: expected {expected}, received {actual or 'success'}")

    def wait_for_page(self, tab, predicate, description):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            response = self.request("read", tab)
            if response["ok"]:
                page = response.get("result", {}).get("page")
                require(isinstance(page, dict), "read: page object is missing")
                if predicate(page):
                    return page
            else:
                code = response.get("error", {}).get("code")
                if code not in ("PAGE_LOADING", "PAGE_CHANGED", "PAGE_SCRIPT_ERROR", "CONTEXT_UNAVAILABLE", "FRAME_UNAVAILABLE"):
                    raise CheckFailed(f"read: {code or 'UNKNOWN_ERROR'}")
            # Only idempotent reads are retried; click, fill and Enter run once.
            time.sleep(0.1)
        raise CheckFailed(f"read: timed out waiting for {description}")


def element(page, label):
    matches = [item for item in page.get("interactive", []) if item.get("label") == label]
    require(len(matches) == 1, f"Expected one interactive element labelled {label!r}")
    return matches[0]


def require_native_press_ack(page):
    """Validate engine dispatch acknowledgement, not the resulting page effect."""
    require(isinstance(page, dict) and page.get("ok") is True and
            page.get("action") == "press" and page.get("key") == "Enter" and
            page.get("input") == "chromium" and page.get("attempted") is True and
            page.get("dispatchAcknowledged") is True and isinstance(page.get("detail"), str),
            "press: expected Chromium Enter dispatch acknowledgement")
    require("trusted" not in page and "formSubmissionRequested" not in page,
            "press: engine acknowledgement must not claim event trust or form submission")


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    checks = []
    owned_tabs = []
    report = {"ok": False, "checks": checks, "scope": "local CEF fixture through CLI"}
    server = None
    server_thread = None
    initial_tabs = set()
    try:
        status = cli.success("status")
        require(status.get("engine") == "CEF/Chromium", "status: expected CEF/Chromium engine")
        initial_tabs = {tab["tab"] for tab in cli.success("tabs")["tabs"]}
        require(initial_tabs, "Keep the app's initial tab open before running this test")
        checks.append("running CEF app and CLI connection")

        server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(FIXTURE.read_bytes()))
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        url = f"http://127.0.0.1:{server.server_port}/fixture"
        opened = cli.success("open", url)
        tab = opened.get("tab")
        require(isinstance(tab, int) and tab not in initial_tabs, "open: missing or reused tab ID")
        owned_tabs.append(tab)
        page = cli.wait_for_page(tab, lambda value: value.get("title") == "Light Browser local fixture", "fixture")
        require(page.get("documentId"), "read: isolated DOM bridge response is missing")
        serialized = json.dumps(page)
        require(FAKE_PASSWORD not in serialized, "read: password value was disclosed")
        require("HIDDEN_VALUE_MUST_NOT_BE_EXPOSED" not in serialized, "read: hidden value was disclosed")
        require("Hidden button must not be exposed" not in serialized, "read: hidden button was disclosed")
        require("MAIN_WORLD_BRIDGE_MUST_NOT_BE_USED" not in serialized, "read: page-owned bridge was used")
        require("value" not in element(page, "Fixture password"), "read: password includes a value property")
        checks.append("page reading, isolated world and password/hidden-value omission")

        old_counter_ref = element(page, "Increment counter")["ref"]
        page = cli.success("read", tab)["page"]
        cli.success("wait", tab, "dom")  # Readiness checks must preserve current refs.
        cli.error("STALE_REF", "click", tab, old_counter_ref)
        cli.success("click", tab, element(page, "Increment counter")["ref"])
        page = cli.wait_for_page(tab, lambda value: "Clicks: 1" in value.get("text", ""), "button event")
        checks.append("refs expire after read, survive wait, and DOM click reaches page handlers")

        target_ref = element(page, "Target button")["ref"]
        cli.success("click", tab, element(page, "Disable target")["ref"])
        cli.error("ELEMENT_UNAVAILABLE", "click", tab, target_ref)
        checks.append("disabled state revalidated before action")

        page = cli.success("read", tab)["page"]
        query_ref = element(page, "Search query")["ref"]
        cli.success("fill", tab, query_ref, TEST_QUERY)
        page = cli.success("read", tab)["page"]
        require(element(page, "Search query").get("value") == TEST_QUERY, "fill: input value did not change")
        old_document = page["documentId"]
        old_ref = element(page, "Search query")["ref"]
        pressed = cli.success("press", tab, old_ref, "Enter")["page"]
        require_native_press_ack(pressed)
        page = cli.wait_for_page(
            tab,
            lambda value: value.get("title") == "Fixture search results" and f"Search result: {TEST_QUERY}" in value.get("text", ""),
            "search results",
        )
        require(page["documentId"] != old_document, "navigation: document ID did not change")
        require(urlsplit(page["url"]).path == "/search", "navigation: search URL did not load")
        cli.error("STALE_REF", "fill", tab, old_ref, "must not apply")
        checks.append("fill, Enter, real form navigation and navigation ref expiry")

        artifacts = args.artifacts_dir.resolve()
        artifacts.mkdir(parents=True, exist_ok=True)
        screenshot = artifacts / f"local-fixture-{uuid.uuid4().hex[:12]}.png"
        captured = cli.success("screenshot", tab, screenshot)
        require(captured.get("path") == str(screenshot), "screenshot: returned path mismatch")
        with screenshot.open("rb") as image:
            require(image.read(8) == b"\x89PNG\r\n\x1a\n", "screenshot: file is not PNG")
        require(screenshot.stat().st_size > 100, "screenshot: file is empty")
        report["screenshot"] = str(screenshot)
        checks.append("screenshot saved as PNG")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError) as error:
        report["error"] = str(error)
    finally:
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
                    remaining = {tab["tab"] for tab in cli.success("tabs")["tabs"]}
                    require(initial_tabs <= remaining, "cleanup: an initial user tab is missing")
                    if not remaining.intersection(owned_tabs):
                        checks.append("only fixture tab closed; initial tabs preserved")
                        break
                    require(time.monotonic() < deadline, "cleanup: fixture tab did not close")
                    time.sleep(0.1)
            except (CheckFailed, KeyError, TypeError) as error:
                cleanup_errors.append(str(error))
        if cleanup_errors:
            report["ok"] = False
            report["cleanupErrors"] = cleanup_errors
        if server:
            server.shutdown()
            server.server_close()
        if server_thread:
            server_thread.join(timeout=2)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--cli", "--cli-path", type=Path, default=ROOT / "target" / "debug" / "lightbrowser")
    parser.add_argument("--data-dir", type=Path, default=os.environ.get("LIGHT_BROWSER_DATA_DIR"),
                        help="Data directory used by the already running app (or LIGHT_BROWSER_DATA_DIR)")
    parser.add_argument("--artifacts-dir", type=Path, default=ROOT / "artifacts" / "smoke")
    args = parser.parse_args()
    if args.data_dir is None:
        parser.error("--data-dir or LIGHT_BROWSER_DATA_DIR is required")
    report = run(args)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
