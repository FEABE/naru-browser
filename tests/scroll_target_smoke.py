#!/usr/bin/env python3
"""Targeted DOM scroll journeys against an already running, isolated Naru build.

Requires LIGHT_BROWSER_DATA_DIR=<repo>/.local-data/e2e-* and NARU_MOCK_KEYCHAIN=1.
Uses only loopback HTTP, one owned task, and two disposable stdlib servers.
It never launches Naru, navigates existing tabs, or changes the selected space.
Cross-site localhost must actually report a child session; an ordinary iframe
does not count as OOPIF coverage. Failures retain their phase in the JSON report.

  python3 tests/scroll_target_smoke.py --cli "dist/Light Browser.app/Contents/MacOS/lightbrowser"

This is an integration runner, not evidence that a build has passed it.
"""

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import re
import sys
import threading
import time
from urllib.parse import parse_qs, urlsplit

from frame_journey_smoke import CheckFailed, Cli, ROOT, guard_environment

FIXTURE = ROOT / "tests/fixtures/scroll-target.html"


def ok(response):
    if not response.get("ok"):
        raise CheckFailed(json.dumps(response, ensure_ascii=False)[:800])
    return response["result"]


def code(response):
    return "ok" if response.get("ok") else response["error"]["code"]


def unique(page, label):
    found = [item for item in page["interactive"] if item.get("label") == label]
    if len(found) != 1:
        raise CheckFailed(f"expected one {label!r}, found {len(found)}")
    return found[0]


def document_item(page):
    found = [item for item in page["interactive"]
             if item.get("scroll", {}).get("target") == "document" and "frame" not in item]
    if len(found) != 1:
        raise CheckFailed(f"expected one local document ref, found {len(found)}")
    return found[0]


def focused_label(page):
    reference = page.get("focusedRef")
    return next((item.get("label") for item in page["interactive"] if item["ref"] == reference), None)


def positions(page):
    return {(item.get("frame"), item["label"]): item["scroll"]["position"]
            for item in page["interactive"] if "scroll" in item}


def close_position(left, right):
    return all(abs(left[axis] - right[axis]) <= 1 for axis in ("x", "y"))


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for part in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(part)
    return value.hexdigest()


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if urlsplit(self.path).path != "/scroll-target.html":
            self.send_error(404)
            return
        body = FIXTURE.read_text("utf-8")
        for key, value in self.server.origins.items():
            body = body.replace("{{" + key + "}}", value)
        data = body.encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def log_message(self, *_args):
        pass


