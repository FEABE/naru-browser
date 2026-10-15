#!/usr/bin/env python3
"""Verify navigation failure retention and CLI reload in an already running CEF app.

Requires one selected user tab and exclusive CLI control. Starts only its own
loopback fixture, task and tabs; never launches the app or modifies profiles.
JSON stdout retains failed fixture responses. This does not test native recovery
buttons, user handoff, external sites, or IME input.
"""

import argparse
from http.server import ThreadingHTTPServer
import json
import os
from pathlib import Path
import sys
import threading
import time

from navigation_recovery_fixture import State, handler
from smoke import BrowserCli, CheckFailed, ROOT, require


EXPECTED_CHECKS = 6


def poll(read, ready, message, timeout=8):
    deadline = time.monotonic() + timeout
    while True:
        value = read()
        if ready(value):
            return value
        require(time.monotonic() < deadline, message)
        time.sleep(0.05)


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {"ok": False, "scope": "real local CEF navigation failure and CLI reload",
              "expectedChecks": EXPECTED_CHECKS, "checks": [], "receipts": {}}
    initial = set()
    owned = set()
    tasks = {}
    baseline = None
    last_owned_response = None
    state = State()
    server = server_thread = None

    def call(*operation, expected=None, receipt=None):
        nonlocal last_owned_response
        response = cli.request(*operation)
        # Only explicitly named operations on this fixture are recorded. Global
        # tabs/task lists may contain user data and are never copied to stdout.
        if receipt:
            report["receipts"][receipt] = response
            last_owned_response = response
        if expected:
            require(not response["ok"] and response.get("error", {}).get("code") == expected,
                    f"{operation[0]}: expected {expected}, received "
                    f"{response.get('error', {}).get('code', 'success')}")
            return response
        require(response["ok"], f"{operation[0]}: "
                f"{response.get('error', {}).get('code', 'UNKNOWN_ERROR')}")
        return response["result"]

    def scoped(task, *operation, **kwargs):
        return call("task", "run", task["id"], task["revision"], *operation, **kwargs)

    def operation(task, *command, **kwargs):
        return scoped(task, *command, **kwargs) if task else call(*command, **kwargs)

    def tabs():
        return call("tabs")["tabs"]

    def tab_ids():
        return {item["tab"] for item in tabs()}

    def own_tab(tab):
        require(tab in owned and tab not in initial, "Refusing to inspect an unowned fixture tab")
        matches = [item for item in tabs() if item["tab"] == tab]
        require(len(matches) == 1, "Fixture tab disappeared")
        return matches[0]

    def current_task(task):
        return call("task", "info", task["id"])["task"]

    def counters_idle(status):
        return (status["pendingWaits"] == 0 and
                status["resources"]["capturesInFlight"] == 0 and
                status["resources"]["pendingTabCreations"] == 0 and
                all(status["downloads"][name] == 0
                    for name in ("active", "awaitingSave", "awaitingPermission")))

    def preserve_user():
        status = call("status")
        require((status["activeSpace"], status["activeTab"], status["overview"]) == baseline,
                "Background recovery changed the original user space or selection")
        require(initial <= tab_ids(), "An initial user tab disappeared")

    def preserve_owner(task, tab, key):
        current = current_task(task)
        report[key] = {"status": current["status"], "revision": current["revision"],
                       "tabs": current["tabs"]}
        require(current["status"] == "active" and current["revision"] == task["revision"],
                "CLI recovery handed off or revised its task")
        require({item["tab"] for item in current["tabs"]} == {tab},
                "Task ownership changed during recovery")
        preserve_user()

    def open_owned(url, task=None):
        result = operation(task, "open", url)
        tab = result.get("tab")
        require(type(tab) is int and tab not in initial and tab not in owned,
                "Open returned a missing or reused tab ID")
        owned.add(tab)
        return tab

    def request_navigation(action, tab, task=None, *extra):
        accepted = operation(task, action, tab, *extra, receipt=f"{tab}-{action}")
        require(accepted.get("tab") == tab and accepted.get("action") == action and
                accepted.get("navigationRequested") is True,
                f"{action}: missing same-tab navigation receipt")

    def complete(tab, url, title, task=None, key="complete"):
        loaded = operation(task, "wait", tab, "load", 10000, receipt=key)
        navigation = loaded["navigation"]
        require(loaded["tab"] == tab and loaded["url"] == url and not loaded["loading"] and
                navigation["committed"] and navigation["documentAvailable"] and
                navigation["mainLoadComplete"] and not navigation["provisional"] and
                navigation["error"] is None, "Reload did not finish on the same error-free document")
        page = operation(task, "read", tab)["page"]
        require(page["title"] == title and page["url"] == url and page["readyState"] == "complete",
                "Completed document title, URL or readiness differs from fixture")
        current = own_tab(tab)
        require(current["navigation"]["error"] is None and not current["loading"],
                "Tab state retained a stale error after successful load")

    def failed(tab, url, task=None, key="failure"):
        response = operation(task, "wait", tab, "load", 10000,
                             expected="NAVIGATION_FAILED", receipt=key + "Wait")
        require(response["error"]["details"]["tab"] == tab, "Failure refers to a different tab")
        settled = poll(lambda: own_tab(tab),
                       lambda value: not value["loading"] and not value["navigation"]["provisional"],
                       "Failed navigation did not finish loading")
        report[key + "SettledState"] = settled

        def exact_failure(value):
            error = value["navigation"]["error"]
            require(isinstance(error, dict) and error.get("code") == -324 and
                    error.get("text") == "ERR_EMPTY_RESPONSE" and error.get("failedUrl") == url,
                    "Settled failure did not retain ERR_EMPTY_RESPONSE (-324)")
            return error

        retained = exact_failure(settled)
        # Allow Chromium retries while observing delayed error replacement.
        # Only a settled navigation must retain the original failure.
        observed_at = time.monotonic()
        observation = {"seconds": 0.0, "settledSamples": 0, "inFlightSamples": 0}
        report[key + "Observation"] = observation
        while True:
            current = own_tab(tab)
            report[key + "LastState"] = current
            elapsed = time.monotonic() - observed_at
            observation["seconds"] = round(elapsed, 3)
            if current["loading"] or current["navigation"]["provisional"]:
                observation["inFlightSamples"] += 1
            else:
                observation["settledSamples"] += 1
                require(exact_failure(current) == retained,
                        "A settled navigation changed the retained failure")
            if elapsed >= 12:
                break
            time.sleep(0.2)
        final_settled = poll(lambda: own_tab(tab),
                             lambda value: not value["loading"] and not value["navigation"]["provisional"],
                             "Failed navigation did not settle after the observation window")
        report[key + "FinalSettledState"] = final_settled
        require(exact_failure(final_settled) == retained,
                "Final settled navigation changed the retained failure")
        repeated = operation(task, "wait", tab, "dom", 1000,
                             expected="NAVIGATION_FAILED", receipt=key + "RepeatedWait")
        require(repeated["error"]["details"]["state"]["navigation"]["error"] == retained,
                "Repeated readiness wait changed the retained failure")

    try:
        status = call("status")
        require(status.get("engine") == "CEF/Chromium" and status.get("windowMode") == "single",
                "Expected an already running single-window CEF app")
        initial = tab_ids()
        require(len(initial) == 1 and status["activeTab"] in initial and
                status["activeSpace"] == "user" and not status["overview"],
                "Keep exactly one original user tab selected before this exclusive test")
        require(counters_idle(status), "Start without concurrent waits, captures, creation or downloads")
        baseline = (status["activeSpace"], status["activeTab"], status["overview"])
        report["initialTabIds"] = sorted(initial)
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler(state))
        server.daemon_threads = False
        server.block_on_close = True
        server_thread = threading.Thread(target=server.serve_forever)
        server_thread.start()
        origin = f"http://127.0.0.1:{server.server_port}"
        unstable = origin + "/unstable"

        user_tab = open_owned(origin + "/ok")
        complete(user_tab, origin + "/ok", "Naru Recovery OK", key="userInitialLoad")
        report["checks"].append("owned user fixture completes with its expected title")
        request_navigation("navigate", user_tab, None, unstable)
        failed(user_tab, unstable, key="userFailure")
        require(state.snapshot()["failures"] > 0, "Fixture did not observe a denied attempt")
        report["checks"].append("user navigation retains -324 after loading ends and repeated readiness waits")

        with state.lock:
            state.allow = True
        request_navigation("reload", user_tab)
        complete(user_tab, unstable, "Naru Recovery Restored", key="userReloadComplete")
        report["checks"].append("unscoped CLI reload restores the same user tab and clears its error")
        call("close", user_tab)
        poll(tab_ids, lambda ids: user_tab not in ids, "Owned user tab did not close")
        preserve_user()

        with state.lock:
            state.allow = False
            failed_before = state.failures
        task = call("task", "create", "Navigation recovery fixture")["task"]
        tasks[task["id"]] = task
        require(task["status"] == "active" and task["revision"] == 1 and not task["tabs"],
                "New recovery task was not empty and active at revision 1")
        ai_tab = open_owned(unstable, task)
        preserve_owner(task, ai_tab, "aiAfterOpen")
        failed(ai_tab, unstable, task, "aiFailure")
        require(state.snapshot()["failures"] > failed_before, "AI fixture failure was not requested")
        preserve_owner(task, ai_tab, "aiAfterFailure")
        report["checks"].append("background AI failure preserves the user's selection and active task revision")
        with state.lock:
            state.allow = True
        request_navigation("reload", ai_tab, task)
        preserve_owner(task, ai_tab, "aiAfterReloadRequest")
        complete(ai_tab, unstable, "Naru Recovery Restored", task, "aiReloadComplete")
        preserve_owner(task, ai_tab, "aiAfterRecovery")
        report["checks"].append("scoped CLI reload restores the same AI tab without handoff or user selection changes")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, ValueError, TimeoutError) as error:
        report["error"] = str(error)
        report["lastOwnedResponse"] = last_owned_response
    finally:
        cleanup_errors = []
        for task in tasks.values():
            try:
                current = current_task(task)
                require(current["id"] == task["id"], "Refusing to finish a different task")
                task_tab_ids = [item["tab"] for item in current["tabs"]] + current["closingTabs"]
                require(all(type(tab) is int for tab in task_tab_ids),
                        "Refusing to finish a task with invalid tab IDs")
                # Do not trust a failed task/open assertion to imply ownership.
                # Finishing a task can close every tab it owns.
                require(set(task_tab_ids) <= owned and not (set(task_tab_ids) & initial),
                        "Refusing to finish a task containing an initial or unowned tab")
                if current["status"] != "finished":
                    call("task", "finish", current["id"], current["revision"])
                current = poll(lambda: current_task(task),
                               lambda value: value["status"] == "finished" and
                               not value["tabs"] and not value["closingTabs"],
                               "Owned task did not finish and close its tabs")
                report["finishedTask"] = {"status": current["status"],
                                          "revision": current["revision"],
                                          "tabs": current["tabs"], "closingTabs": current["closingTabs"]}
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                cleanup_errors.append(str(error))
        if owned:
            try:
                remaining = owned & tab_ids()
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                remaining = set()
                cleanup_errors.append(str(error))
            for tab in sorted(remaining):
                try:
                    require(tab not in initial, "Refusing to close an initial tab")
                    call("close", tab)
                except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                    cleanup_errors.append(str(error))
            try:
                poll(tab_ids, lambda ids: not (ids & owned), "Owned fixture tabs did not close")
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                cleanup_errors.append(str(error))
        if baseline:
            try:
                preserve_user()
                final_status = call("status")
                require(counters_idle(final_status), "Pending resources remain after fixture cleanup")
                report["cleanup"] = {"initialTabIds": sorted(initial), "finalTabIds": sorted(tab_ids()),
                                     "activeSpace": final_status["activeSpace"],
                                     "activeTab": final_status["activeTab"], "pendingCountersZero": True}
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                cleanup_errors.append(str(error))
        state.stop.set()
        if server:
            try:
                server.shutdown()
                if server_thread:
                    server_thread.join(timeout=5)
                server.server_close()
                require(not server_thread or not server_thread.is_alive(), "Fixture server thread did not stop")
                report["fixtureServerStopped"] = True
            except (OSError, RuntimeError) as error:
                cleanup_errors.append(str(error))
        report["fixtureState"] = state.snapshot()
        report["cleanupVerified"] = bool(baseline and server and owned) and not cleanup_errors
        if report["cleanupVerified"]:
            report["checks"].append("only owned task and tabs closed; original user selection and idle resources preserved; server stopped")
        if cleanup_errors:
            report["cleanupErrors"] = cleanup_errors
        report["ok"] = (report["ok"] and report["cleanupVerified"] and
                        len(report["checks"]) == EXPECTED_CHECKS)
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
