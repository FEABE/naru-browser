#!/usr/bin/env python3
"""Journey 08 frame checks against an already running Naru app (local CEF integration).

Serves tests/fixtures/frame_*.html on 127.0.0.1 and reaches the second site as
localhost on the same port, so root and child are different sites. Uses one owned
task; the user's space and selected tab are recorded before and after.
Refuses to run unless LIGHT_BROWSER_DATA_DIR points at a .local-data/e2e-* directory
and NARU_MOCK_KEYCHAIN=1, so the real profile is never touched.

  python3 tests/frame_journey_smoke.py --cli PATH [--serve-only PORT]
"""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import threading
import time
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "tests" / "fixtures"


def guard_environment():
    data_dir = os.environ.get("LIGHT_BROWSER_DATA_DIR", "")
    resolved = Path(data_dir).resolve() if data_dir else None
    if (resolved is None or resolved.parent != (ROOT / ".local-data").resolve()
            or not resolved.name.startswith("e2e-") or os.environ.get("NARU_MOCK_KEYCHAIN") != "1"):
        sys.exit("refusing to run: set LIGHT_BROWSER_DATA_DIR=<repo>/.local-data/e2e-* and NARU_MOCK_KEYCHAIN=1")
    return resolved


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        path = urlsplit(self.path).path
        port = self.server.server_port
        host = (self.headers.get("Host") or "").split(":")[0]
        cross = f"http://localhost:{port}" if host == "127.0.0.1" else f"http://127.0.0.1:{port}"
        if path == "/slow":
            # Body has started but the document never finishes within the read deadline.
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.end_headers()
            try:
                self.wfile.write(b"<!doctype html><html><body><p>SLOW_PARTIAL_MARK</p><input aria-label='Slow input'>" + b" " * 4096)
                self.wfile.flush()
                time.sleep(25)
                self.wfile.write(b"</body></html>")
            except (BrokenPipeError, ConnectionResetError):
                pass
            return
        match = re.fullmatch(r"/f/([a-z]+)\.html", path)
        file = FIXTURES / f"frame_{match.group(1)}.html" if match else None
        if file is None or not file.is_file():
            self.send_response(404)
            self.end_headers()
            return
        # Same host on the paired port: cross-origin but same-site, so Chromium keeps it in-process.
        xo = f"http://{host}:{self.server.pair_port}"
        body = file.read_text("utf-8").replace("{{CROSS}}", cross).replace("{{XO}}", xo).encode("utf-8")
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


def start_server(port=0):
    """Start a primary server and a paired one (same host, other port); return both."""
    servers = [ThreadingHTTPServer(("127.0.0.1", port), Handler)]
    servers.append(ThreadingHTTPServer(("127.0.0.1", port + 1 if port else 0), Handler))
    servers[0].pair_port, servers[1].pair_port = servers[1].server_port, servers[0].server_port
    for server in servers:
        server.daemon_threads = True
        threading.Thread(target=server.serve_forever, daemon=True).start()
    return servers


class CheckFailed(RuntimeError):
    pass


class Cli:
    def __init__(self, executable):
        self.executable = str(executable)
        self.log = []

    def __call__(self, *args, timeout=60):
        process = subprocess.run([self.executable, *map(str, args)], capture_output=True, text=True,
                                 timeout=timeout, check=False)
        response = json.loads(process.stdout)
        self.log.append({"argv": list(map(str, args)), "ok": response.get("ok"),
                         "code": (response.get("error") or {}).get("code")})
        return response


def state(text, name):
    match = re.search(r"STATE_" + re.escape(name) + r" (\{.*?\})(?=\s|$)", text)
    if not match:
        raise CheckFailed(f"state receipt {name} missing")
    return json.loads(match.group(1))


def item(page, label):
    found = [i for i in page["interactive"] if i.get("label") == label]
    if len(found) != 1:
        raise CheckFailed(f"expected one element {label!r}, found {len(found)}")
    return found[0]


