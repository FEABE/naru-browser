#!/usr/bin/env python3
"""Verify pending multiple-download permissions through an already running Naru.

Uses one ephemeral loopback server and four owned task spaces. Run exclusively
with the user space selected and no active downloads or permission requests.
Close native downloads, search and bookmarks overlays before running this test.
No permission is accepted and no save dialog is opened. Initial tabs are never
navigated, adopted or closed. This verifies real CEF lifecycle callbacks through
synthetic CLI clicks; it does not inspect native permission cards or Save UI.
Explicit test space selection is needed to surface Chromium hidden prompts.
This does not verify automatic permission presentation for background tasks.
"""

import argparse
from http.server import ThreadingHTTPServer
import json
import os
from pathlib import Path
import sys
import threading
import time
import uuid

from download_fixture import State, handler
from smoke import BrowserCli, CheckFailed, ROOT, element, require


def poll(read, ready, message):
    deadline = time.monotonic() + 3
    value = None
    while time.monotonic() < deadline:
        value = read()
        if ready(value):
            return value
        time.sleep(0.025)
    raise CheckFailed(f"{message}: {json.dumps(value, ensure_ascii=False)}")


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {"ok": False,
              "scope": "real CEF permission lifecycle through synthetic CLI clicks; explicit test space selection needed to surface Chromium hidden prompts",
              "checks": [], "receipts": {}, "spaceSelections": [],
              "notRun": ["paused user request then resume requires native fixture button"]}
    tasks = {}
    initial = set()
    owned = set()
    baseline = None
    state = server = server_thread = None
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

    def info(task):
        return call("task", "info", task["id"])["task"]

    def manage(action, task, *extra):
        current = call("task", action, task["id"], task["revision"], *extra)["task"]
        tasks[current["id"]] = current
        return current

    def tab_ids():
        return {item["tab"] for item in call("tabs")["tabs"]}

    def downloads():
        value = call("status")["downloads"]
        require(all(type(value.get(key)) is int for key in ("active", "awaitingSave", "awaitingPermission")),
                "Missing download counters; run a build with download permissions")
        return value

    def counts(active, save, permission):
        return poll(downloads,
                    lambda value: (value["active"], value["awaitingSave"], value["awaitingPermission"]) ==
                    (active, save, permission),
                    f"Expected downloads active={active}, awaitingSave={save}, awaitingPermission={permission}")

    def stable_counts(active, save, permission):
        start = time.monotonic()
        samples = 0
        value = None
        while time.monotonic() - start < 3:
            value = downloads()
            require((value["active"], value["awaitingSave"], value["awaitingPermission"]) ==
                    (active, save, permission),
                    f"Revoked hidden request reappeared: {json.dumps(value, ensure_ascii=False)}")
            samples += 1
            time.sleep(0.05)
        require(samples >= 3, "Too few observations to check a delayed hidden permission")
        return {"samples": samples, "seconds": round(time.monotonic() - start, 3), "last": value}

    def preserve_user():
        status = call("status")
        require((status["activeSpace"], status["activeTab"], status["overview"]) == baseline,
                "Background downloads changed the user's current space or selection")
        require(initial <= tab_ids(), "An initial user tab disappeared")

    def create_task(case):
        task = call("task", "create", f"Download permission {case} {run_id}")["task"]
        tasks[task["id"]] = task
        return task

    def show_space(space):
        require(space == "user" or space in tasks, "Refusing to select another task's space")
        call("space", "show", space)
        report["spaceSelections"].append(space)

    def open_tab(url, task=None):
        result = scoped(task, "open", url) if task else call("open", url)
        tab = result["tab"]
        require(type(tab) is int and tab not in initial and tab not in owned,
                "Open returned an existing tab ID")
        owned.add(tab)
        if task:
            scoped(task, "wait", tab, "load", 10000)
        else:
            call("wait", tab, "load", 10000)
        return tab

    def click(tab, label, task=None):
        page = (scoped(task, "read", tab) if task else call("read", tab))["page"]
        require(page.get("title") == "Naru 다운로드 검증", "Wrong download fixture document")
        reference = element(page, label)["ref"]
        result = scoped(task, "click", tab, reference) if task else call("click", tab, reference)
        require(result.get("page", {}).get("ok") is True, "Synthetic click did not succeed")
        return reference

    def queue_hidden(case, tab, task=None):
        before = state.snapshot()
        counts(0, 0, 0)
        click(tab, "작은 파일 저장", task)
        counts(1, 1, 0)
        first = poll(state.snapshot,
                     lambda value: value["requests"]["/small.bin"] == before["requests"]["/small.bin"] + 1,
                     "The first attachment request was not observed")
        reference = click(tab, "느린 파일 저장", task)
        pending = counts(1, 1, 0)
        after = state.snapshot()
        require(after["requests"]["/slow.bin"] == before["requests"]["/slow.bin"],
                "The second attachment reached the server before permission approval")
        preserve_user()
        report["receipts"][case] = {"tab": tab, "before": before, "first": first,
                                    "hiddenDownloads": pending, "hiddenServer": after}
        return reference

    def queue_permission(case, tab, task):
        reference = queue_hidden(case, tab, task)
        try:
            show_space(task["id"])
            report["receipts"][case]["visibleDownloads"] = counts(1, 1, 1)
            require(state.snapshot()["requests"]["/slow.bin"] == 0,
                    "Displaying the pending permission started the second attachment")
        finally:
            show_space("user")
        preserve_user()
        report["receipts"][case]["returnedToUserDownloads"] = counts(1, 1, 1)
        return reference

    def show_revoked(case, task, active, save):
        try:
            show_space(task["id"])
            report["receipts"][case]["revokedVisibleSamples"] = stable_counts(active, save, 0)
            require(state.snapshot()["requests"]["/slow.bin"] == 0,
                    "A revoked hidden request started an attachment when shown")
        finally:
            show_space("user")
        preserve_user()

    def finish(task):
        current = info(task)
        if current["status"] != "finished":
            manage("finish", current)
        current = poll(lambda: info(task),
                       lambda value: value["status"] == "finished" and not value["tabs"] and not value["closingTabs"],
                       "Owned task did not finish closing its created tabs")
        tasks[current["id"]] = current
        return current

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
        counts(0, 0, 0)
        run_id = uuid.uuid4().hex[:10]
        state = State()
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler(state))
        server.daemon_threads = False
        server.block_on_close = True
        server_thread = threading.Thread(target=server.serve_forever)
        server_thread.start()
        url = f"http://127.0.0.1:{server.server_port}/?run={run_id}"

        task = create_task("handoff")
        tab = open_tab(url + "&case=handoff", task)
        stale_task = dict(task)
        reference = queue_permission("handoff", tab, task)
        require(info(task)["status"] == "active", "Permission request unexpectedly paused the AI task")
        preserve_user()
        task = manage("handoff", task)
        report["receipts"]["handoff"]["afterHandoff"] = counts(0, 0, 0)
        scoped(stale_task, "click", tab, reference, expected="TASK_STALE_REVISION")
        require(state.snapshot()["requests"]["/slow.bin"] == 0, "Handoff allowed a pending request")
        preserve_user()
        finish(task)
        report["checks"].append("hidden second download waits without network or user selection change; explicit owned-space selection surfaces permission; after returning to user, handoff clears permission and save queue; old revision cannot click")

        task = create_task("navigation")
        tab = open_tab(url + "&case=navigation", task)
        queue_permission("navigation", tab, task)
        scoped(task, "navigate", tab, url + "&case=navigation&navigation=2")
        scoped(task, "wait", tab, "load", 10000)
        report["receipts"]["navigation"]["afterNavigation"] = counts(1, 1, 0)
        require(state.snapshot()["requests"]["/slow.bin"] == 0, "Navigation allowed a pending request")
        preserve_user()
        finish(task)
        counts(0, 0, 0)
        report["checks"].append("document navigation clears the old permission; finishing its task clears the earlier save request")

        task = create_task("adopt")
        # Creating in the background and releasing keeps the original user
        # selection even when it is not the first user tab. A foreground open
        # followed by close would select the first remaining tab instead.
        tab = open_tab(url + "&case=adopt", task)
        task = manage("release", task, tab)
        queue_hidden("adopt", tab)
        preserve_user()
        task = manage("adopt", task, tab)
        report["receipts"]["adopt"]["afterAdopt"] = counts(1, 1, 0)
        show_revoked("adopt", task, 1, 1)
        require(state.snapshot()["requests"]["/slow.bin"] == 0, "Adoption allowed a pending user request")
        finish(task)
        require(tab in tab_ids(), "Finishing a task closed its borrowed user tab")
        call("close", tab)
        poll(tab_ids, lambda value: tab not in value, "Owned borrowed fixture tab did not close")
        counts(0, 0, 0)
        preserve_user()
        report["checks"].append("adoption revokes a hidden user request before Chromium surfaces it; explicit task selection observes no permission in repeated status samples; finish preserves the borrowed tab and owned-tab close clears its save request")

        task = create_task("hidden-handoff")
        tab = open_tab(url + "&case=hidden-handoff", task)
        queue_hidden("hiddenHandoff", tab, task)
        task = manage("handoff", task)
        report["receipts"]["hiddenHandoff"]["afterHandoff"] = counts(0, 0, 0)
        show_revoked("hiddenHandoff", task, 0, 0)
        finish(task)
        counts(0, 0, 0)
        preserve_user()
        report["checks"].append("handoff revokes a hidden request before its CEF prompt callback; explicitly showing the paused task observes no permission in repeated status samples")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, ValueError, RuntimeError) as error:
        report["error"] = str(error)
        report["lastResponse"] = last_response
    finally:
        cleanup = []
        if state:
            state.stop.set()
        for task in list(tasks.values()):
            try:
                finish(task)
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                cleanup.append(f"Owned task cleanup: {error}")
        if owned:
            try:
                remaining = owned & tab_ids()
                for tab in sorted(remaining):
                    require(tab not in initial, "Refusing to close an initial tab")
                    call("close", tab)
                final = poll(tab_ids, lambda value: not (value & owned), "Owned fixture tabs did not close")
                require(initial <= final, "An initial tab disappeared during cleanup")
                counts(0, 0, 0)
                preserve_user()
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                cleanup.append(f"Owned tab cleanup: {error}")
        if server:
            try:
                if server_thread and server_thread.is_alive():
                    server.shutdown()
            except (OSError, RuntimeError) as error:
                cleanup.append(f"Fixture server shutdown: {error}")
            try:
                server.server_close()
            except (OSError, RuntimeError) as error:
                cleanup.append(f"Fixture server close: {error}")
        if server_thread:
            try:
                server_thread.join(timeout=5)
                require(not server_thread.is_alive(), "Fixture server thread did not stop")
            except (CheckFailed, RuntimeError) as error:
                cleanup.append(str(error))
        if state and state.snapshot()["active"] != 0:
            cleanup.append("Fixture response threads remained active after server close")
        report["cleanup"] = {"taskIds": list(tasks), "ownedTabs": sorted(owned),
                             "server": state.snapshot() if state else None,
                             "serverThreadStopped": server_thread is not None and not server_thread.is_alive()}
        report["cleanupVerified"] = bool(tasks) and not cleanup
        if cleanup:
            report["ok"] = False
            report["cleanupErrors"] = cleanup
        elif tasks:
            report["checks"].append("only owned tasks and tabs closed; initial tabs and user selection preserved; download counters zero and fixture server stopped")
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
