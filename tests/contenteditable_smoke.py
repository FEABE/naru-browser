#!/usr/bin/env python3
"""Prepared local CEF test for Chromium contenteditable fill.

Requires an already-running compatible Naru and exclusive CLI access. Syntax
checking is not CEF validation. Actions are never retried. Uses two owned tasks,
one fixture tab, and preserves the initial user tabs/selection. Focus cases
require the task page to be visible and focused; no app activation is forced.
Native rendered innerText is checked exactly. Event counts, DOM formatting,
same-value no-op behavior and editor undo are not inferred from acknowledgement.
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


FIXTURE = ROOT / "tests/fixtures/contenteditable.html"


def receipt(page):
    match = re.search(r"EDITOR_STATE_BEGIN (\S+) EDITOR_STATE_END", page.get("text", ""))
    require(match is not None, "Editor receipt is missing or truncated")
    return json.loads(unquote(match.group(1)))


def target_events(before, after, target):
    require(after["sequence"] >= before["sequence"], "Fixture event sequence moved backward")
    if after["events"]:
        require(after["events"][0]["sequence"] <= before["sequence"] + 1,
                "Fixture event window lost events from this action")
    return [event for event in after["events"]
            if event["sequence"] > before["sequence"] and event["target"] == target]


def native_events(events, *, changed=False, canceled=False):
    editing = [event for event in events if event["type"] in ("beforeinput", "input")]
    require(all(event["trusted"] is True for event in editing),
            "Chromium editing events must be trusted")
    before = [event for event in editing if event["type"] == "beforeinput"]
    inputs = [event for event in editing if event["type"] == "input"]
    if canceled:
        require(before and all(event["canceled"] is True for event in before) and not inputs,
                "Canceled beforeinput still produced input or was not observed")
    elif changed:
        # Multiline editing can emit several input events and inputType values.
        require(before and inputs and before[0]["sequence"] < inputs[0]["sequence"],
                "Changed contents lack native beforeinput/input evidence")


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {"ok": False, "scope": "local CEF Chromium contenteditable fill", "checks": []}
    initial, owned, tasks = set(), set(), {}
    server = server_thread = baseline = last_response = None
    server_started = False

    def call(*command, expected=None):
        nonlocal last_response
        last_response = cli.request(*command)
        if expected:
            require(not last_response["ok"] and last_response.get("error", {}).get("code") == expected,
                    f"Expected {expected}, received {json.dumps(last_response, ensure_ascii=False)}")
            return last_response
        require(last_response["ok"], f"CLI failed: {json.dumps(last_response, ensure_ascii=False)}")
        return last_response["result"]

    def scoped(task, *command, **kwargs):
        return call("task", "run", task["id"], task["revision"], *command, **kwargs)

    def info(task):
        return call("task", "info", task["id"])["task"]

    def ids():
        return {item["tab"] for item in call("tabs")["tabs"]}

    def preserved():
        status = call("status")
        require((status["activeSpace"], status["activeTab"], status["overview"]) == baseline,
                "User space or selection changed")
        require(initial <= ids(), "An initial tab disappeared")

    try:
        status = call("status")
        require(status.get("engine") == "CEF/Chromium" and status["activeSpace"] == "user" and not status["overview"],
                "Use an idle compatible app with a user tab selected")
        initial = ids()
        require(status["activeTab"] in initial, "Keep an initial user tab selected")
        baseline = (status["activeSpace"], status["activeTab"], status["overview"])
        require(status["pendingWaits"] == 0 and status["resources"]["pendingTabCreations"] == 0,
                "Run without concurrent waits or tab creation")
        path = f"/editor-{uuid.uuid4().hex}.html"
        server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(FIXTURE.read_bytes(), path))
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        server_started = True
        url = f"http://127.0.0.1:{server.server_port}{path}"
        for name in ("Contenteditable fixture A", "Contenteditable isolation B"):
            task = call("task", "create", name)["task"]
            tasks[task["id"]] = task
        a, b = list(tasks.values())
        b_baseline = info(b)
        tab = scoped(a, "open", url)["tab"]
        require(tab not in initial, "Fixture reused an initial tab")
        owned.add(tab)
        scoped(a, "wait", tab, "load", 10000)

        def read():
            page = scoped(a, "read", tab)["page"]
            require(page.get("title") == "Naru contenteditable fixture", "Unexpected document")
            return page

        def fill(label, key, text):
            page = read()
            result = scoped(a, "fill", tab, element(page, label)["ref"], text)["page"]
            require(result.get("ok") is True and result.get("action") == "fill" and
                    result.get("input") == "chromium" and result.get("attempted") is True and
                    result.get("verified") is True and result.get("dispatchAcknowledged") is True and
                    "trusted" not in result and "changed" not in result,
                    "Unexpected contenteditable success receipt")
            before, after = receipt(page), receipt(read())
            require(after["states"][key]["plainText"] == text,
                    "Native rendered text differs from the requested string")
            events = target_events(before, after, key)
            native_events(events, changed=before["states"][key]["plainText"] != text)
            return before, after, events

        def reject(label, key, code, attempted=None):
            page = read()
            response = scoped(a, "fill", tab, element(page, label)["ref"], "new", expected=code)
            if attempted is not None:
                details = response["error"].get("details", {})
                require(details.get("attempted") is attempted and "trusted" not in details and
                        ("outcomeUnknown" not in details or isinstance(details["outcomeUnknown"], bool)),
                        "Native failure lost attempted evidence or claimed synthetic event trust")
            before, after = receipt(page), receipt(read())
            return before, after, target_events(before, after, key)

        page = poll(read, lambda page: receipt(page)["childReady"], "Frame editor did not become ready")
        fill("Normal editor", "normal", "single line 😀")
        for label, key in (("Canonical editor", "canonical"), ("Empty editor", "empty")):
            before = receipt(read())
            fill(label, key, before["states"][key]["plainText"])
        report["checks"].append("single-line and same/empty-value fills verify native text without a synthetic no-op contract")
        fill("Normal editor", "normal", "<b>literal</b>\n다음 😀\nlast line")
        fill("Normal editor", "normal", "")
        report["checks"].append("multiline literal text and clearing verify native innerText without DOM-shape or input-count assumptions")
        for label, key in (("Plain editor", "plain"), ("Shadow editor", "shadow"), ("Frame editor", "frame")):
            fill(label, key, "new")
        preserved()
        report["checks"].append("plaintext-only, open-shadow and same-origin iframe native fills preserve user selection")

        for label, key in (("Inherited editor", "inherited"), ("Nested editor", "nested")):
            before, after, events = reject(label, key, "UNSUPPORTED_ELEMENT")
            require(before["states"][key] == after["states"][key] and not events,
                    "Unsupported descendant changed its contents or editing events")
        before, after, events = reject("Readonly editor", "readonly", "READ_ONLY", False)
        require(before["states"]["readonly"] == after["states"]["readonly"] and not events,
                "Readonly rejection changed the host")
        before, after, events = reject("Canceled editor", "cancel", "FILL_STATE_NOT_REACHED", True)
        require(after["states"]["cancel"]["plainText"] == before["states"]["cancel"]["plainText"],
                "Canceled beforeinput changed the text")
        native_events(events, canceled=True)
        report["checks"].append("unsupported/readonly reject before input; canceled native input retains attempted evidence and unchanged text")

        for label, key in (("Remove on beforeinput", "before-remove"), ("Readonly on beforeinput", "before-readonly"),
                           ("Replace host on input", "rewrite"), ("Remove on input", "input-remove")):
            _, after, events = reject(label, key, "FILL_STATE_UNCONFIRMED", True)
            native_events(events)
            require(any(event["type"] == "beforeinput" for event in events), "Target mutation was not reached")
            require(after["states"][key]["readonly"] if key == "before-readonly" else
                    not after["states"][key]["connected"], "Expected target invalidation did not occur")
        _, after, events = reject("Rollback on input", "rollback", "FILL_STATE_NOT_REACHED", True)
        require(after["states"]["rollback"]["plainText"] == "rollback", "Rollback handler did not run")
        native_events(events, changed=True)
        report["checks"].append("beforeinput invalidation, host replacement/removal and native value rollback cannot claim verified success")

        call("space", "show", a["id"])
        try:
            scoped(a, "click", tab, element(read(), "Refresh editor diagnostic")["ref"])
            require(receipt(read())["hasFocus"] is True, "Focus cases require the displayed fixture to have focus")
            for label, key in (("Replace on focus", "focus-replace"), ("Disable on focus", "focus-disable")):
                before, after, events = reject(label, key, "ELEMENT_UNAVAILABLE", False)
                require(after["focuses"].get(key) == 1 and not events,
                        "Focus invalidation must precede beforeinput and replacement")
        finally:
            call("space", "show", "user")
        preserved()
        report["checks"].append("visible focused host replacement and editability changes reject before events")

        old_ref = element(read(), "Plain editor")["ref"]
        read()
        scoped(a, "fill", tab, old_ref, "later", expected="STALE_REF")
        old_ref = element(read(), "Plain editor")["ref"]
        a = call("task", "handoff", a["id"], a["revision"])["task"]
        tasks[a["id"]] = a
        scoped(a, "fill", tab, old_ref, "later", expected="TASK_PAUSED")
        a = call("task", "resume", a["id"], a["revision"])["task"]
        tasks[a["id"]] = a
        scoped(a, "fill", tab, old_ref, "later", expected="STALE_REF")
        fill("Plain editor", "plain", "resumed")
        old_ref = element(read(), "Plain editor")["ref"]
        scoped(a, "navigate", tab, url + "?new-document")
        scoped(a, "wait", tab, "load", 10000)
        scoped(a, "fill", tab, old_ref, "later", expected="STALE_REF")
        require(info(b) == b_baseline, "Unrelated task changed")
        preserved()
        report["checks"].append("read/navigation/task revisions expire refs while preserving the user selection and unrelated task")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
        report.update(error=str(error), lastResponse=last_response)
    finally:
        errors = []
        for task in tasks.values():
            try:
                current = info(task)
                if current["status"] != "finished":
                    call("task", "finish", current["id"], current["revision"])
                poll(lambda: info(task), lambda value: value["status"] == "finished" and not value["tabs"] and not value["closingTabs"],
                     "Owned task did not close")
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                errors.append(str(error))
        if owned:
            try:
                poll(ids, lambda value: not (value & owned), "Owned tab did not close")
                preserved()
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                errors.append(str(error))
        if server:
            if server_started:
                server.shutdown()
            server.server_close()
        if server_thread:
            server_thread.join(timeout=2)
            if server_thread.is_alive():
                errors.append("Fixture server did not stop")
        report["cleanupVerified"] = bool(tasks) and not errors
        if errors:
            report.update(ok=False, cleanupErrors=errors)
        elif tasks:
            report["checks"].append("owned tasks/tab removed; original tabs/selection and fixture cleanup verified")
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
