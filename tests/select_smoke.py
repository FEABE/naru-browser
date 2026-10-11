#!/usr/bin/env python3
"""Check single native HTML select through an already running Naru CLI.

Uses an ephemeral localhost server and its own two tasks / one task-owned tab.
Requires the user space to be selected; never navigates or closes initial tabs.
Focus-handler cases explicitly show a fresh fixture document, then restore user space.
Run exclusively, after the app and CLI both include the select command.
This does not inspect the macOS picker UI or test trusted native selection.
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


FIXTURE = ROOT / "tests" / "fixtures" / "select.html"
BOUNDARY_VALUE = "😀" * 512


def make_handler(content, path):
    class Handler(BaseHTTPRequestHandler):
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
    match = re.search(r"SELECT_STATE_BEGIN (\S+) SELECT_STATE_END", page.get("text", ""))
    require(match is not None, "Fixture receipt is missing or truncated")
    return json.loads(unquote(match.group(1)))


def focus_receipt(page):
    match = re.search(r"FOCUS_STATE_BEGIN (\S+) FOCUS_STATE_END", page.get("text", ""))
    require(match is not None, "Fixture focus receipt is missing or truncated")
    return json.loads(unquote(match.group(1)))


def option(item, value):
    matches = [candidate for candidate in item.get("options", []) if candidate.get("value") == value]
    require(len(matches) == 1, f"Expected one readable option for value {value[:40]!r}")
    return matches[0]


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {"ok": False, "scope": "real CEF single native select through local synthetic DOM actions",
              "checks": [], "receipts": {}}
    initial = set()
    owned = set()
    tasks = {}
    server = server_thread = None
    last_response = None
    baseline = None

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
                "Task select activity changed the user's current space or selection")
        require(initial <= tab_ids(), "An initial user tab disappeared")

    try:
        status = call("status")
        require(status.get("engine") == "CEF/Chromium", "Expected running CEF app")
        require(status["activeSpace"] == "user" and not status["overview"],
                "Select the user space before this exclusive test")
        initial = tab_ids()
        require(initial and status["activeTab"] in initial, "Keep an initial user tab selected")
        baseline = (status["activeSpace"], status["activeTab"], status["overview"])
        require(status["pendingWaits"] == 0 and status["resources"]["pendingTabCreations"] == 0,
                "Run without concurrent waits or tab creation")
        path = f"/select-{uuid.uuid4().hex}.html"
        server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(FIXTURE.read_bytes(), path))
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        url = f"http://127.0.0.1:{server.server_port}{path}"
        for name in ("Select fixture A", "Select isolation B"):
            task = call("task", "create", name)["task"]
            tasks[task["id"]] = task
        a, b = list(tasks.values())
        b_baseline = task_info(b)
        tab = scoped(a, "open", url)["tab"]
        require(type(tab) is int and tab not in initial, "Open returned an existing tab ID")
        owned.add(tab)
        scoped(a, "wait", tab, "load", 10000)

        def read():
            page = scoped(a, "read", tab)["page"]
            require(page.get("title") == "Naru native select fixture", "Wrong fixture document")
            return page

        def select_ref(reference, value, changed):
            result = scoped(a, "select", tab, reference, value)
            action = result.get("page", {})
            require(result.get("tab") == tab and action.get("ok") is True and
                    action.get("action") == "select" and action.get("trusted") is False and
                    action.get("changed") is changed, "Unexpected select action receipt")

        def select_normal(value, index):
            before = read()
            previous = receipt(before)
            select_ref(element(before, "Normal choice")["ref"], value, True)
            page = read()
            current = receipt(page)
            require(current["selections"]["normal"] == [index], "Wrong selected option index")
            require(current["counts"] == {kind: previous["counts"][kind] + 1 for kind in ("input", "change")},
                    "A changed selection must dispatch exactly one input and one change")
            require([(event["type"], event["target"], event["trusted"]) for event in current["events"][-2:]] ==
                    [("input", "normal", False), ("change", "normal", False)], "Incorrect event order or trust")
            require(option(element(page, "Normal choice"), value).get("selected") is True,
                    "read.options did not preserve the exact selected value")
            return current

        def reject(label, value, code):
            before = read()
            previous = receipt(before)
            scoped(a, "select", tab, element(before, label)["ref"], value, expected=code)
            require(receipt(read()) == previous, f"Rejected {label} selection changed fixture state")

        page = read()
        normal = element(page, "Normal choice")
        require(normal.get("multiple") is False and normal.get("optionsTruncated") is False,
                "Single-select metadata is missing")
        require(option(normal, "alpha")["selected"] is True and option(normal, "beta")["selected"] is False,
                "Initial option selection is wrong")
        require(option(normal, "disabled")["disabled"] is True and
                option(normal, "group-disabled")["disabled"] is True, "Disabled option/group metadata missing")
        hidden = [item for item in normal["options"] if item.get("hidden")]
        require(len(hidden) == 2 and all("value" not in item and "label" not in item for item in hidden),
                "Hidden option details must be omitted")
        require(len(option(normal, "long-label")["label"]) == 240, "Option label bound is not 240")
        require(option(normal, BOUNDARY_VALUE)["value"] == BOUNDARY_VALUE, "1024 UTF-16 value was truncated")
        oversize = element(page, "Oversize value")["options"][1]
        require(oversize.get("valueTooLong") is True and "value" not in oversize, "Oversize value leaked or was truncated silently")
        require(element(page, "Multiple choice").get("multiple") is True, "Multiple-select metadata missing")
        limited = element(page, "Over limit choice")
        require(limited.get("optionsTruncated") is True and len(limited.get("options", [])) == 40,
                "Option reading must be bounded to 40 for one select")
        preserve_user()
        report["checks"].append("bounded native option metadata, exact values, hidden omission and disabled/group state")

        report["receipts"]["normal"] = select_normal("beta", 1)
        page = read()
        unchanged = receipt(page)
        select_ref(element(page, "Normal choice")["ref"], "beta", False)
        require(receipt(read()) == unchanged, "Same-value no-op dispatched events or changed state")
        report["checks"].append("normal selection dispatches synthetic input then change exactly once; same-value request is a no-op")

        select_normal("", 2)
        select_normal("  padded  ", 3)
        reject("Normal choice", "padded", "OPTION_NOT_FOUND")
        report["receipts"]["utf16Boundary"] = select_normal(BOUNDARY_VALUE, 4)
        reject("Normal choice", "😀" * 513, "INVALID_ARGUMENTS")
        report["checks"].append("empty and whitespace values remain exact; 1024 UTF-16 units accepted and 1026 rejected without changes")

        for label, value, code in (
            ("Normal choice", "missing", "OPTION_NOT_FOUND"),
            ("Normal choice", "disabled", "OPTION_UNAVAILABLE"),
            ("Normal choice", "group-disabled", "OPTION_UNAVAILABLE"),
            ("Normal choice", "hidden-option", "OPTION_UNAVAILABLE"),
            ("Normal choice", "group-hidden", "OPTION_UNAVAILABLE"),
            ("Duplicate choice", "duplicate", "AMBIGUOUS_OPTION"),
            ("Multiple choice", "beta", "UNSUPPORTED_ELEMENT"),
            ("Custom choice", "beta", "UNSUPPORTED_ELEMENT"),
            ("Disabled select", "beta", "ELEMENT_UNAVAILABLE"),
            ("Fieldset select", "beta", "ELEMENT_UNAVAILABLE"),
            ("Over limit choice", "v1", "OPTION_LIMIT"),
        ):
            reject(label, value, code)
        report["checks"].append("missing, disabled/group/hidden, duplicate, multiple/custom and over-limit targets reject without selection or events")

        page = read()
        before = receipt(page)
        hidden_ref = element(page, "Hide after read")["ref"]
        scoped(a, "click", tab, element(page, "Hide select after read")["ref"])
        scoped(a, "select", tab, hidden_ref, "beta", expected="ELEMENT_UNAVAILABLE")
        require(receipt(read()) == before, "Hidden-after-read select changed selection/events")
        report["checks"].append("visibility is rechecked on the existing ref after the page hides its select")

        def read_focus():
            page = read()
            scoped(a, "click", tab, element(page, "Refresh focus diagnostic")["ref"])
            page = read()
            return page, focus_receipt(page)

        # Hidden documents defer focus events until displayed. Only these cases
        # intentionally change the selected space; background checks stay above.
        preserve_user()
        report["receipts"]["focusReplacement"] = {}
        try:
            call("space", "show", a["id"])
            for label, key in (("Replace select on focus", "select"), ("Replace option on focus", "option")):
                scoped(a, "navigate", tab, url + f"?focus={key}")
                scoped(a, "wait", tab, "load", 10000)
                page, focus = read_focus()
                before = receipt(page)
                case = {"before": {"focus": focus, "selection": before}}
                report["receipts"]["focusReplacement"][key] = case
                require(focus["hasFocus"] is True and focus["visibilityState"] == "visible",
                        "Focus replacement requires a focused visible document; no native focus was forced")
                require(before["counts"] == {"input": 0, "change": 0} and not before["events"] and
                        before["replacements"] == {"select": 0, "option": 0},
                        "Focus replacement must start with an unchanged fresh document")
                case["response"] = scoped(a, "select", tab, element(page, label)["ref"], "beta",
                                          expected="ELEMENT_UNAVAILABLE")
                page, after_focus = read_focus()
                after = receipt(page)
                case["after"] = {"focus": after_focus, "selection": after}
                require(after_focus["focusCounts"][f"replace-{key}"] == 1 and
                        after["replacements"][key] == 1 and
                        after["selections"] == before["selections"] and
                        after["counts"] == {"input": 0, "change": 0} and not after["events"],
                        "A synchronous focus replacement must reject without selection or input/change events")
                select_ref(element(page, label)["ref"], "beta", True)
        finally:
            call("space", "show", "user")
            preserve_user()
        report["checks"].append("focused fresh documents reject select and option replacement without input/change; fresh refs work and user space is restored")

        old_page = read()
        old_ref = element(old_page, "Normal choice")["ref"]
        page = read()
        require(page["generation"] != old_page["generation"], "Read did not regenerate refs")
        scoped(a, "select", tab, old_ref, "beta", expected="STALE_REF")
        select_ref(element(page, "Normal choice")["ref"], "beta", True)
        report["checks"].append("a new read invalidates previous select refs while fresh refs still work")

        page = read()
        before = receipt(page)
        old_ref = element(page, "Normal choice")["ref"]
        old_a = a.copy()
        a = manage("handoff", a)
        scoped(a, "select", tab, old_ref, "alpha", expected="TASK_PAUSED")
        scoped(old_a, "select", tab, old_ref, "alpha", expected="TASK_STALE_REVISION")
        a = manage("resume", a)
        scoped(a, "select", tab, old_ref, "alpha", expected="STALE_REF")
        page = read()
        require(receipt(page) == before, "Rejected task/revision/ref request changed selection/events")
        select_ref(element(page, "Normal choice")["ref"], "alpha", True)
        require(task_info(b) == b_baseline, "Another task changed during select handoff/resume")
        preserve_user()
        report["checks"].append("handoff/revision/native epoch guards reject stale actions and preserve the user's selection and other task")

        page = read()
        old_ref = element(page, "Normal choice")["ref"]
        scoped(a, "navigate", tab, url + "?document=2")
        scoped(a, "wait", tab, "load", 10000)
        scoped(a, "select", tab, old_ref, "beta", expected="STALE_REF")
        fresh = read()
        require(fresh["documentId"] != page["documentId"] and receipt(fresh)["counts"] == {"input": 0, "change": 0},
                "Navigation did not provide an unchanged new document")
        select_ref(element(fresh, "Normal choice")["ref"], "beta", True)
        preserve_user()
        report["checks"].append("navigation expires select refs before any new read; fresh document remains actionable")
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
                final = poll(tab_ids, lambda value: not (value & owned), "Test-owned tab did not close")
                require(initial <= final, "An initial tab disappeared")
                preserve_user()
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                cleanup.append(str(error))
        if server:
            try:
                server.shutdown()
            except (OSError, RuntimeError) as error:
                cleanup.append(f"Fixture server shutdown: {error}")
            try:
                server.server_close()
            except (OSError, RuntimeError) as error:
                cleanup.append(f"Fixture server close: {error}")
        if server_thread:
            try:
                server_thread.join(timeout=2)
                require(not server_thread.is_alive(), "Fixture server thread did not stop")
            except (CheckFailed, RuntimeError) as error:
                cleanup.append(str(error))
        report["cleanupVerified"] = bool(tasks) and not cleanup
        if cleanup:
            report["ok"] = False
            report["cleanupErrors"] = cleanup
        elif tasks:
            report["checks"].append("only owned tasks and fixture tab closed; initial tabs and user selection preserved; fixture server thread stopped")
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
