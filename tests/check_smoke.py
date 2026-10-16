#!/usr/bin/env python3
"""Check native checkbox/radio actions through an already running Naru CLI.

Prepared for a future CEF run; adding or syntax-checking this script is not a
browser validation result. Run exclusively with the check-capable app and CLI.
Uses localhost, two owned tasks and one fixture tab, preserving initial tabs.
Focus-handler cases explicitly display a fresh fixture and restore user space.
No action is retried. DOM click is synthetic; browser-generated input/change
event trust is recorded separately and is not assumed to be false.
"""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import sys
import threading
import time
from urllib.parse import unquote, urlsplit
import uuid

from smoke import BrowserCli, CheckFailed, ROOT, element, require


FIXTURE = ROOT / "tests" / "fixtures" / "check.html"


def make_handler(content, path):
    class Handler(BaseHTTPRequestHandler):
        def setup(self):
            # Idle Chromium preconnect sockets must not strand fixture cleanup.
            self.request.settimeout(3)
            super().setup()

        def do_GET(self):
            if urlsplit(self.path).path != path:
                self.send_response(204 if self.path == "/favicon.ico" else 404)
                self.end_headers()
                return
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(content)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            try:
                self.wfile.write(content)
            except (BrokenPipeError, ConnectionResetError):
                pass

        def log_message(self, *_args):
            pass

    return Handler


def poll(read, ready, message):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        value = read()
        if ready(value):
            return value
        time.sleep(0.025)
    raise CheckFailed(message)


def receipt(page):
    match = re.search(r"CHECK_STATE_BEGIN (\S+) CHECK_STATE_END", page.get("text", ""))
    require(match is not None, "Fixture receipt is missing or truncated")
    return json.loads(unquote(match.group(1)))


def unchanged(before, after):
    return all(before[key] == after[key] for key in ("states", "counts", "focuses", "events"))


