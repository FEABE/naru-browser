#!/usr/bin/env python3
"""Prepared local CEF test for Chromium text fill and retained DOM fallbacks.

Requires an already-running compatible app and exclusive CLI access. This file
is syntax checked only until explicitly run. Uses one owned task/tab; preserves
initial user tabs/selection. No action retry or user-space activation is needed.
Checks page effects separately from dispatch acknowledgement. Does not establish
physical keyboard input, external-site compatibility or complete focus-race safety.
Native contenteditable fill is covered separately by contenteditable_smoke.py.
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


def target_events(before, after, target):
    require(after["sequence"] >= before["sequence"], "Fixture event sequence moved backward")
    if after["events"]:
        require(after["events"][0]["sequence"] <= before["sequence"] + 1,
                "Fixture event window lost events from this action")
    return [event for event in after["events"]
            if event["sequence"] > before["sequence"] and event["target"] == target]


def chromium_events(events, *, canceled=False, multiline=False, empty_noop=False):
    require(all(event["trusted"] is True and event["inputType"] == "insertText" and
                event["type"] in ("beforeinput", "input") for event in events),
            "Text fill produced an unexpected event, trust value or immediate change")
    beforeinput = [event for event in events if event["type"] == "beforeinput"]
    inputs = [event for event in events if event["type"] == "input"]
    if empty_noop:
        # Independent Chromium observed no editing events for empty-on-empty;
        # the real CEF run must still verify the returned native value below.
        require(not events, "Empty-on-empty insertion unexpectedly emitted editing events")
    else:
        require(len(beforeinput) == 1, "Text fill must produce one beforeinput")
        require((not inputs) if canceled else (len(inputs) >= 1 if multiline else len(inputs) == 1),
                "Unexpected number of native input events")
    require(events == beforeinput + inputs and all(event["canceled"] is canceled for event in beforeinput),
            "Text fill event order or cancellation differs from the native sequence")


def dom_events(events):
    require([event["type"] for event in events] == ["input", "change"] and
            all(event["trusted"] is False for event in events),
            "DOM fallback did not preserve its synthetic event sequence")


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {"ok": False, "scope": "local CEF Chromium text fill and DOM fallback verification", "checks": []}
    initial, owned = set(), set()
    task = baseline = server = server_thread = last_response = None
    server_started = False
    fake_password = f"naru-fill-secret-{uuid.uuid4().hex}"

    def secret_absent(value):
        require(fake_password not in json.dumps(value, ensure_ascii=False),
                "Password value leaked into a response or page snapshot")

    def redacted(value):
        # Preserve a diagnostic response without printing the test password if
        # the implementation violates the non-disclosure check itself.
        if isinstance(value, str):
            return value.replace(fake_password, "[redacted]")
        if isinstance(value, list):
            return [redacted(item) for item in value]
        if isinstance(value, dict):
            return {key: redacted(item) for key, item in value.items()}
        return value

    def call(*command, expected=None):
        nonlocal last_response
        last_response = cli.request(*command)
        secret_absent(last_response)
        if expected:
            require(not last_response["ok"] and last_response.get("error", {}).get("code") == expected,
                    f"Expected {expected}, received {last_response.get('error', {}).get('code', 'success')}")
            return last_response
        require(last_response["ok"], f"CLI failed: {last_response.get('error', {}).get('code', 'unknown')}")
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
        server_started = True
        task = call("task", "create", "Native fill fixture")["task"]
        tab = scoped("open", f"http://127.0.0.1:{server.server_port}{path}")["tab"]
        require(tab not in initial, "Fixture reused an initial tab")
        owned.add(tab)
        scoped("wait", tab, "load", 10000)

        def read():
            page = scoped("read", tab)["page"]
            require(page.get("title") == "Naru native fill fixture", "Unexpected fixture")
            return page

        def fill(label, target, text, *, code=None, attempted=True, native=True):
            preserved()
            page = read()
            result = scoped("fill", tab, element(page, label)["ref"], text, expected=code)
            if code:
                details = result["error"].get("details")
                require(isinstance(details, dict) and details.get("attempted") is attempted,
                        "Fill failure lost its attempted evidence")
                if native:
                    require("trusted" not in details and
                            ("outcomeUnknown" not in details or isinstance(details["outcomeUnknown"], bool)),
                            "Chromium fill failure must not claim event trust")
                else:
                    require(details == {"attempted": attempted, "trusted": False},
                            "DOM fallback changed its boolean failure evidence")
            else:
                outcome = result["page"]
                require(outcome.get("ok") is True and outcome.get("action") == "fill", "Unexpected fill success")
                if native:
                    require(outcome.get("input") == "chromium" and outcome.get("attempted") is True and
                            outcome.get("verified") is True and outcome.get("dispatchAcknowledged") is True and
                            "trusted" not in outcome, "Chromium fill success lacks verified dispatch evidence")
                else:
                    require(outcome.get("trusted") is False and "input" not in outcome,
                            "DOM fallback must retain synthetic evidence")
            before, after = receipt(page), receipt(read())
            preserved()
            return before, after, target_events(before, after, target), result

        poll(read, lambda page: receipt(page)["childReady"] is True, "Frame fixture did not become ready")
        for label, key, text in (("Text input", "text", "same"), ("Search input", "search", "query"),
                                 ("Telephone input", "tel", "+82 10 1234"),
                                 ("URL input", "url", "https://example.test/next"),
                                 ("Textarea input", "textarea", "a\nb"),
                                 ("Shadow input", "shadow", "shadow"), ("Frame input", "frame", "frame")):
            _, after, events, _ = fill(label, key, text)
            require(after["states"][key]["value"] == text, "Requested value was not reached")
            chromium_events(events, multiline=key == "textarea")
        for label, key in (("Text input", "text"), ("Textarea input", "textarea")):
            for text, noop in (("same", False), ("same", False), ("", False), ("", True)):
                _, after, events, _ = fill(label, key, text)
                require(after["states"][key]["value"] == text, "Same-value/empty result differs")
                chromium_events(events, empty_noop=noop)
        _, after, events, _ = fill("Password input", "password", fake_password)
        require(after["states"]["password"]["length"] == len(fake_password), "Password length differs")
        chromium_events(events)
        report["checks"].append("text/search/tel/url/password, LF textarea, empty/same-value, shadow/frame reach exact values with Chromium evidence")

        # The native change belongs to the previous target when the next fill
        # focuses a different control. It is not an immediate fill event.
        _, after_source, events, _ = fill("Rollback on change", "reset-change", "requested")
        require(after_source["states"]["reset-change"]["value"] == "requested", "Fill did not verify before blur")
        chromium_events(events)
        _, after_blur, events, _ = fill("Search input", "search", "blur destination")
        chromium_events(events)
        changes = target_events(after_source, after_blur, "reset-change")
        require(len(changes) == 1 and changes[0]["type"] == "change" and changes[0]["trusted"] is True and
                after_blur["states"]["reset-change"]["value"] == "rollback", "Native change/rollback did not occur on blur")
        report["checks"].append("change occurs on later blur; final-value verification is a point-in-time result")

        for label, key, text, final in (("Maximum length input", "maxlength", "123456789", "1234"),
                                       ("Maximum length textarea", "textarea-maxlength", "123456789", "1234"),
                                       ("Text input", "text", "first\r\nsecond\rthird", "first second third"),
                                       ("Textarea input", "textarea", "first\r\nsecond\rthird", "first\nsecond\nthird"),
                                       ("Rollback on input", "reset-input", "requested", "rollback"),
                                       ("Spoofed own getter", "spoof", "requested", "rollback")):
            _, after, events, _ = fill(label, key, text, code="FILL_STATE_NOT_REACHED")
            require(after["states"][key]["value"] == final, "Unexpected native value after failed fill")
            chromium_events(events, multiline=key == "textarea")
        for label, key in (("Canceled input", "cancel"), ("Canceled textarea", "textarea-cancel")):
            _, after, events, _ = fill(label, key, "requested", code="FILL_STATE_NOT_REACHED")
            require(after["states"][key]["value"] == "old", "Canceled beforeinput still changed the value")
            chromium_events(events, canceled=True)
        _, after, events, _ = fill("Password rollback", "password-reset", fake_password, code="FILL_STATE_NOT_REACHED")
        require(after["states"]["password-reset"]["length"] == 0, "Password input rollback did not run")
        chromium_events(events)
        report["checks"].append("maxlength, CRLF normalization, canceled beforeinput, input rollback and getter spoof cannot claim exact-value success; password values stay absent")

        for label, key in (("Remove on input", "remove"), ("Lock on input", "lock"), ("Retype on input", "type")):
            _, after, events, _ = fill(label, key, "requested", code="FILL_STATE_UNCONFIRMED")
            require(any(event["type"] == "input" and event["trusted"] is True for event in events),
                    "Post-input invalidation did not reach its fixture handler")
            require(not after["states"][key]["connected"] if key == "remove" else
                    after["states"][key]["readonly"] if key == "lock" else after["states"][key]["type"] == "password",
                    "Post-input target change was not observed")
        before, after, events, _ = fill("Readonly input", "readonly", "requested", code="READ_ONLY", attempted=False)
        require(not events and before["states"]["readonly"] == after["states"]["readonly"],
                "Readonly rejection changed its target or emitted an input event")
        report["checks"].append("removed/locked/retyped targets report unconfirmed attempted input; readonly rejects before dispatch")

        for label, key, text in (("Email input", "email", "next@example.test"), ("Number input", "number", "12"),
                                 ("Date input", "date", "2026-10-17"), ("Range input", "range", "72"),
                                 ("Color input", "color", "#aabbcc")):
            _, after, events, _ = fill(label, key, text, native=False)
            require(after["states"][key]["value"] == text, "DOM fallback value differs")
            dom_events(events)
        for label, key, text, final in (("Number input", "number", "invalid", ""),
                                       ("Date input", "date", "invalid", ""),
                                       ("DOM rollback on change", "dom-reset-change", "requested", "rollback")):
            _, after, events, _ = fill(label, key, text, code="FILL_STATE_NOT_REACHED", native=False)
            require(after["states"][key]["value"] == final, "DOM fallback mismatch was not preserved")
            dom_events(events)
        report["checks"].append("email/number/date/range/color retain synthetic input/change and exact-value mismatch checks")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
        report.update(error=redacted(str(error)), lastResponse=redacted(last_response))
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
            for close in ((server.shutdown,) if server_started else ()) + (server.server_close,):
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
