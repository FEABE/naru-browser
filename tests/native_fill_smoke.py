#!/usr/bin/env python3
"""Prepared local CEF test for native input/textarea fill result verification.

Requires an already-running compatible app and exclusive CLI access. This file
is syntax checked only until explicitly run. Uses one owned task/tab; preserves
initial user tabs/selection. No action retry or user-space activation is needed.
Focus-handler timing and native input remain separate validation scopes.
"""

import argparse
from http.server import ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import sys
import threading
from urllib.parse import unquote
import uuid

from check_smoke import make_handler, poll
from smoke import BrowserCli, CheckFailed, ROOT, element, require


FIXTURE = ROOT / "tests/fixtures/native_fill.html"


def receipt(page):
    match = re.search(r"NATIVE_FILL_STATE_BEGIN (\S+) NATIVE_FILL_STATE_END", page.get("text", ""))
    require(match is not None, "Native fill receipt is missing or truncated")
    return json.loads(unquote(match.group(1)))


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {"ok": False, "scope": "local CEF synthetic native fill verification", "checks": []}
    initial, owned = set(), set()
    task = baseline = server = server_thread = last_response = None

    def call(*command, expected=None):
        nonlocal last_response
        last_response = cli.request(*command)
        if expected:
            require(not last_response["ok"] and last_response.get("error", {}).get("code") == expected,
                    f"Expected {expected}, received {json.dumps(last_response, ensure_ascii=False)}")
            return last_response
        require(last_response["ok"], f"CLI failed: {json.dumps(last_response, ensure_ascii=False)}")
        return last_response["result"]

    def scoped(*command, **kwargs):
        return call("task", "run", task["id"], task["revision"], *command, **kwargs)

    def info():
        return call("task", "info", task["id"])["task"]

    def ids():
        return {item["tab"] for item in call("tabs")["tabs"]}

    def preserved():
        status = call("status")
        require((status["activeSpace"], status["activeTab"], status["overview"]) == baseline,
                "The user's selected space/tab changed")
        require(initial <= ids(), "An initial user tab disappeared")

    try:
        status = call("status")
        require(status.get("engine") == "CEF/Chromium" and status["activeSpace"] == "user" and not status["overview"],
                "Use an idle compatible app with a user tab selected")
        initial = ids()
        require(status["activeTab"] in initial, "Keep an initial user tab selected")
        baseline = (status["activeSpace"], status["activeTab"], status["overview"])
        require(status["pendingWaits"] == 0 and status["resources"]["pendingTabCreations"] == 0,
                "Run without concurrent waits or tab creation")
        path = f"/native-fill-{uuid.uuid4().hex}.html"
        server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(FIXTURE.read_bytes(), path))
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        task = call("task", "create", "Native fill fixture")["task"]
        tab = scoped("open", f"http://127.0.0.1:{server.server_port}{path}")["tab"]
        require(tab not in initial, "Fixture reused an initial tab")
        owned.add(tab)
        scoped("wait", tab, "load", 10000)

        def read():
            page = scoped("read", tab)["page"]
            require(page.get("title") == "Naru native fill fixture", "Unexpected fixture")
            return page

        def fill(label, text, code=None, attempted=True):
            page = read()
            result = scoped("fill", tab, element(page, label)["ref"], text, expected=code)
            if code:
                details = result["error"].get("details")
                require(isinstance(details, dict) and set(details) == {"attempted", "trusted"} and
                        details["attempted"] is attempted and details["trusted"] is False,
                        "Fill failure did not preserve its two boolean evidence fields")
            else:
                require(result["page"].get("ok") is True and result["page"].get("action") == "fill" and
                        result["page"].get("trusted") is False, "Unexpected fill success")
            return receipt(page), receipt(read()), result

        poll(read, lambda page: receipt(page)["childReady"], "Frame fixture did not become ready")
        for label, key, text in (("Text input", "text", "same"), ("Text input", "text", "same"),
                                 ("Text input", "text", ""), ("Textarea input", "textarea", "a\nb"),
                                 ("Number input", "number", "12"), ("Date input", "date", "2026-10-17"),
                                 ("Shadow input", "shadow", "shadow"), ("Frame input", "frame", "frame")):
            before, after, _ = fill(label, text)
            require(after["states"][key]["value"] == text, "Requested value was not reached")
            require(after["counts"]["beforeinput"] == before["counts"]["beforeinput"] and
                    all(after["counts"][kind] == before["counts"][kind] + 1 for kind in ("input", "change")),
                    "Native fill must keep input/change, including same-value writes")
            require([(event["type"], event["target"], event["trusted"]) for event in after["events"][-2:]] ==
                    [(kind, key, False) for kind in ("input", "change")], "Unexpected fill event order/trust")
        preserved()
        report["checks"].append("exact/empty/same-value inputs, LF textarea, native types and shadow/frame preserve input/change")

        for label, key, text, final in (("Number input", "number", "invalid", ""),
                                       ("Date input", "date", "invalid", ""),
                                       ("Text input", "text", "a\nb", "ab"),
                                       ("Textarea input", "textarea", "a\r\nb", "a\nb"),
                                       ("Rollback on input", "reset-input", "requested", "rollback"),
                                       ("Rollback on change", "reset-change", "requested", "rollback"),
                                       ("Spoofed own getter", "spoof", "requested", "rollback")):
            before, after, _ = fill(label, text, "FILL_STATE_NOT_REACHED")
            require(after["states"][key]["value"] == final, "Unexpected native value after failed fill")
            require(all(after["counts"][kind] == before["counts"][kind] + 1 for kind in ("input", "change")),
                    "Value mismatch must retain the original input/change sequence")
        report["checks"].append("sanitization, strict CRLF comparison, handler rollback and own-getter spoof cannot claim success")

        for label in ("Remove on input", "Lock on input", "Retype on input"):
            before, after, _ = fill(label, "requested", "FILL_STATE_UNCONFIRMED")
            require(after["counts"]["input"] == before["counts"]["input"] + 1 and
                    after["counts"]["change"] == before["counts"]["change"], "Invalidated target received change")
        before, after, _ = fill("Readonly input", "requested", "READ_ONLY", False)
        require(before == after, "Readonly rejection mutated the fixture")
        secret = "fake-private-fill-value"
        _, _, response = fill("Password rollback", secret, "FILL_STATE_NOT_REACHED")
        require(secret not in json.dumps(response) and secret not in json.dumps(read()), "Password leaked into response/snapshot")
        preserved()
        report["checks"].append("post-input target changes stop change; readonly rejects before setter; password failure does not expose values")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
        report.update(error=str(error), lastResponse=last_response)
    finally:
        errors = []
        if task:
            try:
                current = info()
                if current["status"] != "finished":
                    call("task", "finish", current["id"], current["revision"])
                poll(info, lambda value: value["status"] == "finished" and not value["tabs"] and not value["closingTabs"], "Owned task did not close")
                poll(ids, lambda value: not (value & owned), "Owned tab did not close")
                preserved()
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                errors.append(str(error))
        if server:
            for close in (server.shutdown, server.server_close):
                try:
                    close()
                except (OSError, RuntimeError) as error:
                    errors.append(str(error))
        if server_thread:
            server_thread.join(timeout=2)
            if server_thread.is_alive():
                errors.append("Fixture server did not stop")
        report["cleanupVerified"] = task is not None and not errors
        if errors:
            report.update(ok=False, cleanupErrors=errors)
        elif task:
            report["checks"].append("owned task/tab and fixture removed; initial user selection preserved")
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