def run(cli, origin, report):
    task = None
    before = None
    initial_tabs = set()
    owned = set()

    def check(name, condition, detail=None):
        report["checks"].append({"check": name, "ok": bool(condition), "detail": detail})
        if not condition:
            raise CheckFailed(name)

    def scoped(*args):
        return cli("task", "run", task["id"], task["revision"], *args)

    def selection():
        state = ok(cli("spaces"))
        return [state["activeSpace"], state["activeTab"], state["overview"]]

    def preserved():
        check("current user space, selected tab and overview preserved", selection() == before)

    try:
        report["phase"] = "setup"
        status = ok(cli("status"))
        check("running app is CEF and user space is visible",
              status.get("engine") == "CEF/Chromium"
              and status["activeSpace"] == "user" and not status["overview"])
        before = selection()
        initial_tabs = {item["tab"] for item in ok(cli("tabs"))["tabs"]}
        task = ok(cli("task", "create", "Targeted scroll fixture"))["task"]
        tab = ok(scoped("open", origin + "/scroll-target.html?name=ROOT"))["tab"]
        owned.add(tab)
        ok(scoped("wait", tab, "load", 15000))
        frames = {}
        for _ in range(40):
            response = scoped("frames", tab)
            if response.get("ok"):
                frames = {parse_qs(urlsplit(frame["url"]).query).get("name", [""])[0]: frame
                          for frame in response["result"]["frames"]}
                if {"ROOT", "SAME", "CROSS"} <= frames.keys():
                    break
            time.sleep(0.1)
        check("three fixture documents discovered", {"ROOT", "SAME", "CROSS"} <= frames.keys())
        check("cross-site frame is an actual OOPIF",
              bool(frames["CROSS"]["sessionId"]) and not frames["SAME"]["sessionId"],
              {name: frames[name]["sessionId"] for name in ("ROOT", "SAME", "CROSS")})
        report["frames"] = {name: {"frameId": frame["frameId"], "sessionId": frame["sessionId"]}
                            for name, frame in frames.items()}

        def read(name="ROOT", local=False):
            result = ok(scoped("read-frame", tab, frames[name]["frameId"])) if local else ok(scoped("read", tab))
            if local:
                check(f"{name} advertises targeted scroll", "scroll" in result["supportedActions"])
            return result["page"]

        def exercise(name, label=None, *, local=False, dx=43, dy=61):
            report["phase"] = f"{name}: {label or 'document'}"
            # Existing key preparation sets a measurable logical DOM focus.
            # Its result is checked, but is not counted as scroll evidence.
            page = read(name, local)
            ok(scoped("press", tab, unique(page, f"{name} focus sentinel")["ref"], "End"))
            parent_before = read() if local else None
            page = read(name, local)
            target = unique(page, label) if label else document_item(page)
            metadata = target["scroll"]
            check(f"{report['phase']} has the expected scroll metadata",
                  metadata["target"] == ("element" if label else "document")
                  and metadata["axes"]["y"] and (not dx or metadata["axes"]["x"])
                  and all(key in metadata for key in ("position", "extent", "viewport")))
            check(f"{report['phase']} focus baseline", focused_label(page) == f"{name} focus sentinel")
            result = ok(scoped("scroll", tab, target["ref"], dx, dy))["page"]
            expected = {"x": metadata["position"]["x"] + dx, "y": metadata["position"]["y"] + dy}
            check(f"{report['phase']} scrolls only once by the requested delta",
                  result.get("action") == "scroll" and result.get("input") == "dom"
                  and result.get("target") == metadata["target"] and result.get("attempted") is True
                  and result.get("changed") is True
                  and close_position(result["before"], metadata["position"])
                  and close_position(result["after"], expected), result)
            after = read(name, local)
            moved = unique(after, label) if label else document_item(after)
            check(f"{report['phase']} reread observes the new position",
                  close_position(moved["scroll"]["position"], result["after"]))
            check(f"{report['phase']} preserves logical focus",
                  focused_label(after) == focused_label(page))
            old_positions, new_positions = positions(page), positions(after)
            target_key = (target.get("frame"), target["label"])
            check(f"{report['phase']} preserves other containers and document offsets",
                  all(key in new_positions and close_position(value, new_positions[key])
                      for key, value in old_positions.items() if key != target_key))
            if label:
                check(f"{report['phase']} preserves its reading document viewport",
                      close_position(page["viewport"], after["viewport"]))
            if local:
                parent_after = read()
                check(f"{report['phase']} preserves the top document and ROOT containers",
                      close_position(parent_before["viewport"], parent_after["viewport"])
                      and all(close_position(unique(parent_before, root_label)["scroll"]["position"],
                                             unique(parent_after, root_label)["scroll"]["position"])
                              for root_label in ("ROOT ancestor", "ROOT list")))
            preserved()

        exercise("ROOT", "ROOT list")
        exercise("ROOT", "ROOT shadow list")
        exercise("SAME", "SAME list")  # Same-origin child ref returned by ordinary read.
        exercise("SAME", local=True, dx=0)
        exercise("CROSS", "CROSS list", local=True)
        exercise("CROSS", local=True, dx=0)

        report["phase"] = "read-page refs"
        batch = ok(scoped("read-page", tab))
        documents = {parse_qs(urlsplit(item["frame"]["url"]).query).get("name", [""])[0]: item["page"]
                     for item in batch["documents"]}
        check("read-page returns scroll-capable refs for all three documents",
              {"ROOT", "SAME", "CROSS"} <= documents.keys()
              and "scroll" in batch["supportedActions"]
              and all("scroll" in documents[name]["supportedActions"] for name in ("ROOT", "SAME", "CROSS")))
        cross_target = unique(documents["CROSS"], "CROSS list")
        root_target = unique(documents["ROOT"], "ROOT list")
        cross_result = ok(scoped("scroll", tab, cross_target["ref"], 0, 13))["page"]
        root_result = ok(scoped("scroll", tab, root_target["ref"], 0, 17))["page"]
        check("one read-page snapshot keeps multiple document refs usable",
              close_position(cross_result["before"], cross_target["scroll"]["position"])
              and close_position(root_result["before"], root_target["scroll"]["position"])
              and abs(cross_result["after"]["y"] - cross_result["before"]["y"] - 13) <= 1
              and abs(root_result["after"]["y"] - root_result["before"]["y"] - 17) <= 1)
        preserved()

        report["phase"] = "rejected references and ownership"
        page = read()
        stale_root = unique(page, "ROOT list")["ref"]
        current = read()
        check("root ref from the preceding read is rejected",
              code(scoped("scroll", tab, stale_root, 0, 11)) == "STALE_REF")
        sentinel = unique(current, "ROOT focus sentinel")["ref"]
        check("a non-scrollable ref never falls back to an ancestor",
              code(scoped("scroll", tab, sentinel, 0, 11)) == "ELEMENT_NOT_SCROLLABLE")
        unchanged = read()
        check("rejected root actions leave scroll positions unchanged", positions(current) == positions(unchanged))
        page = read("CROSS", True)
        stale_frame = unique(page, "CROSS list")["ref"]
        current = read("CROSS", True)
        check("frame ref from the preceding read is rejected",
              code(scoped("scroll", tab, stale_frame, 0, 11)) == "STALE_FRAME_SNAPSHOT")
        current_ref = unique(current, "CROSS list")["ref"]
        old_revision = task["revision"]
        task = ok(cli("task", "handoff", task["id"], old_revision))["task"]
        check("old task revision cannot scroll",
              code(cli("task", "run", task["id"], old_revision, "scroll", tab, current_ref, 0, 11)) == "TASK_STALE_REVISION")
        check("paused task cannot scroll", code(scoped("scroll", tab, current_ref, 0, 11)) == "TASK_PAUSED")
        task = ok(cli("task", "resume", task["id"], task["revision"]))["task"]
        check("handoff invalidates the old frame ref",
              code(scoped("scroll", tab, current_ref, 0, 11)) == "STALE_FRAME_SNAPSHOT")
        check("rejected frame actions leave offsets unchanged",
              positions(read("CROSS", True)) == positions(current))
        preserved()

        report["phase"] = "changed DOM ownership and legacy scroll"
        page = read()
        moved = unique(page, "ROOT list")
        ok(scoped("click", tab, unique(page, "Move ROOT list into SAME document")["ref"]))
        response = scoped("scroll", tab, moved["ref"], 0, 11)
        check("ref whose element moved to another document is rejected",
              code(response) in ("STALE_REF", "ELEMENT_UNAVAILABLE"), code(response))
        after_move = read()
        moved_after = unique(after_move, "ROOT list")
        receipt = re.search(r"ROOT_LIST_MOVED_TO_SAME_DOCUMENT (\{[^}]+\})", after_move["text"])
        check("fixture confirms the element moved into the child", bool(receipt) and bool(moved_after.get("frame")))
        check("rejected moved ref preserves offsets sampled after DOM adoption",
              close_position(moved_after["scroll"]["position"], json.loads(receipt.group(1))))
        legacy = ok(scoped("scroll", tab, 0, 17))["page"]
        after_legacy = read()
        check("legacy scroll TAB DX DY still scrolls the main window",
              legacy.get("action") == "scroll"
              and abs(after_legacy["viewport"]["y"] - after_move["viewport"]["y"] - 17) <= 1)
        preserved()
        report["ok"] = True
    except Exception as error:
        report["error"] = str(error)
    finally:
        errors = []
        if task:
            try:
                current = ok(cli("task", "info", task["id"]))["task"]
                if current["status"] != "finished":
                    ok(cli("task", "finish", current["id"], current["revision"]))
                for _ in range(50):
                    current = ok(cli("task", "info", task["id"]))["task"]
                    if current["status"] == "finished" and not current["tabs"] and not current["closingTabs"]:
                        break
                    time.sleep(0.1)
                if current["status"] != "finished" or current["tabs"] or current["closingTabs"]:
                    errors.append("owned task tabs did not finish closing")
                existing = {item["tab"] for item in ok(cli("tabs"))["tabs"]}
                if existing & owned:
                    errors.append("owned tab remains in the browser")
                if not initial_tabs <= existing:
                    errors.append("a pre-existing tab disappeared")
            except Exception as error:
                errors.append(str(error))
        if before is not None:
            try:
                report["userSelection"] = {"before": before, "after": selection()}
                if report["userSelection"]["after"] != before:
                    errors.append("user selection changed")
            except Exception as error:
                errors.append(str(error))
        report["taskCleanupVerified"] = task is not None and not errors
        if errors:
            report.update(ok=False, cleanupErrors=errors)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, default=ROOT / "dist/Light Browser.app/Contents/MacOS/lightbrowser")
    args = parser.parse_args()
    data_dir = guard_environment()
    report = {"ok": False, "evidence": "local CEF integration", "checks": [], "profile": str(data_dir),
              "cliSha256": digest(args.cli), "sourceHashes": {
                  str(path.relative_to(ROOT)): digest(path)
                  for path in (Path(__file__).resolve(), FIXTURE, ROOT / "tests/frame_journey_smoke.py",
                               ROOT / "src/browser.rs", ROOT / "src/dom_bridge.js",
                               ROOT / "src/cli.rs", ROOT / "src/protocol.rs")}}
    cli = Cli(args.cli)
    servers, threads = [], []
    try:
        for _ in range(2):
            servers.append(ThreadingHTTPServer(("127.0.0.1", 0), Handler))
        origin = f"http://127.0.0.1:{servers[0].server_port}"
        origins = {"SAME": origin, "CROSS": f"http://localhost:{servers[1].server_port}"}
        for server in servers:
            server.origins = origins
            server.daemon_threads = True
            worker = threading.Thread(target=server.serve_forever, daemon=True)
            worker.start()
            threads.append(worker)
        run(cli, origin, report)
    except Exception as error:
        report.update(ok=False, error=str(error))
    finally:
        for index, server in enumerate(servers):
            if index < len(threads):
                server.shutdown()
            server.server_close()
        for worker in threads:
            worker.join(timeout=2)
        report["serverCleanupVerified"] = not any(worker.is_alive() for worker in threads)
        report["commands"] = cli.log
        if not report["serverCleanupVerified"]:
            report["ok"] = False
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
