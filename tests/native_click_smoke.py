#!/usr/bin/env python3
"""Prepared local CEF check for native hover/click and observable page effects.

Requires an already-running compatible app and exclusive CLI access. Uses one
owned task/tab and localhost fixture, preserving the user's selected space/tab.
Does not launch an app, force window focus, retry pointer actions or submit externally.
Syntax checking is not a CEF result. Menu opening and hover relocation must
actually run; dispatch acknowledgement alone never proves the page effect.
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
from smoke import BrowserCli, CheckFailed, ROOT, element, require, require_native_click_ack


FIXTURE = ROOT / "tests/fixtures/native_click.html"


def receipt(page):
    match = re.search(r"NATIVE_CLICK_STATE_BEGIN (\S+) NATIVE_CLICK_STATE_END", page.get("text", ""))
    require(match is not None, "Native click receipt is missing or truncated")
    return json.loads(unquote(match.group(1)))


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {"ok": False, "scope": "local CEF native hover/click and background selection", "checks": []}
    initial, owned = set(), set()
    task = baseline = server = server_thread = last_response = None
    server_started = False

    def call(*command, expected=None):
        nonlocal last_response
        last_response = cli.request(*command)
        if expected:
            require(last_response["ok"] is False and last_response.get("error", {}).get("code") == expected,
                    f"Expected {expected}, received {json.dumps(last_response, ensure_ascii=False)}")
            return last_response
        require(last_response["ok"] is True, f"CLI failed: {json.dumps(last_response, ensure_ascii=False)}")
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
                "Native hover/click changed the user's selected space/tab")
        require(initial <= ids(), "An initial tab disappeared")

    try:
        status = call("status")
        require(status.get("engine") == "CEF/Chromium" and status["activeSpace"] == "user" and
                status["overview"] is False, "Use an idle compatible app with a user tab selected")
        initial = ids()
        require(status["activeTab"] in initial, "Keep an initial user tab selected")
        baseline = (status["activeSpace"], status["activeTab"], status["overview"])
        require(status["pendingWaits"] == 0 and status["resources"]["pendingTabCreations"] == 0,
                "Run without concurrent waits or tab creation")
        path = f"/native-click-{uuid.uuid4().hex}.html"
        server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(FIXTURE.read_bytes(), path))
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        server_started = True
        task = call("task", "create", "Native click fixture")["task"]
        tab = scoped("open", f"http://127.0.0.1:{server.server_port}{path}")["tab"]
        require(isinstance(tab, int) and tab not in initial, "Fixture reused an initial tab")
        owned.add(tab)
        scoped("wait", tab, "load", 10000)

        def read():
            page = scoped("read", tab)["page"]
            require(page.get("title") == "Naru native click fixture", "Unexpected fixture")
            return page

        def click(label, target, observed_page=None):
            preserved()
            page = read() if observed_page is None else observed_page
            before = receipt(page)
            result = scoped("click", tab, element(page, label)["ref"])
            require_native_click_ack(result["page"])
            # Observe late receipts without ever repeating pointer actions.
            after = receipt(poll(read, lambda value: receipt(value)["clicks"][target] > before["clicks"][target],
                                 "Click acknowledgement did not produce the expected page effect"))
            events = [event for event in after["events"] if event["sequence"] > before["sequence"]]
            require(after["clicks"][target] == before["clicks"][target] + 1 and
                    all(after["counts"][kind] == before["counts"][kind] + 1 for kind in
                        ("pointerdown", "mousedown", "pointerup", "mouseup")),
                    "Expected exactly one pointer/mouse down/up pair and target activation")
            require(events and all(event["trusted"] is True for event in events),
                    "Page observed an untrusted event in the native pointer/default sequence")
            require(all(event["target"] == target and event["button"] == 0 for event in events
                        if event["type"] in ("pointerdown", "mousedown", "pointerup", "mouseup")),
                    "Pointer down/up reached the wrong element or button")
            preserved()
            return before, after, events

        poll(read, lambda page: receipt(page)["frameReady"] is True, "Nested frame fixture did not become ready")
        preserved()
        page = read()
        before = receipt(page)
        require(before["hoverMenuOpen"] is False and before["hoverMenuOpens"] == 0 and
                all(value == 0 for value in before["clicks"].values()) and
                not any(item.get("label") == "Hover menu choice" for item in page.get("interactive", [])),
                "Hover menu must begin closed, without a selectable item or prior clicks")
        result = scoped("hover", tab, element(page, "Hover menu trigger")["ref"])
        ack = result["page"]
        require(isinstance(ack, dict) and ack.get("ok") is True and ack.get("action") == "hover" and
                ack.get("input") == "chromium" and ack.get("attempted") is True and
                ack.get("dispatchAcknowledged") is True and ack.get("pointVerified") is True,
                "hover: expected Chromium move acknowledgement and point verification")
        require("trusted" not in ack and "prepared" not in ack,
                "hover: internal preparation or event trust must not be reported as success")
        # Read after hover exposes the newly opened item and supplies its new ref.
        # Poll observations only; the hover command is never repeated.
        menu_page = poll(read, lambda value: receipt(value)["hoverMenuOpen"] is True and
                         any(item.get("label") == "Hover menu choice" for item in value.get("interactive", [])),
                         "Hover acknowledgement did not expose the menu item")
        after = receipt(menu_page)
        require(after["hoverMenuOpens"] == 1 and after["hoverMenuTrusted"] is True and
                after["counts"] == before["counts"] and after["clicks"] == before["clicks"] and
                all(after["counts"][kind] == 0 for kind in
                    ("pointerdown", "mousedown", "pointerup", "mouseup", "click")),
                "Hover must open the menu without pointer down/up, clicks, or selection")
        preserved()
        before, after, events = click("Hover menu choice", "menu-choice", menu_page)
        require(after["hoverMenuSelection"] == 1 and after["hoverMenuOpen"] is False and
                after["clicks"]["menu-trigger"] == 0 and after["clicks"]["menu-choice"] == 1 and
                after["counts"]["click"] == before["counts"]["click"] + 1,
                "The newly read menu ref did not select its item exactly once")
        report["checks"].append("hover opens a menu with zero clicks/down/up; a fresh read ref selects its item once; user selection preserved")

        for label, target in (("Native click button", "button"), ("Shadow click button", "shadow"),
                              ("Nested frame button", "nested"), ("Overflow button", "overflow")):
            before, after, events = click(label, target)
            require(after["counts"]["click"] == before["counts"]["click"] + 1,
                    "One pointer action produced extra click events")
            if target == "overflow":
                require(before["scrollTop"] == 0 and after["scrollTop"] > 0,
                        "Overflow case did not exercise scroll preparation")
        report["checks"].append("main/shadow/nested scaled frame/overflow targets each receive one trusted native click")

        before, after, events = click("Native checkbox label", "label")
        require(before["checked"] is False and after["checked"] is True and
                after["clicks"]["checkbox"] == before["clicks"]["checkbox"] + 1 and
                [(event["type"], event["target"]) for event in events if event["type"] in ("input", "change")] ==
                [("input", "checkbox"), ("change", "checkbox")],
                "Label activation did not trigger the checkbox's native default/input/change behavior")
        before, after, events = click("Canceled checkbox", "canceled")
        require(before["canceledChecked"] is False and after["canceledChecked"] is False and
                all(after["counts"][kind] == before["counts"][kind] for kind in ("input", "change")) and
                any(event["type"] == "click" and event["defaultPrevented"] is True for event in events),
                "Canceled checkbox click failed to roll back its default checked state")
        report["checks"].append("label defaults and canceled checkbox effects checked separately from dispatch acknowledgement")

        for label, hover in (("Covered button", False), ("Moving hover button", True)):
            preserved()
            page = read()
            before = receipt(page)
            response = scoped("click", tab, element(page, label)["ref"], expected="CLICK_POINT_UNAVAILABLE")
            details = response["error"].get("details")
            require(isinstance(details, dict) and details.get("attempted") is hover and
                    "trusted" not in details, "Pointer rejection must use an exact attempted boolean")
            after = receipt(read())
            require(after["counts"] == before["counts"] and after["clicks"] == before["clicks"],
                    "Occlusion/stale hover point still sent pointer down/up or activated a control")
            if hover:
                require(after["hoverMoves"] == before["hoverMoves"] + 1 and after["moves"] > before["moves"],
                        "Hover rejection did not exercise a real pointer move and target relocation")
            else:
                require(after["moves"] == before["moves"], "Preparation rejection still moved the pointer")
            preserved()
        report["checks"].append("overlay rejects before dispatch; hover relocation rejects after move without down/up; user selection preserved")

        def load_hidden(query):
            # Switching spaces hides the task view; the next document loads without painting.
            call("space", "show", task["id"])
            call("space", "show", "user")
            preserved()
            scoped("navigate", tab, f"http://127.0.0.1:{server.server_port}{path}?{query}")
            scoped("wait", tab, "load", 10000)
            page = poll(read, lambda value: receipt(value)["frameReady"] is True, "Hidden fixture frames did not load")
            state = receipt(page)
            require(state["visibility"] == "hidden" and state["frames"] == 0 and state["sequence"] == 0,
                    "Precondition: the reloaded task tab must be hidden, never painted and without input")
            return page

        page = load_hidden("hidden-click")
        before, after, _ = click("Native click button", "button", page)
        require(after["counts"]["pointerdown"] == 1 and after["counts"]["click"] == 1,
                "Never-painted hidden tab did not receive exactly one native click")
        page = load_hidden("hidden-hover")
        scoped("hover", tab, element(page, "Hover menu trigger")["ref"])
        after = receipt(poll(read, lambda value: receipt(value)["hoverMenuOpen"] is True,
                             "Hover did not reach the never-painted hidden tab"))
        require(after["counts"]["pointerdown"] == 0 and after["counts"]["click"] == 0 and
                all(value == 0 for value in after["clicks"].values()),
                "Hover in a never-painted hidden tab produced pointer down or clicks")
        preserved()
        report["checks"].append("hidden-then-loaded task tab (hidden, rAF 0) receives one native click; hover-only yields zero clicks")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, ValueError, RuntimeError) as error:
        report.update(error=str(error), lastResponse=last_response)
    finally:
        errors = []
        if task:
            try:
                current = info()
                if current["status"] != "finished":
                    call("task", "finish", current["id"], current["revision"])
                poll(info, lambda value: value["status"] == "finished" and not value["tabs"] and not value["closingTabs"],
                     "Owned task did not finish closing")
                poll(ids, lambda value: not (value & owned), "Owned fixture tab did not close")
                preserved()
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                errors.append(str(error))
        if server:
            for close in ((server.shutdown, server.server_close) if server_started else (server.server_close,)):
                try:
                    close()
                except (OSError, RuntimeError) as error:
                    errors.append(str(error))
        if server_started:
            server_thread.join(timeout=2)
            if server_thread.is_alive():
                errors.append("Fixture server did not stop")
        report["cleanupVerified"] = task is not None and not errors
        if errors:
            report.update(ok=False, cleanupErrors=errors)
        elif task:
            report["checks"].append("owned task/tab and fixture server removed; initial tabs and selection preserved")
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
