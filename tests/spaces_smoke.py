#!/usr/bin/env python3
"""Verify user/task spaces in an already running CEF app using localhost fixtures.

Run without concurrent tab activity or CLI tests. Only this test's tasks and tabs
are changed. Existing tabs are preserved and the original space is restored when
possible. This checks CLI metadata and page access, not native visibility, input
focus, popup routing, or thumbnail rendering. Does not launch or rebuild the app.
"""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import sys
import threading
import time
from urllib.parse import urlsplit
import uuid

from smoke import BrowserCli, CheckFailed, ROOT, require


def poll(read, ready, message):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        value = read()
        if ready(value):
            return value
        time.sleep(0.025)
    raise CheckFailed(message)


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {
        "ok": False,
        "scope": "real CEF local fixtures and CLI space metadata; native visibility and focus require separate UI verification",
        "checks": [],
    }
    initial = set()
    owned = set()
    tasks = {}
    original = None
    last_response = None
    server = server_thread = None
    marker = uuid.uuid4().hex

    def call(*operation, expected=None):
        nonlocal last_response
        last_response = cli.request(*operation)
        if expected:
            require(not last_response["ok"] and last_response.get("error", {}).get("code") == expected,
                    f"Expected {expected}, received {json.dumps(last_response, ensure_ascii=False)}")
            return last_response
        require(last_response["ok"], f"CLI request failed: {json.dumps(last_response, ensure_ascii=False)}")
        return last_response["result"]

    def command(task, *operation, expected=None):
        return call("task", "run", task["id"], task["revision"], *operation, expected=expected)

    def info(task):
        return call("task", "info", task["id"])["task"]

    def tab_ids():
        return {tab["tab"] for tab in call("tabs")["tabs"]}

    def snapshot():
        return {"spaces": call("spaces"), "status": call("status"), "tabs": call("tabs")["tabs"]}

    def space(state, identity):
        matches = [item for item in state["spaces"]["spaces"] if item["id"] == identity]
        require(len(matches) == 1, f"Expected one space with ID {identity}")
        return matches[0]

    def consistent(state):
        spaces, status, tabs = state["spaces"], state["status"], state["tabs"]
        if spaces["activeSpace"] != status.get("activeSpace") or spaces["activeTab"] != status.get("activeTab"):
            return False
        if spaces["overview"] != status.get("overview") or status["tabs"] != len(tabs):
            return False
        active = [tab["tab"] for tab in tabs if tab["active"]]
        if active != ([] if spaces["activeTab"] is None else [spaces["activeTab"]]):
            return False
        entries = spaces["spaces"]
        if len({item["id"] for item in entries}) != len(entries):
            return False
        if [item["id"] for item in entries if item["active"]] != [spaces["activeSpace"]]:
            return False
        membership = {}
        for item in entries:
            if item["selectedTab"] is not None and item["selectedTab"] not in item["tabs"]:
                return False
            for tab in item["tabs"]:
                if tab in membership:
                    return False
                membership[tab] = item["id"]
        return membership == {tab["tab"]: tab["space"] for tab in tabs}

    def settled(identity=None, selected=None, overview=None):
        return poll(snapshot, lambda state: consistent(state) and
                    (identity is None or state["spaces"]["activeSpace"] == identity) and
                    (selected is None or state["spaces"]["activeTab"] == selected) and
                    (overview is None or state["spaces"]["overview"] == overview),
                    "Space, tab and status metadata did not converge on the expected selection")

    def create(name):
        task = call("task", "create", name)["task"]
        tasks[task["id"]] = task
        return task

    def open_tab(name, task=None):
        result = command(task, "open", origin + "/" + name) if task else call("open", origin + "/" + name)
        tab = result["tab"]
        require(type(tab) is int and tab not in initial and tab not in owned, "Open reused a tab ID")
        owned.add(tab)
        if task:
            command(task, "wait", tab, "load", 5000)
        else:
            call("wait", tab, "load", 5000)
        return tab

    def read_page(name, tab, task=None):
        page = (command(task, "read", tab) if task else call("read", tab))["page"]
        require(page["url"] == origin + "/" + name and page["title"] == f"Space fixture {name}" and
                f"Space marker {marker} {name}" in page["text"], "Read returned another space's page")

    def manage(action, task, *extra):
        result = call("task", action, task["id"], task["revision"], *extra)
        tasks[task["id"]] = result["task"]
        return result

    def finish(task, *extra):
        result = manage("finish", task, *extra)
        tasks[task["id"]] = poll(lambda: info(task), lambda current: current["status"] == "finished",
                                 "Task did not finish after its created tabs closed")
        return result

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            name = urlsplit(self.path).path.removeprefix("/" + marker + "/")
            if name not in ("a1", "a2", "b1", "borrowed"):
                self.send_response(204)
                self.end_headers()
                return
            body = (f"<!doctype html><html><head><meta charset='utf-8'><title>Space fixture {name}</title>"
                    f"</head><body><h1>Space marker {marker} {name}</h1></body></html>").encode("utf-8")
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

    try:
        original = settled()
        require(original["status"]["engine"] == "CEF/Chromium" and
                original["status"]["windowMode"] == "single", "Expected a running single-window CEF app")
        initial = {tab["tab"] for tab in original["tabs"]}
        require(1 <= len(initial) <= 4, "Space smoke requires 1..4 initial tabs for four fixture tabs")
        user = space(original, "user")
        require(user["kind"] == "user" and user["status"] == "user" and user["selectedTab"] in initial,
                "Keep an initial user-space tab open before this test")
        require(original["status"]["resources"]["pendingTabCreations"] == 0 and
                original["status"]["resources"]["capturesInFlight"] == 0, "Run without pending creation or capture")
        report["initialTabs"] = sorted(initial)
        call("space", "show", "user")
        baseline = settled("user", user["selectedTab"], False)
        report["checks"].append("space membership, active space, selected tabs and single-window status agree")

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        origin = f"http://127.0.0.1:{server.server_port}/{marker}"
        a, b, empty = create("Space smoke A"), create("Space smoke B"), create("Space smoke empty")
        current = settled("user", baseline["spaces"]["activeTab"])
        for task in (a, b, empty):
            item = space(current, task["id"])
            require(item["kind"] == "task" and item["status"] == "active" and
                    item["tabs"] == [] and item["selectedTab"] is None, "A new task space must be empty")
        require(tab_ids() == initial, "Task creation created an unexpected CEF tab")
        require([item["id"] for item in command(a, "spaces")["spaces"]] == [a["id"]],
                "Scoped space listing leaked another task or the user space")
        command(a, "space", "show", a["id"], expected="SPACE_UI_REQUIRES_UNSCOPED")
        command(a, "space", "overview", expected="SPACE_UI_REQUIRES_UNSCOPED")
        report["checks"].append("empty task spaces allocate no tabs, preserve user selection and scope their listing; scoped UI switches are rejected")

        tab_a1, tab_a2, tab_b = open_tab("a1", a), open_tab("a2", a), open_tab("b1", b)
        current = settled("user", baseline["spaces"]["activeTab"])
        require(space(current, a["id"])["selectedTab"] == tab_a1 and
                space(current, b["id"])["selectedTab"] == tab_b, "Task opens changed their remembered first selection")
        require(set(space(current, a["id"])["tabs"]) == {tab_a1, tab_a2} and
                space(current, b["id"])["tabs"] == [tab_b], "Task tabs were assigned to the wrong space")
        for task, tab, name in ((a, tab_a1, "a1"), (a, tab_a2, "a2"), (b, tab_b, "b1")):
            read_page(name, tab, task)
        command(a, "read", tab_b, expected="TASK_TAB_MISMATCH")
        call("read", tab_a1, expected="TASK_REQUIRED")
        settled("user", baseline["spaces"]["activeTab"])
        report["checks"].append("background task opens and reads preserve user selection; first task selection and ownership are isolated")

        for identity, tab in ((a["id"], tab_a1), (b["id"], tab_b),
                              ("user", baseline["spaces"]["activeTab"]), (a["id"], tab_a1)):
            result = call("space", "show", identity)
            require(result["activeSpace"] == identity and result["activeTab"] == tab,
                    "Explicit show did not return its selected space and remembered tab")
            settled(identity, tab, False)
        report["checks"].append("explicit space switches restore each space's own selected tab")

        result = call("space", "overview")
        require(result["overview"] is True, "Overview did not open")
        read_page("b1", tab_b, b)
        settled(a["id"], overview=True)
        call("space", "show", "user")
        settled("user", baseline["spaces"]["activeTab"], False)
        call("space", "show", empty["id"])
        current = settled(empty["id"], overview=False)
        require(current["spaces"]["activeTab"] is None, "Empty space displayed another space's tab")
        finish(empty)
        current = settled("user", baseline["spaces"]["activeTab"])
        require(empty["id"] not in {item["id"] for item in current["spaces"]["spaces"]},
                "Finished empty space remains in the switcher")
        report["checks"].append("overview survives background reads; empty-space selection has no active tab and finishing it restores the user space")

        call("space", "show", b["id"])
        borrowed = open_tab("borrowed")
        current = settled(b["id"], tab_b)
        require(borrowed in space(current, "user")["tabs"], "Unscoped open did not go to the user space")
        a = manage("adopt", a, borrowed)["task"]
        current = settled(b["id"], tab_b)
        require(borrowed in space(current, a["id"])["tabs"] and borrowed not in space(current, "user")["tabs"],
                "Adopt did not move the borrowed tab into the task space")
        a = manage("release", a, borrowed)["task"]
        current = settled(b["id"], tab_b)
        require(borrowed in space(current, "user")["tabs"], "Release did not return the borrowed tab to its source space")
        read_page("borrowed", borrowed)
        a = manage("adopt", a, borrowed)["task"]
        report["checks"].append("unscoped open enters user space without stealing a viewed task; adopt and release move a borrowed tab and preserve current selection")

        result = finish(a, "--keep", tab_a1)
        require(set(result["closeRequested"]) == {tab_a2}, "Finish closed a kept or borrowed tab")
        current = settled(b["id"], tab_b)
        kept = space(current, a["id"])
        require(kept["status"] == "finished" and kept["tabs"] == [tab_a1] and kept["selectedTab"] == tab_a1,
                "Kept created tab did not remain in its completed task space")
        require(borrowed in space(current, "user")["tabs"] and tab_a2 not in tab_ids(),
                "Finish did not return the borrowed tab or close the unkept created tab")
        read_page("a1", tab_a1)
        read_page("borrowed", borrowed)
        call("space", "show", a["id"])
        settled(a["id"], tab_a1)
        report["checks"].append("finish keeps created results in the completed space, returns borrowed tabs, closes other created tabs and releases CLI ownership")

        call("space", "show", b["id"])
        b = manage("adopt", b, tab_a1)["task"]
        current = settled(b["id"], tab_b)
        require(space(current, a["id"])["tabs"] == [] and
                tab_a1 in space(current, b["id"])["tabs"],
                "Adopting a kept result lost its completed source space or assigned it incorrectly")
        read_page("a1", tab_a1, b)
        b = manage("release", b, tab_a1)["task"]
        current = settled(b["id"], tab_b)
        require(space(current, a["id"])["tabs"] == [tab_a1] and
                space(current, a["id"])["selectedTab"] == tab_a1 and
                tab_a1 not in space(current, "user")["tabs"],
                "Release sent the kept result to user space instead of its completed source space")
        read_page("a1", tab_a1)
        report["checks"].append("borrowing a completed result preserves its source space; release returns it to that completed space")

        finish(b)
        current = settled("user")
        require(b["id"] not in {item["id"] for item in current["spaces"]["spaces"]} and
                tab_b not in tab_ids(), "Finishing the viewed task left an empty space or its created tab")
        report["checks"].append("finishing the viewed task without kept tabs removes its empty space and returns to user space")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
        report["error"] = str(error)
        report["lastResponse"] = last_response
    finally:
        cleanup = []
        for task in list(tasks.values()):
            try:
                current = info(task)
                if current["status"] != "finished":
                    finish(current)
            except (CheckFailed, OSError, KeyError, TypeError) as error:
                cleanup.append(str(error))
        if owned:
            try:
                for tab in sorted(owned & tab_ids()):
                    require(tab not in initial, "Refusing to close an initial tab")
                    call("close", tab)
                final = poll(tab_ids, lambda value: not (value & owned), "Test-owned tabs did not close")
                require(initial <= final, "An initial tab disappeared")
                current = settled()
                require(not ({task for task in tasks} & {item["id"] for item in current["spaces"]["spaces"]}),
                        "Cleanup retained an empty test-owned space")
                report["finalResources"] = current["status"]["resources"]
                require(report["finalResources"]["pendingTabCreations"] == 0 and
                        report["finalResources"]["capturesInFlight"] == 0,
                        "Cleanup left a creation reservation or capture")
                report["checks"].append("test tasks, tabs and empty spaces cleaned up; initial tabs preserved and reservations returned to zero")
            except (CheckFailed, OSError, KeyError, TypeError) as error:
                cleanup.append(str(error))
        if original:
            try:
                current = settled()
                original_id = original["spaces"]["activeSpace"]
                if original_id in {item["id"] for item in current["spaces"]["spaces"]}:
                    call("space", "show", original_id)
                    if original["spaces"]["overview"]:
                        call("space", "overview")
            except (CheckFailed, OSError, KeyError, TypeError) as error:
                cleanup.append(str(error))
        if cleanup:
            report["ok"] = False
            report["cleanupErrors"] = cleanup
        if server:
            server.shutdown()
            server.server_close()
        if server_thread:
            server_thread.join(timeout=2)
    report["checkCount"] = len(report["checks"])
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
