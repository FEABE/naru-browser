#!/usr/bin/env python3
"""Check single-window CLI metadata and tab behavior in an already running CEF app.

Uses two test-owned tasks and localhost pages; existing tabs are kept.
Does not launch the app or change profiles. Native window count, tab selection by
mouse, trusted popup creation and the visible stop control require separate UI verification. Run without
concurrent CLI tests or tab creation. New PNG evidence remains in --output-dir.
"""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import struct
import sys
import threading
import time
from urllib.parse import urlsplit
import uuid
import zlib

from smoke import BrowserCli, CheckFailed, ROOT, element, require


COLORS = {"a": (32, 96, 160), "b": (160, 96, 32), "popup-a": (48, 144, 80)}


def poll(read, ready, message):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        value = read()
        if ready(value):
            return value
        time.sleep(0.025)
    raise CheckFailed(message)


def first_png_pixel(path):
    """Read only the first RGB(A) pixel, with bounded decompression and file reads."""
    size = path.stat().st_size
    with path.open("rb") as source:
        require(source.read(8) == b"\x89PNG\r\n\x1a\n", "Screenshot is not a PNG")
        decoder = zlib.decompressobj()
        sample = b""
        channels = None
        dimensions = None
        while source.tell() < size:
            header = source.read(8)
            require(len(header) == 8, "PNG chunk header is incomplete")
            length, kind = struct.unpack(">I4s", header)
            require(length <= size - source.tell() - 4, "PNG chunk exceeds the file size")
            if kind == b"IHDR":
                require(dimensions is None and length == 13, "PNG has an invalid image header")
                width, height, depth, color, compression, filters, interlace = struct.unpack(">IIBBBBB", source.read(13))
                require(width > 0 and height > 0 and depth == 8 and color in (2, 6) and
                        compression == 0 and filters == 0 and interlace == 0,
                        "Screenshot uses an unsupported PNG encoding for the pixel check")
                channels = 3 if color == 2 else 4
                dimensions = {"width": width, "height": height}
            elif kind == b"IDAT":
                require(channels is not None, "PNG image data precedes its header")
                remaining = length
                while remaining:
                    block = source.read(min(4096, remaining))
                    require(block, "PNG image data is incomplete")
                    remaining -= len(block)
                    sample += decoder.decompress(block, 1 + channels - len(sample))
                    if len(sample) == 1 + channels:
                        # On row zero/pixel zero, left/up/upper-left predictors
                        # are all zero, so every PNG filter leaves this pixel unchanged.
                        require(sample[0] <= 4, "PNG has an invalid row filter")
                        if channels == 4:
                            require(sample[4] == 255, "Fixture screenshot pixel is not opaque")
                        return tuple(sample[1:4]), dimensions
            else:
                source.seek(length, 1)
            require(len(source.read(4)) == 4, "PNG chunk checksum is incomplete")
    raise CheckFailed("PNG has no complete first pixel")