def attempted_details(response, state):
    details = response.get("error", {}).get("details")
    require(isinstance(details, dict) and details == {"attempted": True, "trusted": False, **state},
            "Attempted check error must preserve only its confirmed boolean state")


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {"ok": False, "scope": "real CEF native checkbox/radio through local synthetic DOM actions",
              "checks": [], "receipts": {}}
    initial = set()
    owned = set()
    tasks = {}
    server = server_thread = None
    baseline = None
    last_response = None

    def call(*operation, expected=None):
        nonlocal last_response
        last_response = cli.request(*operation)
        if expected:
            require(not last_response["ok"] and last_response.get("error", {}).get("code") == expected,
                    f"Expected {expected}, received {json.dumps(last_response, ensure_ascii=False)}")
            return last_response
        require(last_response["ok"], f"CLI failed: {json.dumps(last_response, ensure_ascii=False)}")
        return last_response["result"]

    def scoped(task, *operation, **kwargs):
        return call("task", "run", task["id"], task["revision"], *operation, **kwargs)

    def task_info(task):
        return call("task", "info", task["id"])["task"]

    def manage(action, task):
        updated = call("task", action, task["id"], task["revision"])["task"]
        tasks[updated["id"]] = updated
        return updated

    def tab_ids():
        return {item["tab"] for item in call("tabs")["tabs"]}

    def preserve_user():
        status = call("status")
        require((status["activeSpace"], status["activeTab"], status["overview"]) == baseline,
                "Check activity changed the user's selected space or tab")
        require(initial <= tab_ids(), "An initial user tab disappeared")

    try:
        status = call("status")
        require(status.get("engine") == "CEF/Chromium", "Expected a running CEF app")
        require(status["activeSpace"] == "user" and not status["overview"],
                "Select the user space before this exclusive test")
        initial = tab_ids()
        require(initial and status["activeTab"] in initial, "Keep an initial user tab selected")
        baseline = (status["activeSpace"], status["activeTab"], status["overview"])
        require(status["pendingWaits"] == 0 and status["resources"]["pendingTabCreations"] == 0,
                "Run without concurrent waits or tab creation")
        path = f"/check-{uuid.uuid4().hex}.html"
        server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(FIXTURE.read_bytes(), path))
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        url = f"http://127.0.0.1:{server.server_port}{path}"
        for name in ("Check fixture A", "Check isolation B"):
            task = call("task", "create", name)["task"]
            tasks[task["id"]] = task
        a, b = list(tasks.values())
        b_baseline = task_info(b)
        tab = scoped(a, "open", url)["tab"]
        require(type(tab) is int and tab not in initial, "Open returned an existing tab")
        owned.add(tab)
        scoped(a, "wait", tab, "load", 10000)

        def read():
            page = scoped(a, "read", tab)["page"]
            require(page.get("title") == "Naru native check fixture", "Wrong fixture document")
            return page

        def check_ref(reference, checked, changed):
            result = scoped(a, "check", tab, reference, str(checked).lower())
            action = result.get("page", {})
            require(result.get("tab") == tab and action.get("ok") is True and
                    action.get("action") == "check" and action.get("trusted") is False and
                    action.get("changed") is changed and action.get("checked") is checked,
                    "Unexpected check success receipt")
            return action

        def change(label, key, checked):
            before = read()
            old = receipt(before)
            check_ref(element(before, label)["ref"], checked, True)
            current = receipt(read())
            require(current["states"][key]["checked"] is checked and
                    current["states"][key]["indeterminate"] is False, "Requested native state was not reached")
            require(current["counts"] == {kind: old["counts"][kind] + 1 for kind in ("click", "input", "change")},
                    "Changed check must cause exactly one click/input/change")
            events = current["events"][-3:]
            require([(event["type"], event["target"]) for event in events] ==
                    [(kind, key) for kind in ("click", "input", "change")], "Incorrect native activation event sequence")
            require(events[0]["trusted"] is False and all(type(event["trusted"]) is bool for event in events),
                    "Synthetic click must be untrusted; record native follow-up event trust separately")
            return current

        def reject(label, checked, code):
            before = read()
            scoped(a, "check", tab, element(before, label)["ref"], str(checked).lower(), expected=code)
            require(unchanged(receipt(before), receipt(read())), f"Rejected {label} changed state, focus or events")

        page = poll(read, lambda value: receipt(value)["childReady"], "Same-origin frame fixture did not load")
        require(element(page, "Mixed checkbox").get("indeterminate") is True and
                element(page, "Normal checkbox").get("indeterminate") is False,
                "Native checkbox indeterminate metadata is missing")
        require("indeterminate" not in element(page, "Radio A") and
                "indeterminate" not in element(page, "Custom checkbox"), "Indeterminate leaked onto unsupported controls")
        before = receipt(page)
        check_ref(element(page, "Normal checkbox")["ref"], False, False)
        require(unchanged(before, receipt(read())), "Initial false no-op changed focus/state/events")
        report["receipts"]["normal"] = change("Normal checkbox", "normal", True)
        page = read()
        before = receipt(page)
        check_ref(element(page, "Normal checkbox")["ref"], True, False)
        require(unchanged(before, receipt(read())), "Repeated true no-op changed focus/state/events")
        change("Normal checkbox", "normal", False)
        preserve_user()
        report["checks"].append("checkbox read metadata, true/false one-click activation and already-matching zero-focus/event no-op")

        report["receipts"]["radio"] = change("Radio B", "radio-b", True)
        require(receipt(read())["states"]["radio-a"]["checked"] is False, "Native radio activation did not clear its peer")
        for label, value, code in (
            ("Radio A", False, "UNSUPPORTED_STATE"), ("Radio B", False, "UNSUPPORTED_STATE"),
            ("Mixed checkbox", False, "UNSUPPORTED_STATE"), ("Mixed checkbox", True, "UNSUPPORTED_STATE"),
            ("Custom checkbox", True, "UNSUPPORTED_ELEMENT"), ("Disabled checkbox", True, "ELEMENT_UNAVAILABLE"),
            ("Fieldset checkbox", True, "ELEMENT_UNAVAILABLE"), ("Normal checkbox", "TRUE", "INVALID_ARGUMENTS"),
        ):
            if value == "TRUE":
                page = read()
                before = receipt(page)
                scoped(a, "check", tab, element(page, label)["ref"], value, expected=code)
                require(unchanged(before, receipt(read())), "Invalid CLI boolean changed the fixture")
            else:
                reject(label, value, code)
        report["checks"].append("native radio group behavior; unsupported false/mixed/custom, disabled and invalid boolean reject without events")

        for label, key in (("Canceled checkbox", "cancel"), ("Canceled radio", "cancel-radio-b")):
            page = read()
            before = receipt(page)
            response = scoped(a, "check", tab, element(page, label)["ref"], "true", expected="CHECK_STATE_NOT_REACHED")
            attempted_details(response, {"checked": False, **({"indeterminate": False} if key == "cancel" else {})})
            after = receipt(read())
            require(after["states"][key]["checked"] is False and
                    after["counts"] == {**before["counts"], "click": before["counts"]["click"] + 1},
                    "Canceled activation retried or emitted input/change")
            require(after["events"][-1]["trusted"] is False, "Canceled click was not synthetic")
            if key == "cancel-radio-b":
                require(after["states"]["cancel-radio-a"]["checked"] is True, "Canceled radio activation did not restore its peer")
            report["receipts"][key] = {"response": response, "state": after}
        report["checks"].append("preventDefault rolls back checkbox/radio activation after one click and no input/change")

        for label, key, code in (
            ("Reset on change", "reset", "CHECK_STATE_NOT_REACHED"),
            ("Mixed on change", "mixed-after", "CHECK_STATE_NOT_REACHED"),
            ("Remove on click", "remove-click", "CHECK_STATE_UNCONFIRMED"),
            ("Retype on click", "retype-click", "CHECK_STATE_UNCONFIRMED"),
            ("Disable on click", "disable-click", "CHECK_STATE_UNCONFIRMED"),
        ):
            page = read()
            before = receipt(page)
            response = scoped(a, "check", tab, element(page, label)["ref"], "true", expected=code)
            attempted_details(response, {"checked": False, "indeterminate": False} if key == "reset" else
                              {"checked": True, "indeterminate": True} if key == "mixed-after" else {})
            after = receipt(read())
            require(after["counts"]["click"] == before["counts"]["click"] + 1, "Failed check was retried")
            if key == "reset":
                require(after["states"][key]["checked"] is False, "Reset handler did not run")
            if key == "mixed-after":
                require(after["states"][key]["indeterminate"] is True, "Mixed handler did not run")
            if key == "remove-click":
                require(key not in after["states"] and
                        all(after["counts"][kind] == before["counts"][kind] for kind in ("input", "change")),
                        "Detached input must stop activation before input/change")
            report["receipts"][key] = {"response": response, "state": after}
        report["checks"].append("post-handler reset/mixed reports not reached; removed/retyped/disabled reports unconfirmed without retry")

        change("Shadow checkbox", "shadow-check", True)
        change("Frame checkbox", "frame-check", True)
        page = read()
        old_ref = element(page, "Normal checkbox")["ref"]
        scoped(a, "click", tab, element(page, "Hide normal after read")["ref"])
        scoped(a, "check", tab, old_ref, "false", expected="ELEMENT_UNAVAILABLE")
        preserve_user()
        report["checks"].append("open Shadow DOM and same-origin iframe native inputs work; hidden-after-read target rejects")

        # Background focus events can be deferred. These explicit cases require
        # a focused visible fixture instead of treating missing handlers as a pass.
        try:
            call("space", "show", a["id"])
            for label, key, code in (
                ("Replace on focus", "replace-focus", "ELEMENT_UNAVAILABLE"),
                ("Retype on focus", "retype-focus", "ELEMENT_UNAVAILABLE"),
                ("Mixed on focus", "mixed-focus", "UNSUPPORTED_STATE"),
                ("Reach state on focus", "finish-focus", None),
            ):
                scoped(a, "navigate", tab, url + f"?focus={key}")
                scoped(a, "wait", tab, "load", 10000)
                page = poll(read, lambda value: receipt(value)["childReady"], "Fresh iframe did not load")
                scoped(a, "click", tab, element(page, "Refresh check diagnostic")["ref"])
                page = read()
                before = receipt(page)
                require(before["hasFocus"] is True and before["visibilityState"] == "visible",
                        "Focus cases require a focused visible document; no native focus was forced")
                if code:
                    response = scoped(a, "check", tab, element(page, label)["ref"], "true", expected=code)
                else:
                    response = check_ref(element(page, label)["ref"], True, False)
                after = receipt(read())
                require(after["focuses"].get(key) == 1 and after["counts"] == before["counts"],
                        "Focus mutation must run once with zero click/input/change")
                report["receipts"][key] = {"response": response, "state": after}
        finally:
            call("space", "show", "user")
            preserve_user()
        report["checks"].append("focused fresh documents reject replacement/type/mixed mutations; focus reaching state is a no-click success")

        page = read()
        old_ref = element(page, "Normal checkbox")["ref"]
        fresh = read()
        scoped(a, "check", tab, old_ref, "true", expected="STALE_REF")
        check_ref(element(fresh, "Normal checkbox")["ref"], True, True)
        page = read()
        before = receipt(page)
        old_ref = element(page, "Normal checkbox")["ref"]
        old_a = a.copy()
        a = manage("handoff", a)
        scoped(a, "check", tab, old_ref, "false", expected="TASK_PAUSED")
        scoped(old_a, "check", tab, old_ref, "false", expected="TASK_STALE_REVISION")
        a = manage("resume", a)
        scoped(a, "check", tab, old_ref, "false", expected="STALE_REF")
        page = read()
        require(unchanged(before, receipt(page)), "Rejected revision/ref changed native state/events")
        check_ref(element(page, "Normal checkbox")["ref"], False, True)
        require(task_info(b) == b_baseline, "Another task changed during check handoff/resume")
        preserve_user()
        report["checks"].append("read, task handoff and revision expiry reject stale checks while preserving user selection and the other task")

        page = read()
        old_ref = element(page, "Normal checkbox")["ref"]
        scoped(a, "navigate", tab, url + "?document=new")
        scoped(a, "wait", tab, "load", 10000)
        scoped(a, "check", tab, old_ref, "true", expected="STALE_REF")
        fresh = read()
        require(fresh["documentId"] != page["documentId"], "Navigation did not replace the document")
        check_ref(element(fresh, "Normal checkbox")["ref"], True, True)
        preserve_user()
        report["checks"].append("navigation expires previous document refs; a fresh read remains actionable")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
        report["error"] = str(error)
        report["lastResponse"] = last_response
    finally:
        cleanup = []
        for task in tasks.values():
            try:
                current = task_info(task)
                if current["status"] != "finished":
                    call("task", "finish", current["id"], current["revision"])
                poll(lambda: task_info(task),
                     lambda value: value["status"] == "finished" and not value["tabs"] and not value["closingTabs"],
                     "Test task did not finish closing its fixture tabs")
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                cleanup.append(str(error))
        if owned:
            try:
                final = poll(tab_ids, lambda value: not (value & owned), "Owned fixture tab did not close")
                require(initial <= final, "An initial tab disappeared")
                preserve_user()
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                cleanup.append(str(error))
        if server:
            for operation in (server.shutdown, server.server_close):
                try:
                    operation()
                except (OSError, RuntimeError) as error:
                    cleanup.append(f"Fixture server cleanup: {error}")
        if server_thread:
            server_thread.join(timeout=2)
            if server_thread.is_alive():
                cleanup.append("Fixture server thread did not stop")
        report["cleanupVerified"] = bool(tasks) and not cleanup
        if cleanup:
            report["ok"] = False
            report["cleanupErrors"] = cleanup
        elif tasks:
            report["checks"].append("only owned tasks/tab closed; initial tabs/selection preserved; fixture server stopped")
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
