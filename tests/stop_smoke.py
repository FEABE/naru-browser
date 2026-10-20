#!/usr/bin/env python3
"""Stop the first load of a tab through a running app's CLI, then navigate again.

Example:
  python3 tests/stop_smoke.py --cli target/release/lightbrowser --data-dir "$LIGHT_BROWSER_DATA_DIR"

Does not launch the app. A localhost server answers /slow after 8 seconds (/body
sends headers and a partial body, then stalls). For `open` right before `stop`, a
`navigate` on the initial tab and a task-owned tab, the run checks that stop is
accepted, that the tab stops loading and that a following navigate is not
NAVIGATION_BUSY. Only the CLI path is covered: the native toolbar stop button is
not driven here (see docs/reload-stop.md).
"""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import threading
import time

from smoke import BrowserCli, CheckFailed, require


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        try:
            if self.path.startswith("/favicon"):
                self.send_response(204)
                self.end_headers()
                return
            if self.path.startswith("/body"):
                self.send_response(200)
                self.send_header("Content-Type", "text/html")
                self.send_header("Content-Length", "200")
                self.end_headers()
                self.wfile.write(b"<!doctype html><title>b</title>")
                self.wfile.flush()
                time.sleep(8)
                return
            if self.path.startswith("/slow"):
                time.sleep(8)
            body = b"<!doctype html><title>fast</title><p>done</p>"
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def log_message(self, *_args):
        pass


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    origin = f"http://127.0.0.1:{server.server_address[1]}"
    checks = []
    task = None
    opened = []

    def state(tab):
        return next(t for t in cli.success("tabs")["tabs"] if t["tab"] == tab)

    def stop_then_navigate(label, issue):
        tab = issue("stop-check")
        started = time.monotonic()
        stopped = issue("stop", tab)
        require(stopped["ok"], f"{label}: stop {stopped}")
        deadline = time.monotonic() + 3
        while state(tab)["loading"] and time.monotonic() < deadline:
            time.sleep(0.05)
        tab_state = state(tab)
        require(not tab_state["loading"] and not tab_state["navigation"]["provisional"],
                f"{label}: still loading after stop: {tab_state['navigation']}")
        require(tab_state["navigation"]["stopped"] is True, f"{label}: stopped flag missing")
        navigated = issue("navigate", tab, origin + "/fast")
        require(navigated["ok"], f"{label}: navigate after stop: {json.dumps(navigated)[:300]}")
        require(time.monotonic() - started < 6, f"{label}: took as long as the slow response")
        checks.append(label)
        return tab

    try:
        def plain(path):
            def issue(kind, *rest):
                if kind == "stop-check":
                    tab = cli.success("open", origin + path)["tab"]
                    opened.append(tab)
                    return tab
                return cli.request(kind, *rest)
            return issue

        stop_then_navigate("open then immediate stop", plain("/slow"))
        stop_then_navigate("open, partial body, stop", plain("/body"))

        task = cli.success("task", "create", "stop-smoke")["task"]

        def in_task(kind, *rest):
            if kind == "stop-check":
                response = cli.request("task", "run", task["id"], task["revision"], "open", origin + "/slow")
                require(response["ok"], f"task open: {response}")
                opened.append(response["result"]["tab"])
                return response["result"]["tab"]
            return cli.request("task", "run", task["id"], task["revision"], kind, *rest)

        stop_then_navigate("task-owned open then immediate stop", in_task)
        report = {"ok": True, "checks": checks}
    finally:
        for tab in opened:
            cli.request("close", tab)
        if task is not None:
            info = cli.request("task", "info", task["id"])
            if info.get("ok"):
                cli.request("task", "finish", task["id"], info["result"]["task"]["revision"])
        server.shutdown()
    return report


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--data-dir", type=Path, required=True)
    parser.add_argument("--out")
    args = parser.parse_args()
    try:
        report = run(args)
        code = 0
    except CheckFailed as error:
        report = {"ok": False, "error": str(error)}
        code = 1
    text = json.dumps(report, ensure_ascii=False, indent=2)
    if args.out:
        Path(args.out).write_text(text + "\n")
    print(text)
    raise SystemExit(code)


if __name__ == "__main__":
    main()
