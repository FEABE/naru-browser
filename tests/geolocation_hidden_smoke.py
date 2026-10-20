#!/usr/bin/env python3
"""Count geolocation permission requests of a hidden AI tab through a running app.

Example:
  python3 tests/geolocation_hidden_smoke.py --cli target/release/lightbrowser \
      --data-dir "$LIGHT_BROWSER_DATA_DIR" [--out report.json]

Does not launch the app. Uses the localhost fixture tests/fixtures/site-permissions.html
only; no coordinates are read. Native card buttons cannot be pressed from here, so
the card is cancelled through a control change (task handoff, IGNORE), which kills the
Blink-side request in the same way as a hidden-page dismissal. See docs/site-permissions.md.
"""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading
import time

from smoke import BrowserCli, CheckFailed, ROOT, element, require

FIXTURE = ROOT / "tests/fixtures/site-permissions.html"


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/favicon"):
            self.send_response(204)
            self.end_headers()
            return
        body = FIXTURE.read_bytes()
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
    log = []
    report = {"ok": False, "log": log}
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    origin = f"http://127.0.0.1:{server.server_address[1]}"
    task = tab = None

    def note(step, **data):
        log.append({"step": step, **data})

    def status():
        return cli.success("status")

    def pending():
        return status()["sitePermissions"]["requests"]

    def wait_pending(count, seconds=6):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            rows = pending()
            if len(rows) == count:
                return rows
            time.sleep(0.1)
        return pending()

    try:
        initial = status()
        user_space = initial["activeSpace"]
        require(user_space == "user", "the user space must be shown at start")
        task = cli.success("task", "create", "m5-geo")["task"]

        def run_in(*command):
            return cli.request("task", "run", task["id"], task["revision"], *command)

        opened = run_in("open", origin + "/")
        require(opened["ok"], f"open: {opened}")
        tab = opened["result"]["tab"]
        require(run_in("wait", tab, "load", 10000)["ok"], "load")
        page = run_in("read", tab)["result"]["page"]
        if args.variant == "hidden-from-start":
            clicked = run_in("click", tab, element(page, "위치 권한 요청")["ref"])
            require(clicked["ok"], f"click: {clicked}")
            rows = wait_pending(1)
            note("hidden-from-start", pending=len(rows))
            require(not rows or True, "")
        else:
            cli.success("space", "show", task["id"])
            time.sleep(0.5)
            page = run_in("read", tab)["result"]["page"]
            clicked = run_in("click", tab, element(page, "위치 권한 요청")["ref"])
            require(clicked["ok"], f"click: {clicked}")
            rows = wait_pending(1)
            note("visible-request", pending=len(rows))
            cli.success("space", "show", "user")
            time.sleep(0.5)
            rows = pending()
        note("hidden-request", pending=len(rows), activeSpace=status()["activeSpace"])
        if not rows: note("page", text=cli.success("read", tab)["page"]["text"][:400]) if False else None
        require(len(rows) == 1, "the hidden AI tab should have one pending card")
        first_token = rows[0]["token"]
        require(status()["activeSpace"] == user_space, "AI work changed the user's space")

        # Hidden for a while: the page must not produce additional requests by itself.
        time.sleep(args.idle)
        rows = pending()
        note("hidden-idle", pending=len(rows), tokens=[r["token"] for r in rows])
        require([r["token"] for r in rows] == [first_token], "hidden tab produced extra requests")

        # Cancel the card (IGNORE) without changing ownership: CLI stop clears
        # pending site permissions of the tab (the same cancel_tab as a user
        # cancel at the callback level, but IGNORE instead of DISMISS).
        stopped = run_in("stop", tab)
        require(stopped["ok"], f"stop: {stopped}")
        rows = wait_pending(0)
        note("after-cancel", pending=len(rows), owner=cli.success("task", "info", task["id"])["task"]["status"])
        require(not rows, "the card should be gone after stop")
        time.sleep(0.5)
        text = run_in("read", tab)["result"]["page"]["text"]
        note("receipt-while-hidden", receipt=text[text.find("["):][:200], activeSpace=status()["activeSpace"])

        # Show the AI space (the page becomes visible) and count requests.
        cli.success("space", "show", task["id"])
        seen = set()
        deadline = time.monotonic() + args.watch
        while time.monotonic() < deadline:
            for row in pending():
                seen.add(row["token"])
            time.sleep(0.1)
        note("after-visible", newTokens=sorted(seen))
        text = run_in("read", tab)["result"]["page"]["text"]
        note("page-receipt", receipt=text[text.find("["):][:300])
        cli.success("space", "show", "user")
        rows = wait_pending(0, 2)
        note("back-to-user", pending=len(rows))
        report["rerequests"] = len(seen)
        require(len(seen) == 0 or not args.expect_fixed,
                f"hidden-cancelled location call was asked again: {sorted(seen)}")
        require("error-code-1" in text or not args.expect_fixed,
                "the cancelled location call should end with error-code-1")

        # Control: a cancel while the tab is VISIBLE must not suppress a later,
        # deliberate request of the same page.
        cli.success("space", "show", task["id"])
        time.sleep(0.5)
        page = run_in("read", tab)["result"]["page"]
        require(run_in("click", tab, element(page, "위치 권한 요청")["ref"])["ok"], "click 2")
        rows = wait_pending(1)
        note("visible-request-2", pending=len(rows))
        require(len(rows) == 1, "a visible request must still show its card")
        require(run_in("stop", tab)["ok"], "stop 2")
        wait_pending(0)
        time.sleep(0.5)
        page = run_in("read", tab)["result"]["page"]
        require(run_in("click", tab, element(page, "위치 권한 요청")["ref"])["ok"], "click 3")
        rows = wait_pending(1)
        note("visible-retry-after-visible-cancel", pending=len(rows))
        require(len(rows) == 1 or not args.expect_fixed,
                "a deliberate retry after a visible cancel was wrongly dismissed")
        run_in("stop", tab)
        cli.success("space", "show", "user")
        report["ok"] = True
    finally:
        try:
            if tab is not None:
                cli.request("close", tab)
            if task is not None:
                info = cli.request("task", "info", task["id"])
                if info.get("ok"):
                    cli.request("task", "finish", task["id"], info["result"]["task"]["revision"])
        finally:
            server.shutdown()
    return report


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cli", type=lambda p: __import__("pathlib").Path(p), required=True)
    parser.add_argument("--data-dir", type=lambda p: __import__("pathlib").Path(p), required=True)
    parser.add_argument("--out")
    parser.add_argument("--variant", choices=("hidden-from-start", "visible-then-hidden"), default="visible-then-hidden")
    parser.add_argument("--expect-fixed", action="store_true",
                        help="fail when the hidden-cancelled call is asked again")
    parser.add_argument("--idle", type=float, default=3)
    parser.add_argument("--watch", type=float, default=4)
    args = parser.parse_args()
    try:
        report = run(args)
        code = 0
    except CheckFailed as error:
        report = {"ok": False, "error": str(error)}
        code = 1
    text = json.dumps(report, ensure_ascii=False, indent=2)
    if args.out:
        open(args.out, "w").write(text + "\n")
    print(text)
    raise SystemExit(code)


if __name__ == "__main__":
    main()