def run(cli, origin, report):
    """Main-frame navigation/reload is avoided: on this build it crashed the app once OOPIF
    child sessions were tracked (see artifacts/e2e-v15/j08). Each scenario uses its own tab."""
    def check(name, condition, detail=None):
        report["checks"].append({"check": name, "ok": bool(condition), "detail": detail})
        if not condition:
            raise CheckFailed(f"{name}: {json.dumps(detail, ensure_ascii=False)[:400]}")

    def ok(response):
        if not response.get("ok"):
            raise CheckFailed(json.dumps(response, ensure_ascii=False)[:600])
        return response["result"]

    def code(response):
        return "ok" if response.get("ok") else response["error"]["code"]

    before = ok(cli("spaces"))
    task = ok(cli("task", "create", "frame journey smoke"))["task"]
    tid, rev = task["id"], task["revision"]

    def run_task(*args):
        return cli("task", "run", tid, rev, *args)

    def scenario(query, want):
        """Open a fresh tab, wait until frames named in `want` exist, return (tab, {name: frame})."""
        tab = ok(run_task("open", origin + "/f/root.html" + query))["tab"]
        ok(run_task("wait", tab, "load", 15000))
        by_name = {}
        for _ in range(40):
            response = run_task("frames", tab)
            if response["ok"]:
                by_name = {(re.search(r"[?&]name=([A-Z]+)", f["url"]) or [None, "ROOT"])[1]: f for f in response["result"]["frames"]}
                if want <= set(by_name):
                    return tab, by_name
            time.sleep(0.25)
        ok(run_task("close", tab))
        raise CheckFailed(f"frames never listed {sorted(want - set(by_name))}")

    def read(tab, frame):
        return ok(run_task("read-frame", tab, frame["frameId"]))["page"]

    def root_click(tab, label):
        page = ok(run_task("read", tab))["page"]
        ok(run_task("click", tab, item(page, label)["ref"]))
        time.sleep(0.4)

    tabs = []
    try:
        tab, f = scenario("", {"ROOT", "SAME", "SAMEN", "XO", "XON", "CROSS", "CROSSN"})
        tabs.append(tab)
        check("only the cross-site frames use a child session",
              all(bool(f[n]["sessionId"]) == n.startswith("CROSS") for n in f), {n: f[n]["sessionId"] for n in f})
        again = ok(run_task("frames", tab))["frames"]
        check("second frames call lists the same frames", sorted(x["frameId"] for x in again) == sorted(x["frameId"] for x in f.values()))

        for name in ("SAME", "XO", "CROSS", "CROSSN"):
            result = ok(run_task("read-frame", tab, f[name]["frameId"]))
            page = result["page"]
            marks = set(re.findall(r"[A-Z]+_(?:DOC|SHADOW)_MARK", page["text"]))
            check(f"{name} read-frame returns only its document and open shadow", marks == {f"{name}_DOC_MARK", f"{name}_SHADOW_MARK"}, sorted(marks))
            check(f"{name} read-frame contract", result["atomic"] is False and result["pointerActionsSupported"] is True
                  and len(result["supportedActions"]) == 7 and page["scope"] == "local-frame"
                  and (page.get("focusedRef") is None or page["focusedRef"] in {i["ref"] for i in page["interactive"]}))
            first_ref = item(page, f"{name} other")["ref"]
            codes = [code(run_task("fill", tab, item(page, f"{name} text")["ref"], f"fill-{name}")),
                     code(run_task("insert", tab, item(page, f"{name} area")["ref"], "+ins")),
                     code(run_task("press", tab, item(page, f"{name} text")["ref"], "End")),
                     code(run_task("check", tab, item(page, f"{name} check")["ref"], "true")),
                     code(run_task("select", tab, item(page, f"{name} choice")["ref"], "b")),
                     code(run_task("hover", tab, item(page, f"{name} button")["ref"]))]
            check(f"{name} fill/insert/press/check/select/hover responses", codes == ["ok"] * 6, codes)
            page = read(tab, f[name])
            hovered = state(page["text"], name)
            check(f"{name} hover moved without press or click", hovered["move"] >= 1 and hovered["down"] == hovered["up"] == hovered["click"] == 0, hovered)
            check(f"{name} stale ref after a new read-frame", code(run_task("fill", tab, first_ref, "MUST_NOT")) == "STALE_FRAME_SNAPSHOT")
            values = {k: item(page, f"{name} {k}") for k in ("text", "area", "check", "choice")}
            check(f"{name} DOM state after inputs", values["text"].get("value") == f"fill-{name}" and values["area"].get("value") == "+insarea-seed"
                  and values["check"].get("checked") is True and values["choice"].get("value") == "b" and "End@text" in hovered["keys"],
                  {k: v.get("value") for k, v in values.items()})
            ok(run_task("click", tab, item(page, f"{name} button")["ref"]))
            page = read(tab, f[name])
            clicked = state(page["text"], name)
            check(f"{name} click reached the page exactly once", clicked["click"] == clicked["down"] == clicked["up"] == 1, clicked)
            ref = item(page, f"{name} other")["ref"]
            head, epoch, token, rest = ref.split("/", 3)
            for forged in (f"{head}/{int(epoch) + 1}/{token}/{rest}", f"{head}/{epoch}/{int(token) + 1}/{rest}", f"{head}/{epoch}/{token}/0/{rest}", rest):
                check(f"{name} forged ref rejected", code(run_task("fill", tab, forged, "MUST_NOT")) in ("STALE_FRAME_SNAPSHOT", "STALE_REF"), forged)
            check(f"{name} genuine ref still valid after forged attempts", code(run_task("fill", tab, ref, "genuine")) == "ok")
        ok(run_task("close", tab))
        tabs.remove(tab)

        tab, f = scenario("?same=&xo=", {"CROSS"})
        tabs.append(tab)
        refs = {i["label"]: i["ref"] for i in read(tab, f["CROSS"])["interactive"]}
        root_click(tab, "inert cross")
        check("inert parent rejects fill before dispatch", code(run_task("fill", tab, refs["CROSS text"], "MUST_NOT")) == "FRAME_OWNER_UNAVAILABLE")
        refs = {i["label"]: i["ref"] for i in read(tab, f["CROSS"])["interactive"]}
        check("inert parent rejects click before dispatch", code(run_task("click", tab, refs["CROSS button"])) == "FRAME_OWNER_UNAVAILABLE")
        root_click(tab, "restore cross")
        root_click(tab, "cover cross")
        refs = {i["label"]: i["ref"] for i in read(tab, f["CROSS"])["interactive"]}
        check("covered frame rejects click", code(run_task("click", tab, refs["CROSS button"])) == "CLICK_POINT_UNAVAILABLE")
        page = read(tab, f["CROSS"])
        untouched = state(page["text"], "CROSS")
        check("no input reached the frame while inert or covered", item(page, "CROSS text").get("value") == "seed-value"
              and untouched["move"] == untouched["down"] == untouched["click"] == 0, untouched)
        ok(run_task("close", tab))
        tabs.remove(tab)

        tab, f = scenario("?same=&xo=&cross=name%3DMM%26mm%3Dhide", {"MM"})
        tabs.append(tab)
        page = read(tab, f["MM"])
        response = run_task("click", tab, item(page, "MM button")["ref"])
        moved = state(read(tab, f["MM"])["text"], "MM")
        check("target hidden by mousemove: no button press", code(response) != "ok" and moved["move"] == 1 and moved["down"] == moved["click"] == 0,
              {"code": code(response), "state": moved})
        ok(run_task("close", tab))
        tabs.remove(tab)
    finally:
        for tab in tabs:
            cli("task", "run", tid, rev, "close", tab)
        info = ok(cli("task", "info", tid))["task"]
        if info["status"] != "finished":
            ok(cli("task", "finish", tid, info["revision"]))
        after = ok(cli("spaces"))
        report["user"] = {"before": [before["activeSpace"], before["activeTab"]], "after": [after["activeSpace"], after["activeTab"]]}
        check("user space and selected tab preserved", report["user"]["before"] == report["user"]["after"], report["user"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, default=ROOT / "dist/Light Browser.app/Contents/MacOS/lightbrowser")
    parser.add_argument("--serve-only", type=int, metavar="PORT")
    args = parser.parse_args()
    guard_environment()
    servers = start_server(args.serve_only or 0)
    server = servers[0]
    if args.serve_only:
        print(f"serving http://127.0.0.1:{server.server_port} (cross-site: localhost, same-site cross-origin: port {server.pair_port})", flush=True)
        threading.Event().wait()
    report = {"ok": False, "checks": []}
    cli = Cli(args.cli)
    try:
        run(cli, f"http://127.0.0.1:{server.server_port}", report)
        report["ok"] = True
    except (CheckFailed, KeyError, TypeError, json.JSONDecodeError, subprocess.TimeoutExpired) as error:
        report["error"] = str(error)
    finally:
        for each in servers:
            each.shutdown()
        report["commands"] = cli.log
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
