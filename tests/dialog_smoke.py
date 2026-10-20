#!/usr/bin/env python3
"""Verify JavaScript dialog handling for AI-owned and user-space tabs through a running app.

Example:
  python3 tests/dialog_smoke.py --cli target/release/lightbrowser \
      --data-dir "$LIGHT_BROWSER_DATA_DIR"

Does not launch the app or create profiles. Only the localhost fixture
tests/fixtures/dialogs.html and tabs/tasks created here are used; the app's
initial tab must stay open. Run without other concurrent CLI tests.

Covers: queueing of alert/confirm/prompt/beforeunload in AI tabs, accept and
dismiss, DIALOG_PENDING fast failures, overflow auto-dismiss, cleanup on
handoff/close/finish, and that a user-space tab is NOT queued. The native
dialog of the user-space tab is not observable here; see docs/dialog-control.md.
"""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
import signal
from pathlib import Path
import sys
import threading
import time

from smoke import BrowserCli, CheckFailed, ROOT, element, require

FIXTURE = ROOT / "tests/fixtures/dialogs.html"


class Handler(BaseHTTPRequestHandler):
    body = b""

    def do_GET(self):
        if self.path.startswith("/favicon"):
            self.send_response(204)
            self.end_headers()
            return
        body = b"<!doctype html><title>Plain</title><p>PLAIN_PAGE</p>" if self.path.startswith("/plain") else Handler.body
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


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    checks = []
    report = {"ok": False, "scope": "JS dialogs through real CEF CLI and a localhost fixture", "checks": checks}
    initial, owned, tasks = set(), set(), []
    server = server_thread = None

    def tab_states():
        return {state["tab"]: state for state in cli.success("tabs")["tabs"]}

    def wait_until(description, predicate, seconds=10):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            value = predicate()
            if value:
                return value
            time.sleep(0.05)
        raise CheckFailed(f"timed out: {description}")

    def make_task(name):
        task = cli.success("task", "create", name)["task"]
        tasks.append(task)
        return task

    def run_in(task, *command):
        return cli.request("task", "run", task["id"], task["revision"], *command)

    def ok_in(task, *command):
        response = run_in(task, *command)
        require(response["ok"], f"task run {command}: {json.dumps(response, ensure_ascii=False)[:400]}")
        return response["result"]

    def code_in(task, expected, *command):
        started = time.monotonic()
        response = run_in(task, *command)
        elapsed = time.monotonic() - started
        code = response.get("error", {}).get("code")
        require(not response["ok"] and code == expected,
                f"{command}: expected {expected}, got {json.dumps(response, ensure_ascii=False)[:400]}")
        return response["error"], elapsed

    def open_tab(task, path="/dialogs"):
        tab = ok_in(task, "open", origin + path)["tab"]
        owned.add(tab)
        ok_in(task, "wait", tab, "load", 10000)
        return tab

    def read_retry(task, tab):
        # Right after a dialog is answered the tab may briefly report TAB_BUSY while
        # the input focus cleanup is acknowledged by the resumed renderer.
        deadline = time.monotonic() + 8
        while True:
            response = run_in(task, "read", tab)
            if response["ok"] or response["error"]["code"] not in ("TAB_BUSY", "PAGE_LOADING") or time.monotonic() > deadline:
                require(response["ok"], f"read: {json.dumps(response, ensure_ascii=False)[:300]}")
                return response["result"]
            time.sleep(0.05)

    def refs(task, tab):
        page = read_retry(task, tab)["page"]
        return {label: element(page, label)["ref"] for label in
                ("Alert now", "Alert in timer", "Confirm", "Prompt", "Arm beforeunload",
                 "Disarm beforeunload", "Plain button")}

    def result_text(task, tab):
        text = read_retry(task, tab)["page"]["text"]
        marker = text.find("RESULT ")
        return text[marker:].splitlines()[0] if marker >= 0 else text[:120]

    def wait_result(task, tab, expected):
        wait_until(f"page result {expected!r}", lambda: result_text(task, tab) == f"RESULT {expected}")

    def dialog_of(tab):
        return tab_states().get(tab, {}).get("dialog")

    def wait_dialog(tab, kind=None):
        dialog = wait_until(f"dialog on tab {tab}", lambda: dialog_of(tab))
        if kind:
            require(dialog["type"] == kind, f"expected {kind}, got {dialog}")
        return dialog

    def click(task, tab, label):
        # Refs are only valid for the latest read, so look the button up right before clicking.
        return run_in(task, "click", tab, refs(task, tab)[label])

    try:
        status = cli.success("status")
        require(status.get("engine") == "CEF/Chromium", "Expected the real CEF app")
        require(status["dialogs"]["pending"] == 0, "Start without pending dialogs")
        initial = set(tab_states())
        baseline = (status["activeSpace"], status["activeTab"], status["overview"])
        Handler.body = FIXTURE.read_bytes()
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        origin = f"http://127.0.0.1:{server.server_port}"

        task = make_task("Dialog smoke A")
        tab = open_tab(task)
        buttons = refs(task, tab)

        # 1. Queued alert from a timer; blocked page commands fail fast.
        receipt = click(task, tab, "Alert in timer")
        require(receipt["ok"], f"timer click failed: {receipt}")
        dialog = wait_dialog(tab, "alert")
        require(dialog["message"] == "TIMER_ALERT" and dialog["origin"] == origin and
                dialog["defaultPrompt"] == "", f"unexpected dialog: {dialog}")
        status = cli.success("status")
        require(status["dialogs"]["pending"] == 1 and status["dialogs"]["requests"] == [{"tab": tab, "type": "alert"}],
                f"status.dialogs: {status['dialogs']}")
        require((status["activeSpace"], status["activeTab"], status["overview"]) == baseline,
                "A queued dialog changed the user's space/selection")
        for command in (("read", tab), ("click", tab, buttons["Plain button"]),
                        ("wait", tab, "text", "never", 5000), ("wait", tab, "load", 5000),
                        ("fill", tab, buttons["Plain button"], "x"), ("reload", tab)):
            error, elapsed = code_in(task, "DIALOG_PENDING", *command)
            require(elapsed < 3, f"{command[0]} should fail fast, took {elapsed:.1f}s")
            require(error["details"]["dialog"]["type"] == "alert", "DIALOG_PENDING must carry the dialog")
        checks.append("AI-tab alert is queued, visible in tabs/status, selection unchanged; read/click/wait/fill/reload fail fast with DIALOG_PENDING")

        # 2. Authorization and validation do not consume the dialog.
        unscoped = cli.request("dialog", tab, "accept")
        require(not unscoped["ok"] and unscoped["error"]["code"] == "TASK_REQUIRED", f"unscoped: {unscoped}")
        text_error = run_in(task, "dialog", tab, "accept", "typed")
        require(not text_error["ok"] and text_error["error"]["code"] == "DIALOG_TEXT_NOT_ALLOWED", f"text: {text_error}")
        stale = cli.request("task", "run", task["id"], task["revision"] + 5, "dialog", tab, "dismiss")
        require(not stale["ok"], "A stale revision must be rejected")
        other = make_task("Dialog smoke foreign")
        foreign = run_in(other, "dialog", tab, "dismiss")
        require(not foreign["ok"] and foreign["error"]["code"] == "TASK_TAB_MISMATCH", f"foreign: {foreign}")
        require(dialog_of(tab) is not None, "Rejected requests must not consume the dialog")
        cli.success("task", "finish", other["id"], other["revision"])
        tasks.remove(other)
        checks.append("dialog requires task run, owner and revision; TEXT on alert is DIALOG_TEXT_NOT_ALLOWED; dialog stays queued")

        # 3. Accept lets the page continue.
        answered = ok_in(task, "dialog", tab, "accept")
        require(answered["action"] == "accept" and answered["dialog"]["type"] == "alert", f"answer: {answered}")
        wait_result(task, tab, "timer-alert-done")
        require(dialog_of(tab) is None and cli.success("status")["dialogs"]["pending"] == 0, "dialog not cleared")
        none, _ = code_in(task, "NO_DIALOG", "dialog", tab, "dismiss")
        checks.append("accept resumes the page (DOM shows timer-alert-done); a second answer is NO_DIALOG")

        # 4. alert triggered by the click itself: the in-flight click must not hang.
        started = time.monotonic()
        response = click(task, tab, "Alert now")
        elapsed = time.monotonic() - started
        require(elapsed < 8, f"click that opens an alert hung for {elapsed:.1f}s")
        if not response["ok"]:
            require(response["error"]["code"] == "DIALOG_PENDING", f"click: {response}")
            in_flight = "DIALOG_PENDING"
        else:
            in_flight = "ok"
        wait_dialog(tab, "alert")
        ok_in(task, "dialog", tab, "accept")
        wait_result(task, tab, "alert-done")
        checks.append(f"click that opens alert returns promptly ({in_flight}, {elapsed:.2f}s) and accept resumes it")

        # 5. confirm dismiss/accept.
        click(task, tab, "Confirm")
        dialog = wait_dialog(tab, "confirm")
        require(dialog["message"] == "CONFIRM_MESSAGE", f"confirm: {dialog}")
        ok_in(task, "dialog", tab, "dismiss")
        wait_result(task, tab, "confirm:false")
        click(task, tab, "Confirm")
        wait_dialog(tab, "confirm")
        ok_in(task, "dialog", tab, "accept")
        wait_result(task, tab, "confirm:true")
        checks.append("confirm dismiss -> DOM confirm:false; accept -> confirm:true")

        # 6. prompt text.
        click(task, tab, "Prompt")
        dialog = wait_dialog(tab, "prompt")
        require(dialog["message"] == "PROMPT_MESSAGE" and dialog["defaultPrompt"] == "DEFAULT_VALUE", f"prompt: {dialog}")
        answered = ok_in(task, "dialog", tab, "accept", "텍스트 TEXT")
        require(answered.get("text") == "텍스트 TEXT", f"prompt answer: {answered}")
        wait_result(task, tab, "prompt:텍스트 TEXT")
        click(task, tab, "Prompt")
        wait_dialog(tab, "prompt")
        ok_in(task, "dialog", tab, "accept")
        wait_result(task, tab, "prompt:DEFAULT_VALUE")
        click(task, tab, "Prompt")
        wait_dialog(tab, "prompt")
        ok_in(task, "dialog", tab, "dismiss")
        wait_result(task, tab, "prompt:null")
        checks.append("prompt accept TEXT -> DOM shows TEXT; accept without TEXT -> default; dismiss -> null")

        # 7. beforeunload on navigation: queued, dismiss stays, accept leaves.
        click(task, tab, "Arm beforeunload")
        wait_result(task, tab, "unload-armed")
        ok_in(task, "navigate", tab, origin + "/plain")
        dialog = wait_dialog(tab, "beforeunload")
        require(dialog["origin"] == origin, f"beforeunload origin: {dialog}")
        ok_in(task, "dialog", tab, "dismiss")
        time.sleep(0.5)
        require(tab_states()[tab]["url"].endswith("/dialogs"), "dismissing beforeunload must keep the page")
        ok_in(task, "navigate", tab, origin + "/plain")
        wait_dialog(tab, "beforeunload")
        ok_in(task, "dialog", tab, "accept")
        wait_until("navigation after accepting beforeunload",
                   lambda: tab_states()[tab]["url"].endswith("/plain"))
        checks.append("beforeunload on navigate is queued; dismiss stays on the page; accept leaves")
        ok_in(task, "navigate", tab, origin + "/dialogs")
        ok_in(task, "wait", tab, "text", "Dialog fixture", 10000)
        buttons = refs(task, tab)

        # 8. close with a pending beforeunload and with a pending alert completes.
        for kind, label in (("beforeunload", "Arm beforeunload"), ("alert", "Alert in timer")):
            victim = open_tab(task)
            if kind == "beforeunload":
                ok_in(task, "click", victim, refs(task, victim)[label])
                wait_result(task, victim, "unload-armed")
                ok_in(task, "close", victim)  # triggers beforeunload; auto-accepted for explicit close
            else:
                ok_in(task, "click", victim, refs(task, victim)[label])
                wait_dialog(victim, "alert")
                ok_in(task, "close", victim)
            wait_until(f"tab closes with {kind} pending", lambda: victim not in tab_states(), 10)
            owned.discard(victim)
            require(cli.success("status")["dialogs"]["pending"] == 0, "pending dialog leaked after close")
        checks.append("task-run close completes with a pending beforeunload or alert; no dialog leaks")

        # 9. handoff dismisses queued dialogs; commands then need resume.
        tab2 = open_tab(task)
        refs2 = refs(task, tab2)
        ok_in(task, "click", tab2, refs2["Alert in timer"])
        wait_dialog(tab2, "alert")
        handed = cli.success("task", "handoff", task["id"], task["revision"])["task"]
        task = dict(task, revision=handed["revision"])
        tasks[0] = task
        wait_until("handoff dismissal", lambda: dialog_of(tab2) is None)
        require(cli.success("status")["dialogs"]["pending"] == 0, "handoff left a dialog queued")
        resumed = cli.success("task", "resume", task["id"], task["revision"])["task"]
        task = dict(task, revision=resumed["revision"])
        tasks[0] = task
        wait_result(task, tab2, "timer-alert-done")
        checks.append("handoff dismisses the queued alert (page continued: timer-alert-done)")

        # 10. global cap of 4 and overflow auto-dismiss.
        second = make_task("Dialog smoke B")
        capped = [tab2]
        # task A already has tab and tab2; add two more, then one tab in task B.
        extra_a = [open_tab(task) for _ in range(2)]
        capped += extra_a
        capped_task = {tab: task for tab in [tab, tab2, *extra_a]}
        fifth = open_tab(second)
        capped_task[fifth] = second
        first_four = [tab2, *extra_a, tab]
        for item in first_four:
            ok_in(capped_task[item], "click", item, refs(capped_task[item], item)["Alert in timer"])
        wait_until("four pending dialogs", lambda: cli.success("status")["dialogs"]["pending"] == 4)
        overflow_base = cli.success("status")["dialogs"]["overflow"]
        fifth_refs = refs(second, fifth)
        ok_in(second, "click", fifth, fifth_refs["Alert in timer"])
        wait_until("overflow recorded", lambda: cli.success("status")["dialogs"]["overflow"] >= overflow_base + 1)
        require(cli.success("status")["dialogs"]["pending"] == 4, "Overflow must not exceed four pending dialogs")
        require(tab_states()[fifth]["dialogOverflow"] >= 1 and dialog_of(fifth) is None, "overflow not on the fifth tab")
        wait_result(second, fifth, "timer-alert-done")  # auto-dismissed alert let the page continue
        for item in first_four:
            ok_in(capped_task[item], "dialog", item, "accept")
        wait_until("all dialogs answered", lambda: cli.success("status")["dialogs"]["pending"] == 0)
        checks.append("5th dialog beyond the global cap of 4 is auto-dismissed (overflow counted) and its page continues")

        # 11. finish with a pending beforeunload completes.
        for item in (tab, tab2, *extra_a):
            owned.add(item)
        armed = extra_a[0]
        ok_in(task, "click", armed, refs(task, armed)["Arm beforeunload"])
        wait_result(task, armed, "unload-armed")
        ok_in(task, "click", extra_a[1], refs(task, extra_a[1])["Alert in timer"])
        wait_dialog(extra_a[1], "alert")
        finished = cli.success("task", "finish", task["id"], task["revision"])
        require(finished["closeRequested"], f"finish: {finished}")
        wait_until("task A tabs closed", lambda: not ({tab, tab2, *extra_a} & set(tab_states())), 15)
        owned -= {tab, tab2, *extra_a}
        info = cli.success("task", "info", task["id"])["task"]
        require(info["status"] == "finished", f"task did not finish: {info['status']}")
        require(cli.success("status")["dialogs"]["pending"] == 0, "dialog leaked after finish")
        tasks.remove(task)
        checks.append("task finish with a pending alert and an armed beforeunload completes (tabs closed, task finished)")
        cli.success("task", "finish", second["id"], second["revision"])
        tasks.remove(second)
        owned.discard(fifth)

        # 12. user-space tab is NOT queued (CEF default). Native dialog itself is not observable.
        overflow_before = cli.success("status")["dialogs"]["overflow"]
        user_tab = cli.success("open", origin + "/dialogs")["tab"]
        owned.add(user_tab)
        cli.success("wait", user_tab, "load", 10000)
        page = cli.success("read", user_tab)["page"]
        cli.success("click", user_tab, element(page, "Alert in timer")["ref"])
        time.sleep(1.2)
        status = cli.success("status")
        require(status["dialogs"]["pending"] == 0 and status["dialogs"]["overflow"] == overflow_before,
                "user tab dialog was queued")
        require(tab_states()[user_tab]["dialog"] is None, "user tab must not report a queued dialog")
        report["userTabNativeDialog"] = "not observable by CLI; queue stayed empty (default CEF path)"
        closed = cli.request("close", user_tab)
        report["userTabCloseRequest"] = closed["ok"]
        try:
            wait_until("user tab with native dialog closes", lambda: user_tab not in tab_states(), 5)
            owned.discard(user_tab)
            report["userTabClosed"] = True
        except CheckFailed:
            report["userTabClosed"] = False
        checks.append("user-space alert is not queued (status.dialogs.pending 0, tabs[].dialog null)")
        final = cli.success("status")
        require((final["activeSpace"], final["activeTab"], final["overview"]) == baseline,
                "The dialog checks changed the user's space/selection")
        if args.quit_pid:
            # Quit path: an AI tab with an armed beforeunload and another with a pending
            # alert must not keep the app from exiting after SIGTERM (close_all).
            quit_task = make_task("Dialog quit")
            armed_tab = open_tab(quit_task)
            ok_in(quit_task, "click", armed_tab, refs(quit_task, armed_tab)["Arm beforeunload"])
            wait_result(quit_task, armed_tab, "unload-armed")
            alert_tab = open_tab(quit_task)
            ok_in(quit_task, "click", alert_tab, refs(quit_task, alert_tab)["Alert in timer"])
            wait_dialog(alert_tab, "alert")
            os.kill(args.quit_pid, signal.SIGTERM)
            deadline = time.monotonic() + 10
            exited = False
            while time.monotonic() < deadline:
                try:
                    os.kill(args.quit_pid, 0)
                except ProcessLookupError:
                    exited = True
                    break
                time.sleep(0.1)
            report["quitExitedWithinSeconds"] = exited
            require(exited, "App did not exit within 10s of SIGTERM with queued dialogs")
            owned.clear()
            tasks.clear()
            checks.append("SIGTERM with an armed beforeunload tab and a pending alert exits the app")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
        report["error"] = str(error)
    finally:
        cleanup = []
        for item in list(tasks):
            try:
                cli.request("task", "finish", item["id"], cli.success("task", "info", item["id"])["task"]["revision"])
            except (CheckFailed, KeyError, TypeError) as error:
                cleanup.append(f"task {item.get('id')}: {error}")
        for item in sorted(owned):
            try:
                cli.request("close", item)
            except CheckFailed as error:
                cleanup.append(str(error))
        if owned:
            try:
                deadline = time.monotonic() + 6
                while owned & set(tab_states()) and time.monotonic() < deadline:
                    time.sleep(0.1)
                left = owned & set(tab_states())
                if left:
                    cleanup.append(f"tabs still open: {sorted(left)}")
            except (CheckFailed, KeyError, TypeError) as error:
                cleanup.append(str(error))
        if cleanup:
            report["cleanupErrors"] = cleanup
        if server:
            server.shutdown()
            server.server_close()
        if server_thread:
            server_thread.join(timeout=2)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--cli", type=Path, default=ROOT / "target/release/lightbrowser")
    parser.add_argument("--data-dir", type=Path, default=os.environ.get("LIGHT_BROWSER_DATA_DIR"))
    parser.add_argument("--quit-pid", type=int, help="app PID; adds a final SIGTERM-with-dialogs quit check (the app exits)")
    args = parser.parse_args()
    if args.data_dir is None:
        parser.error("--data-dir or LIGHT_BROWSER_DATA_DIR is required")
    report = run(args)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
