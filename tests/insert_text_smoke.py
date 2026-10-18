#!/usr/bin/env python3
"""Prepared E2E for Chromium insertion at an existing caret/selection.

Requires an already-running compatible Naru app and exclusive CLI access. Uses
one owned task/tab and localhost fixture without activating its space. Syntax
checking is not a CEF result. No action is retried and no external site is used.
Input/textarea success verifies an exact value. Contenteditable success verifies
a matching input event only; fixture text/HTML observations are separate evidence,
not a promise of editor-model, formatting or whole-document compatibility.
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
from smoke import BrowserCli, CheckFailed, ROOT, element, require, require_native_press_ack


FIXTURE = ROOT / "tests/fixtures/insert_text.html"


def receipt(page):
    match = re.search(r"INSERT_STATE_BEGIN (\S+) INSERT_STATE_END", page.get("text", ""))
    require(match is not None, "Insertion receipt is missing or truncated")
    return json.loads(unquote(match.group(1)))


def insertion_events(before, after, target):
    require(after["sequence"] >= before["sequence"], "Fixture event sequence moved backward")
    if after["events"]:
        require(after["events"][0]["sequence"] <= before["sequence"] + 1,
                "Fixture event window lost this action's events")
    return [event for event in after["events"] if event["sequence"] > before["sequence"] and
            event["target"] == target and event["type"] in ("beforeinput", "input")]


def native_inputs(events, *, canceled=False):
    require(events and all(event["trusted"] is True and event["inputType"] == "insertText"
                           for event in events), "Insertion events lack native insertText evidence")
    before = [event for event in events if event["type"] == "beforeinput"]
    inputs = [event for event in events if event["type"] == "input"]
    require(before and all(event["canceled"] is canceled for event in before),
            "Unexpected beforeinput cancellation")
    require(not inputs if canceled else bool(inputs), "Unexpected input event presence")


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {"ok": False, "scope": "local CEF insertion and background selection", "checks": [],
              "observations": {}}
    initial, owned = set(), set()
    task = baseline = server = server_thread = last_response = None
    server_started = False
    fake_password = f"naru-insert-{uuid.uuid4().hex}"

    def redacted(value):
        if isinstance(value, str):
            return value.replace(fake_password, "[redacted]")
        if isinstance(value, list):
            return [redacted(item) for item in value]
        if isinstance(value, dict):
            return {key: redacted(item) for key, item in value.items()}
        return value

    def call(*command, expected=None, allow_error=False):
        nonlocal last_response
        last_response = cli.request(*command)
        require(fake_password not in json.dumps(last_response, ensure_ascii=False),
                "Password text leaked into a response or page snapshot")
        if expected:
            require(last_response["ok"] is False and last_response.get("error", {}).get("code") == expected,
                    f"Expected {expected}, received {last_response.get('error', {}).get('code', 'success')}")
            return last_response
        if allow_error:
            return last_response
        require(last_response["ok"] is True,
                f"CLI failed: {last_response.get('error', {}).get('code', 'unknown')}")
        return last_response["result"]

    def scoped(*command, **kwargs):
        return call("task", "run", task["id"], task["revision"], *command, **kwargs)

    def info():
        return call("task", "info", task["id"])["task"]

    def ids():
        return {item["tab"] for item in call("tabs")["tabs"]}

    def preserved(check_task=True):
        status = call("status")
        require((status["activeSpace"], status["activeTab"], status["overview"]) == baseline,
                "Insertion changed the user's selected space/tab")
        require(initial <= ids(), "An initial user tab disappeared")
        if task and check_task:
            current = info()
            require(current["status"] == "active" and current["revision"] == task["revision"],
                    "Input changed the task's ownership or revision")

    def acknowledged(page, verification):
        require(page.get("ok") is True and page.get("action") == "insert" and
                page.get("input") == "chromium" and page.get("attempted") is True and
                page.get("dispatchAcknowledged") is True and page.get("verification") == verification,
                "Insertion acknowledgement or verification kind differs")
        require(not any(key in page for key in ("trusted", "prepared", "verified", "text", "value", "observation")),
                "Insertion exposed internal state or claimed a different verification contract")

    def failure(response, attempted):
        details = response["error"].get("details")
        require(isinstance(details, dict) and details.get("attempted") is attempted,
                "Insertion failure lost its attempted evidence")
        require(not any(key in details for key in ("trusted", "text", "value", "observation")),
                "Insertion failure leaked text or claimed event trust")

    try:
        status = call("status")
        require(status.get("engine") == "CEF/Chromium" and status["activeSpace"] == "user" and
                status["overview"] is False, "Use an idle compatible app with a user tab selected")
        initial = ids()
        require(status["activeTab"] in initial, "Keep an initial user tab selected")
        baseline = (status["activeSpace"], status["activeTab"], status["overview"])
        require(status["pendingWaits"] == 0 and status["resources"]["pendingTabCreations"] == 0,
                "Run without concurrent waits or tab creation")
        path = f"/insert-text-{uuid.uuid4().hex}.html"
        server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(FIXTURE.read_bytes(), path))
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        server_started = True
        task = call("task", "create", "Insertion fixture")["task"]
        tab = scoped("open", f"http://127.0.0.1:{server.server_port}{path}")["tab"]
        require(isinstance(tab, int) and tab not in initial, "Fixture reused an initial tab")
        owned.add(tab)
        scoped("wait", tab, "load", 10000)

        def read():
            page = scoped("read", tab)["page"]
            require(page.get("title") == "Naru insertion fixture", "Unexpected fixture")
            return page

        def insert(label, target, text, *, code=None, attempted=True, verification="value"):
            preserved()
            page = read()
            before = receipt(page)
            result = scoped("insert", tab, element(page, label)["ref"], text, expected=code)
            if code:
                failure(result, attempted)
            else:
                acknowledged(result["page"], verification)
            after = receipt(read())
            preserved()
            return before, after, insertion_events(before, after, target)

        def setup_editor(name):
            page = read()
            result = scoped("click", tab, element(page, f"Prepare {name} insertion")["ref"])["page"]
            require(result.get("action") == "click" and result.get("input") == "chromium" and
                    result.get("dispatchAcknowledged") is True, "Fixture setup click was not acknowledged")
            target = {"empty editor": "empty-editor", "canceled editor": "cancel-editor"}.get(name, name)
            poll(read, lambda page: receipt(page)["preparedEditor"] == target and
                 receipt(page)["states"][target]["selected"] is not None,
                 "Editor setup did not expose a selection in its own host")
            preserved()

        poll(read, lambda page: receipt(page)["childReady"] is True, "Frame fixture did not become ready")
        for label, target, text, start, final in (
            ("Insertion caret", "caret", "!", 5, "alpha!"),
            ("Insertion multiline", "multiline", "\nX", 5, "alpha\nX\nbeta"),
            ("Insertion shadow", "shadow", "X", 3, "shaXdow"),
            ("Insertion frame", "frame", "X", 2, "frXame"),
        ):
            before, after, events = insert(label, target, text)
            require(before["states"][target]["start"] == before["states"][target]["end"] == start,
                    "Fixture did not retain its original caret")
            require(after["states"][target]["value"] == final, "Insertion replaced more than the original selection")
            native_inputs(events)
        report["checks"].append("caret append, textarea LF, open shadow and same-origin iframe preserve text outside the caret")

        for _ in range(2):
            page = read()
            result = scoped("press", tab, element(page, "Insertion keyboard selection")["ref"], "Shift+ArrowLeft")
            require_native_press_ack(result["page"], "Shift+ArrowLeft")
        before, after, events = insert("Insertion keyboard selection", "replace", "X")
        require((before["states"]["replace"]["start"], before["states"]["replace"]["end"]) == (3, 5),
                "Shift+ArrowLeft did not create the intended selection")
        require(after["states"]["replace"]["value"] == "abcX", "Keyboard selection was not replaced exactly")
        native_inputs(events)
        report["checks"].append("two native Shift+ArrowLeft actions followed by insert replace only the selected suffix")

        for name, label, target, text, selected, final in (
            ("plain", "Insertion plain editor", "plain", "!", "", "hello!"),
            ("rich", "Insertion rich editor", "rich", "X", "", "alXpha beta"),
            ("selected", "Insertion selected editor", "selected", "replaced", "alpha beta", "replaced"),
        ):
            setup_editor(name)
            before, after, events = insert(label, target, text, verification="input-event")
            require(before["states"][target]["selected"] == selected, "Editor fixture selection was not prepared")
            require(after["states"][target]["value"] == final, "Fixture editor text differs from the observed insertion")
            native_inputs(events)
            require(any(event["type"] == "input" and event["data"] == text for event in events),
                    "Editor did not observe the matching inserted text")
            report["observations"][target] = {"text": after["states"][target]["value"],
                                               "html": after["states"][target]["html"]}
        report["checks"].append("plain/rich/selected editors return input-event verification; fixture text/HTML recorded separately")

        for label, target, final in (("Insertion empty caret", "empty-caret", "keep"),
                                     ("Insertion empty selection", "empty-selection", "abef")):
            _, after, events = insert(label, target, "")
            require(after["states"][target]["value"] == final, "Empty insertion did not reach its exact expected value")
            report["observations"][target] = {"value": final, "events": events}
        setup_editor("empty editor")
        page = read()
        before = receipt(page)
        response = scoped("insert", tab, element(page, "Insertion empty editor")["ref"], "", allow_error=True)
        after = receipt(read())
        events = insertion_events(before, after, "empty-editor")
        if response["ok"]:
            acknowledged(response["result"]["page"], "input-event")
            require(any(event["type"] == "input" and event["trusted"] is True and
                        event["inputType"] == "insertText" and event["data"] == "" for event in events),
                    "Empty editor success lacks a matching input event")
        else:
            require(response["error"]["code"] == "INSERT_STATE_UNCONFIRMED", "Unexpected empty editor rejection")
            failure(response, True)
        require(after["states"]["empty-editor"]["value"] == "keep", "Empty collapsed editor insertion changed its content")
        report["observations"]["empty-editor"] = {"ok": response["ok"], "events": events,
                                                  "code": response.get("error", {}).get("code")}
        preserved()
        report["checks"].append("empty control insertion verifies exact state; empty editor requires matching event or reports unconfirmed")

        _, after, events = insert("Insertion password", "password", fake_password)
        require(after["states"]["password"]["length"] == len(fake_password), "Password insertion length differs")
        native_inputs(events)
        require(all(event["data"] is None for event in events), "Fixture retained password input data")
        report["checks"].append("password reaches expected length without exposing its text in snapshots or responses")

        before, after, events = insert("Insertion canceled", "cancel", "X", code="INSERT_STATE_NOT_REACHED")
        require(after["states"]["cancel"]["value"] == before["states"]["cancel"]["value"] == "stay",
                "Canceled insertion still changed the control")
        native_inputs(events, canceled=True)
        _, after, events = insert("Insertion maximum length", "maxlength", "12345", code="INSERT_STATE_NOT_REACHED")
        require(after["states"]["maxlength"]["value"] == "ab12", "Unexpected maxlength truncation")
        native_inputs(events)
        setup_editor("canceled editor")
        _, after, events = insert("Insertion canceled editor", "cancel-editor", "X",
                                  code="INSERT_STATE_UNCONFIRMED", verification="input-event")
        require(after["states"]["cancel-editor"]["value"] == "stay", "Canceled editor insertion changed its content")
        native_inputs(events, canceled=True)
        report["checks"].append("canceled beforeinput and maxlength cannot report successful insertion")

        for label, target in (("Insertion retype", "retype"), ("Insertion remove", "remove"),
                              ("Insertion blur", "blur")):
            before, after, events = insert(label, target, "X", code="INSERT_STATE_UNCONFIRMED")
            require(any(event["type"] == "input" and event["trusted"] is True for event in events),
                    "Post-input invalidation handler did not run")
            require(after["states"]["other"]["value"] == before["states"]["other"]["value"],
                    "Target-change fixture unexpectedly edited another control")
            if target == "retype":
                require(after["states"][target]["type"] == "password", "Input type did not change")
            elif target == "remove":
                require(after["states"][target]["connected"] is False, "Input target did not detach")
            else:
                require(after["active"] == "other", "Input handler did not move focus to the other target")
        report["checks"].append("retyped, detached and focus-changed targets remain attempted but unconfirmed")

        for label, target, counter in (("Insertion focus range", "focus-range", "focusRangeChanges"),
                                       ("Insertion focus value", "focus-value", "focusValueChanges")):
            before, after, events = insert(label, target, "X", code="SELECTION_NOT_REACHED", attempted=False)
            require(after[counter] == before[counter] + 1 and not events,
                    "Focus-time mutation was not observed before insertion rejection")
            if target == "focus-range":
                require(after["states"][target]["value"] == "alpha" and
                        after["states"][target]["start"] == after["states"][target]["end"] == 4,
                        "Range mutation was overwritten by insertion preparation")
            else:
                require(after["states"][target]["value"] == "changed", "Focus handler's value change was overwritten")
        report["checks"].append("focus-time value/range changes reject before native insertion")
        preserved()
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
                poll(info, lambda value: value["status"] == "finished" and not value["tabs"] and not value["closingTabs"],
                     "Owned task did not close")
                poll(ids, lambda value: not (value & owned), "Owned tab did not close")
                preserved(check_task=False)
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                errors.append(redacted(str(error)))
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
            report["checks"].append("owned task/tab and local server removed; initial user selection preserved")
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
