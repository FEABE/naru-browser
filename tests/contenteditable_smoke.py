#!/usr/bin/env python3
"""Prepared local CEF test for plain-text contenteditable fill.

Requires an already-running compatible Naru and exclusive CLI access. Syntax
checking is not CEF validation. Actions are never retried. Uses two owned tasks,
one fixture tab, and preserves the initial user tabs/selection. Focus cases
require the task page to be visible and focused; no app activation is forced.
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


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {"ok": False, "scope": "local CEF synthetic contenteditable fill", "checks": []}
    initial, owned, tasks = set(), set(), {}
    server = server_thread = baseline = last_response = None

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

        def fill(label, text, changed=True):
            page = read()
            result = scoped(a, "fill", tab, element(page, label)["ref"], text)["page"]
            require(result.get("ok") is True and result.get("action") == "fill" and
                    result.get("trusted") is False and result.get("changed") is changed,
                    "Unexpected contenteditable success receipt")
            return receipt(read())

        def reject(label, code, attempted=None):
            page = read()
            response = scoped(a, "fill", tab, element(page, label)["ref"], "new", expected=code)
            if attempted is not None:
                require(response["error"].get("details") == {"attempted": attempted, "trusted": False},
                        "Fill failure must preserve only its two boolean evidence fields")
            return receipt(page), receipt(read())

        page = poll(read, lambda page: receipt(page)["childReady"], "Frame editor did not become ready")
        for label, key in (("Canonical editor", "canonical"), ("Empty editor", "empty")):
            before = receipt(read())
            after = fill(label, before["states"][key]["text"], False)
            require(before == after, "Canonical no-op changed focus, state or events")
        text = "  <b>literal</b>\n다음 😀  "
        before = receipt(read())
        after = fill("Normal editor", text)
        require(after["states"]["normal"]["text"] == text and after["states"]["normal"]["canonical"],
                "Literal text or full replacement was not preserved")
        events = after["events"][-2:]
        require([event["type"] for event in events] == ["beforeinput", "input"] and
                all(event["target"] == "normal" and event["data"] == text and event["inputType"] == "insertText" and
                    event["trusted"] is False and event["composed"] is True for event in events) and
                [event["cancelable"] for event in events] == [True, False] and
                after["counts"]["change"] == before["counts"]["change"], "Unexpected editing event sequence")
        for label, key in (("Plain editor", "plain"), ("Shadow editor", "shadow"), ("Frame editor", "frame")):
            require(fill(label, "new")["states"][key]["text"] == "new", "Editing realm action failed")
        require(fill("Normal editor", "")["states"]["normal"]["text"] == "", "Empty replacement failed")
        preserved()
        report["checks"].append("canonical no-op, exact literal/empty text, plaintext-only, open-shadow and same-origin frame fill")

        for label in ("Inherited editor", "Nested editor"):
            before, after = reject(label, "UNSUPPORTED_ELEMENT")
            require(before == after, "Unsupported descendant changed the page")
        before, after = reject("Readonly editor", "READ_ONLY", False)
        require(before == after, "Readonly rejection changed the page")
        before, after = reject("Canceled editor", "INPUT_CANCELED", False)
        require(after["states"]["cancel"] == before["states"]["cancel"] and
                after["counts"]["beforeinput"] == before["counts"]["beforeinput"] + 1 and
                after["counts"]["input"] == before["counts"]["input"], "Canceled beforeinput replaced contents or dispatched input")
        for label in ("Remove on beforeinput", "Readonly on beforeinput"):
            before, after = reject(label, "ELEMENT_UNAVAILABLE", False)
            require(after["counts"]["input"] == before["counts"]["input"], "Invalidated host received input")
        reject("Rollback on input", "FILL_STATE_NOT_REACHED", True)
        reject("Rewrite on input", "FILL_STATE_UNCONFIRMED", True)
        reject("Remove on input", "FILL_STATE_UNCONFIRMED", True)
        report["checks"].append("unsupported/read-only, beforeinput cancellation/mutation and post-input rollback/replacement evidence")

        call("space", "show", a["id"])
        try:
            scoped(a, "click", tab, element(read(), "Refresh editor diagnostic")["ref"])
            require(receipt(read())["hasFocus"] is True, "Focus cases require the displayed fixture to have focus")
            for label, key in (("Replace on focus", "focus-replace"), ("Disable on focus", "focus-disable")):
                before, after = reject(label, "ELEMENT_UNAVAILABLE", False)
                require(after["focuses"].get(key) == 1 and after["counts"] == before["counts"],
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
        fill("Plain editor", "resumed")
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