def fixture(name, prefix):
    red, green, blue = COLORS[name]
    link = f'<a href="/{prefix}/popup-a" target="_blank">Open A popup</a>' if name == "a" else ""
    return f'''<!doctype html><html lang="en"><head><meta charset="utf-8">
<title>Single window fixture {name}</title><style>
html,body{{margin:0;min-height:100vh;background:rgb({red},{green},{blue});color:white}}
main{{padding:32px;font:18px system-ui}}a{{color:white}}input{{font:inherit}}
</style></head><body><main><h1>Single window fixture {name}</h1>
<p>Content marker {prefix} {name}</p><label>Fixture input <input value="preserve-{name}"></label>
{link}</main></body></html>'''.encode("utf-8")


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {
        "ok": False,
        "scope": "real CEF local tabs and CLI single-window metadata; native window count and stop UI not inspected",
        "checks": [],
        "screenshots": {},
    }
    initial = set()
    owned = set()
    tasks = {}
    server = server_thread = None
    prefix = uuid.uuid4().hex

    def call(*operation, expected=None):
        response = cli.request(*operation)
        if expected:
            require(not response["ok"] and response.get("error", {}).get("code") == expected,
                    f"Expected {expected}, received {json.dumps(response, ensure_ascii=False)}")
            return response
        require(response["ok"], f"CLI request failed: {json.dumps(response, ensure_ascii=False)}")
        return response["result"]

    def command(task, *operation, expected=None):
        return call("task", "run", task["id"], task["revision"], *operation, expected=expected)

    def info(task):
        return call("task", "info", task["id"])["task"]

    def state():
        status = call("status")
        tabs = call("tabs")["tabs"]
        require(status.get("windowMode") == "single", "Expected status.windowMode=single")
        require(all(isinstance(tab.get("active"), bool) for tab in tabs), "tabs.active is missing or not boolean")
        return {"status": status, "tabs": tabs}

    def consistent(expected_active=None):
        def ready(value):
            active = [tab["tab"] for tab in value["tabs"] if tab["active"]]
            return (len(active) == 1 and active[0] == value["status"].get("activeTab") and
                    value["status"]["tabs"] == len(value["tabs"]) and
                    (expected_active is None or active[0] == expected_active))
        return poll(state, ready, "status.activeTab and tabs.active did not converge on the expected tab")

    def tab_ids():
        return {tab["tab"] for tab in call("tabs")["tabs"]}

    def open_tab(task, name):
        tab = command(task, "open", origin + "/" + name)["tab"]
        require(type(tab) is int and tab not in initial and tab not in owned, "Open reused a tab ID")
        owned.add(tab)
        command(task, "wait", tab, "load", 5000)
        # Opening an AI tab keeps the user's current space and selection.
        # The explicit UI command selects the task's remembered tab.
        call("space", "show", task["id"])
        consistent(tab)
        return tab

    def read_page(task, tab, name):
        page = command(task, "read", tab)["page"]
        require(page.get("title") == f"Single window fixture {name}" and
                f"Content marker {prefix} {name}" in page.get("text", "") and
                page.get("url") == origin + "/" + name,
                f"Read returned the wrong page for fixture {name}")
        return page

    def capture(task, tab, name, active_tab):
        screenshot = output / f"single-window-{prefix}-{name}.png"
        require(not screenshot.exists(), "Screenshot evidence path already exists")
        captured = command(task, "screenshot", tab, screenshot)
        evidence = {"path": str(screenshot), "bytes": captured.get("bytes")}
        report["screenshots"][name] = evidence
        require(captured.get("tab") == tab and captured.get("path") == str(screenshot) and
                captured.get("bytes") == screenshot.stat().st_size and screenshot.stat().st_size > 100,
                f"Capture metadata for {name} does not match its new PNG file")
        pixel, dimensions = first_png_pixel(screenshot)
        evidence.update(firstPixel=pixel, **dimensions)
        consistent(active_tab)
        return evidence

    def finish(task):
        result = call("task", "finish", task["id"], task["revision"])
        tasks[task["id"]] = result["task"]
        current = poll(lambda: info(task), lambda value: value["status"] == "finished", "Task did not finish after tab close callbacks")
        tasks[task["id"]] = current
        return result

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            path = urlsplit(self.path).path
            name = path.removeprefix("/" + prefix + "/")
            if path != f"/{prefix}/{name}" or name not in COLORS:
                self.send_response(204)
                self.end_headers()
                return
            body = fixture(name, prefix)
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
        baseline = consistent()
        require(baseline["status"]["engine"] == "CEF/Chromium", "Expected a running real CEF app")
        initial = {tab["tab"] for tab in baseline["tabs"]}
        require(1 <= len(initial) <= 5,
                f"Single-window smoke requires 1..5 existing tabs for two fixture tabs; found {len(initial)}")
        require(baseline["status"]["resources"]["pendingTabCreations"] == 0 and
                baseline["status"]["resources"]["capturesInFlight"] == 0,
                "Run without other pending tab creation or capture")
        report["initialTabs"] = sorted(initial)
        report["checks"].append("single-window metadata agrees on exactly one active CEF tab")

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        origin = f"http://127.0.0.1:{server.server_port}/{prefix}"
        output = args.output_dir.resolve()
        output.mkdir(parents=True, exist_ok=True)
        for name in ("Single window A", "Single window B"):
            task = call("task", "create", name)["task"]
            tasks[task["id"]] = task
        a, b = list(tasks.values())
        tab_a = open_tab(a, "a")
        read_page(a, tab_a, "a")
        foreground_a = capture(a, tab_a, "a-foreground", tab_a)
        tab_b = open_tab(b, "b")
        require(tab_ids() == initial | {tab_a, tab_b}, "Open changed an unrelated tab")
        read_page(b, tab_b, "b")
        foreground_b = capture(b, tab_b, "b-foreground", tab_b)
        require(max(abs(left - right) for left, right in zip(foreground_a["firstPixel"], foreground_b["firstPixel"])) >= 32,
                "Foreground fixture captures are not visually distinct enough to identify the background target")
        page_a = read_page(a, tab_a, "a")
        require(element(page_a, "Fixture input").get("value") == "preserve-a", "Background input state changed")
        consistent(tab_b)
        report["checks"].append("explicit space show selects each task tab; background read keeps the selected tab and correct page identity")

        background_a = capture(a, tab_a, "a-background", tab_b)
        # Compare the actual same-run output, without assuming CSS RGB bytes
        # survive the browser's capture/color pipeline unchanged.
        require(all(abs(actual - expected) <= 2 for actual, expected in
                    zip(background_a["firstPixel"], foreground_a["firstPixel"])),
                f"Background A pixel {background_a['firstPixel']} differs from foreground A {foreground_a['firstPixel']}")
        report["screenshot"] = background_a
        report["captureComparison"] = {
            "baseline": "a-foreground", "target": "a-background", "distinctFixture": "b-foreground",
            "perChannelTolerance": 2, "minimumForegroundChannelDifference": 32,
        }
        report["checks"].append("background A screenshot matches its foreground baseline and differs from foreground B while B stays active")

        before_panel = tab_ids()
        call("panel")
        require(tab_ids() == before_panel, "Panel compatibility command created an unexpected CEF tab")
        consistent(tab_b)
        report["checks"].append("panel compatibility command succeeds without adding a CEF tab or changing the selected tab")

        finished = finish(a)
        require(set(finished["closeRequested"]) == {tab_a}, "Task finish closed more than its remaining created tab")
        require(tab_ids() == initial | {tab_b}, "Task A finish removed Task B or an initial tab")
        read_page(b, tab_b, "b")
        consistent()
        report["checks"].append("finishing Task A closes its created tab while Task B and initial user tabs remain usable")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, ValueError, zlib.error) as error:
        report["error"] = str(error)
    finally:
        cleanup = []
        for task in list(tasks.values()):
            try:
                current = info(task)
                if current["status"] != "finished":
                    finish(current)
            except (CheckFailed, OSError, KeyError, TypeError) as error:
                cleanup.append(str(error))
        # Also remove a fixture popup if incorrect ownership left it unscoped.
        if owned:
            try:
                remaining = owned & tab_ids()
                for tab in sorted(remaining):
                    require(tab not in initial, "Refusing to close an initial user tab")
                    call("close", tab)
                final = poll(tab_ids, lambda tabs: not (tabs & owned), "Test-owned tabs did not close")
                require(initial <= final, "An initial user tab disappeared")
                final_state = consistent()
                report["finalResources"] = final_state["status"]["resources"]
                require(report["finalResources"]["pendingTabCreations"] == 0 and
                        report["finalResources"]["capturesInFlight"] == 0,
                        "Cleanup left a creation reservation or screenshot capture")
                report["checks"].append("test tasks and tabs cleaned up; initial tabs and a consistent active-tab state preserved")
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
    report["checkCount"] = len(report["checks"])
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, default=ROOT / "target/release/lightbrowser")
    parser.add_argument("--data-dir", type=Path, default=os.environ.get("LIGHT_BROWSER_DATA_DIR"))
    parser.add_argument("--output-dir", type=Path, default=ROOT / "artifacts/single-window")
    args = parser.parse_args()
    if args.data_dir is None:
        parser.error("--data-dir or LIGHT_BROWSER_DATA_DIR is required")
    report = run(args)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
