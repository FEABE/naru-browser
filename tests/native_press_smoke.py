#!/usr/bin/env python3
"""Prepared local CEF check for keyboard dispatch and observable page effects.

Requires an already-running compatible app and exclusive CLI access. Uses one
owned task/tab and localhost fixture, preserving the user's selected space/tab.
Does not launch an app, force window focus, retry key actions or submit externally.
Syntax checking is not a CEF result. Focus-redirection coverage requires its actual
focus handler to run; dispatch acknowledgement alone never proves the page effect.
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


FIXTURE = ROOT / "tests/fixtures/native_press.html"


def receipt(page):
    match = re.search(r"NATIVE_PRESS_STATE_BEGIN (\S+) NATIVE_PRESS_STATE_END", page.get("text", ""))
    require(match is not None, "Native Enter receipt is missing or truncated")
    return json.loads(unquote(match.group(1)))


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {"ok": False, "scope": "local CEF keyboard input and background selection", "checks": []}
    initial, owned = set(), set()
    task = baseline = server = server_thread = last_response = None
    server_started = False

    def call(*command, expected=None):
        nonlocal last_response
        last_response = cli.request(*command)
        if expected:
            require(last_response["ok"] is False and last_response.get("error", {}).get("code") == expected,
                    f"Expected {expected}, received {json.dumps(last_response, ensure_ascii=False)}")
            return last_response
        require(last_response["ok"] is True, f"CLI failed: {json.dumps(last_response, ensure_ascii=False)}")
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
                "Native Enter changed the user's selected space/tab")
        require(initial <= ids(), "An initial tab disappeared")

    try:
        status = call("status")
        require(status.get("engine") == "CEF/Chromium" and status["activeSpace"] == "user" and
                status["overview"] is False, "Use an idle compatible app with a user tab selected")
        initial = ids()
        require(status["activeTab"] in initial, "Keep an initial user tab selected")
        baseline = (status["activeSpace"], status["activeTab"], status["overview"])
        require(status["pendingWaits"] == 0 and status["resources"]["pendingTabCreations"] == 0,
                "Run without concurrent waits or tab creation")
        path = f"/native-press-{uuid.uuid4().hex}.html"
        server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(FIXTURE.read_bytes(), path))
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        server_started = True
        task = call("task", "create", "Native Enter fixture")["task"]
        tab = scoped("open", f"http://127.0.0.1:{server.server_port}{path}")["tab"]
        require(isinstance(tab, int) and tab not in initial, "Fixture reused an initial tab")
        owned.add(tab)
        scoped("wait", tab, "load", 10000)

        def read():
            page = scoped("read", tab)["page"]
            require(page.get("title") == "Naru native Enter fixture", "Unexpected fixture")
            return page

        def press(label, target):
            preserved()
            page = read()
            before = receipt(page)
            result = scoped("press", tab, element(page, label)["ref"], "Enter")
            require_native_press_ack(result["page"])
            # Poll observations only. Never repeat a key action if its effect is late.
            after = receipt(poll(read, lambda value: receipt(value)["counts"]["keyup"] > before["counts"]["keyup"],
                                 "Enter acknowledgement did not produce an observable keyup"))
            require(all(after["counts"][kind] == before["counts"][kind] + 1 for kind in ("keydown", "keyup")),
                    "Enter did not produce exactly one keydown/keyup pair")
            events = [event for event in after["events"] if event["sequence"] > before["sequence"]]
            require(all(event["trusted"] is True for event in events),
                    "The fixture observed an untrusted event in the native key/default sequence")
            key_events = [event for event in events if event["type"] in ("keydown", "keypress", "keyup")]
            require(key_events and all(event["target"] == target and event["key"] == "Enter" and
                                       event["trusted"] is True for event in key_events),
                    "Native key events reached a different target or had unexpected trust")
            preserved()
            return before, after, events

        poll(read, lambda page: receipt(page)["childReady"] is True, "Frame fixture did not become ready")
        preserved()
        for label, target in (("Enter button", "button"), ("Shadow Enter button", "shadow")):
            before, after, events = press(label, target)
            require(after["clicks"][target] == before["clicks"][target] + 1 and
                    after["counts"]["click"] == before["counts"]["click"] + 1,
                    "Enter did not activate the requested button exactly once")
            require(any(event["type"] == "click" and event["target"] == target and event["trusted"] is True
                        for event in events), "Button activation was not observed as a native default action")
        report["checks"].append("main/open-shadow buttons activate once; acknowledgement checked separately from page effects")

        for label, target in (("Enter textarea", "textarea"), ("Frame Enter textarea", "frame")):
            before, after, events = press(label, target)
            require(before["states"][target]["value"] == "" and after["states"][target]["value"] == "\n",
                    "Native textarea Enter did not insert one LF")
            require([(event["type"], event["inputType"]) for event in events if event["type"] in ("beforeinput", "input")] ==
                    [("beforeinput", "insertLineBreak"), ("input", "insertLineBreak")],
                    "Textarea native editing events were missing or duplicated")
        before, after, events = press("Enter editor", "editable")
        require(before["states"]["editable"]["html"] == "" and after["states"]["editable"]["html"] != "" and
                any(event["type"] == "input" and event["inputType"] == "insertParagraph" for event in events),
                "Contenteditable Enter did not produce its native paragraph effect")
        report["checks"].append("main/frame textarea LF and contenteditable paragraph effects follow native Enter")

        before, after, events = press("Enter form input", "query")
        require(after["counts"]["submit"] == before["counts"]["submit"] + 1 and after["submitter"] == "primary" and
                after["clicks"]["primary"] == before["clicks"]["primary"] + 1 and
                after["clicks"]["secondary"] == before["clicks"]["secondary"],
                "Implicit submission did not use the first submit button exactly once")
        require(any(event["type"] == "submit" and event["trusted"] is True for event in events),
                "Native form submission event was not observed")
        report["checks"].append("form uses the primary submitter; fixture prevents navigation and external submission")

        before, after, events = press("Canceled Enter", "cancel")
        require(after["states"]["cancel"]["value"] == "" and
                all(after["counts"][kind] == before["counts"][kind] for kind in ("keypress", "beforeinput", "input", "click", "submit")) and
                any(event["type"] == "keydown" and event["defaultPrevented"] is True for event in events),
                "Canceled keydown still produced a default action")
        report["checks"].append("keydown preventDefault suppresses editing/default actions without suppressing dispatch acknowledgement")

        preserved()
        page = read()
        before = receipt(page)
        response = scoped("press", tab, element(page, "Redirected Enter")["ref"], "Enter", expected="FOCUS_NOT_REACHED")
        details = response["error"].get("details")
        require(isinstance(details, dict) and details.get("attempted") is False and
                "trusted" not in details and "formSubmissionRequested" not in details,
                "Focus rejection must report boolean attempted:false without obsolete action claims")
        after = receipt(read())
        require(after["redirects"] == before["redirects"] + 1 and after["active"] == "other",
                "Focus-redirection handler did not run; background focus timing is not verified")
        require(after["counts"] == before["counts"] and after["states"]["redirect"]["value"] == "" and
                after["states"]["other"]["value"] == "", "Rejected focus still dispatched keys or editing")
        preserved()
        report["checks"].append("actual focus redirection rejects before key dispatch; every action preserves the user space/tab")

        # Each action starts in the fixture's declared state. Only observations
        # are polled; an uncertain key action is never dispatched a second time.
        cases = [
            ("Tab", "Key first", "key-first", "Key second", {}),
            ("Shift+Tab", "Key second", "key-second", "Key first", {}),
            ("ArrowLeft", "Key edit", "key-edit", "Key edit", {"start": 1, "end": 1}),
            ("ArrowRight", "Key edit", "key-edit", "Key edit", {"start": 3, "end": 3}),
            ("Home", "Key edit", "key-edit", "Key edit", {"start": 0, "end": 0}),
            ("End", "Key edit", "key-edit", "Key edit", {"start": 4, "end": 4}),
            ("Backspace", "Key edit", "key-edit", "Key edit", {"value": "acd"}),
            ("Delete", "Key edit", "key-edit", "Key edit", {"value": "abd"}),
            ("Space", "Key checkbox", "key-check", "Key checkbox", {"checked": True}),
            ("Escape", "Key popup", "key-popup", "Key popup", {"expanded": "false"}),
            ("ArrowUp", "Key select", "key-select", "Key select", {"value": "a"}),
            ("ArrowDown", "Key select", "key-select", "Key select", {"value": "c"}),
            ("PageUp", "Key select", "key-select", "Key select", {"value": "a"}),
            ("PageDown", "Key select", "key-select", "Key select", {"value": "d"}),
            ("Enter", "Enter textarea", "textarea", "Enter textarea", {"value": "\n"}),
            ("Shift+Enter", "Enter textarea", "textarea", "Enter textarea", {"value": "\n"}),
            ("Shift+ArrowLeft", "Key edit", "key-edit", "Key edit", {"start": 1, "end": 2}),
            ("Shift+ArrowRight", "Key edit", "key-edit", "Key edit", {"start": 2, "end": 3}),
            ("Shift+Home", "Key edit", "key-edit", "Key edit", {"start": 0, "end": 2}),
            ("Shift+End", "Key edit", "key-edit", "Key edit", {"start": 2, "end": 4}),
            *[(key, "Key lines", "key-lines", "Key lines", {"selection": True}) for key in
              ("Shift+ArrowUp", "Shift+ArrowDown", "Shift+PageUp", "Shift+PageDown")],
            ("Backspace", "Key canceled deletion", "key-cancel", "Key canceled deletion", {"value": "abcd"}),
        ]
        def press_key(key, label, target, focused_label, expected, *, fresh=True):
            preserved()
            if fresh:
                scoped("reload", tab)
                scoped("wait", tab, "load", 10000)
            page = poll(read, lambda value: receipt(value)["childReady"] is True,
                        "Reloaded key fixture did not become ready")
            before = receipt(page)
            result = scoped("press", tab, element(page, label)["ref"], key)
            require_native_press_ack(result["page"], key)
            page = poll(read, lambda value: receipt(value)["counts"]["keyup"] > before["counts"]["keyup"],
                        f"{key}: no observable keyup after acknowledgement")
            after = receipt(page)
            events = [event for event in after["events"] if event["sequence"] > before["sequence"]]
            key_events = [event for event in events if event["type"] in ("keydown", "keyup")]
            parts = key.split("+")
            base_key, modifiers = parts[-1], set(parts[:-1])
            event_key = {"Space": " ", "A": "a", "Z": "Z" if "Shift" in modifiers else "z"}.get(base_key, base_key)
            code = f"Key{base_key}" if base_key in ("A", "Z") else base_key
            require([event["type"] for event in key_events] == ["keydown", "keyup"] and
                    key_events[0]["target"] == target and key_events[1]["target"] == after["active"] and all(
                        event["key"] == event_key and event["code"] == code and
                        event["shift"] is ("Shift" in modifiers) and event["alt"] is ("Alt" in modifiers) and
                        event["meta"] is ("Meta" in modifiers) and event["ctrl"] is False and
                        event["trusted"] is True
                        for event in key_events), f"{key}: wrong event pair or payload")
            require(all(event["trusted"] is True for event in events), f"{key}: untrusted editing/default event")
            require(result["page"].get("keydownTarget") == "matched", f"{key}: original keydown target was not confirmed")
            require(page.get("focusedRef") == element(page, focused_label)["ref"],
                    f"{key}: read did not identify the resulting focused element")
            state = after["states"][target]
            for field, value in expected.items():
                if field == "selection":
                    require(state["start"] < state["end"], f"{key}: no native text selection")
                else:
                    require(state[field] == value, f"{key}: unexpected {field}: {state[field]!r}")
            preserved()
            return before, after, events

        for case in cases:
            press_key(*case)
        report["checks"].append("24 key forms and canceled deletion: native effects, focused refs, user selection preserved")

        # The fixture uses an unwrapped, short ASCII document and one ASCII
        # word. These explicit command boundaries do not assert OS-specific
        # word segmentation for punctuation, languages or bidirectional text.
        editing_cases = [
            ("Meta+A", "Key edit", "key-edit", "Key edit", {"start": 0, "end": 4}),
            ("Meta+ArrowLeft", "Shortcut document", "key-document", "Shortcut document", {"start": 11, "end": 11}),
            ("Meta+ArrowRight", "Shortcut document", "key-document", "Shortcut document", {"start": 22, "end": 22}),
            ("Meta+ArrowUp", "Shortcut document", "key-document", "Shortcut document", {"start": 0, "end": 0}),
            ("Meta+ArrowDown", "Shortcut document", "key-document", "Shortcut document", {"start": 28, "end": 28}),
            ("Meta+Shift+ArrowLeft", "Shortcut document", "key-document", "Shortcut document", {"start": 11, "end": 17}),
            ("Meta+Shift+ArrowRight", "Shortcut document", "key-document", "Shortcut document", {"start": 17, "end": 22}),
            ("Meta+Shift+ArrowUp", "Shortcut document", "key-document", "Shortcut document", {"start": 0, "end": 17}),
            ("Meta+Shift+ArrowDown", "Shortcut document", "key-document", "Shortcut document", {"start": 17, "end": 28}),
            ("Meta+Backspace", "Shortcut document", "key-document", "Shortcut document", {"value": "alpha beta\ndelta\nomega"}),
            ("Alt+ArrowLeft", "Shortcut word", "key-word", "Shortcut word", {"start": 0, "end": 0}),
            ("Alt+ArrowRight", "Shortcut word", "key-word", "Shortcut word", {"start": 8, "end": 8}),
            ("Alt+Shift+ArrowLeft", "Shortcut word", "key-word", "Shortcut word", {"start": 0, "end": 4}),
            ("Alt+Shift+ArrowRight", "Shortcut word", "key-word", "Shortcut word", {"start": 4, "end": 8}),
            ("Alt+Backspace", "Shortcut word", "key-word", "Shortcut word", {"value": "abet"}),
            ("Alt+Delete", "Shortcut word", "key-word", "Shortcut word", {"value": "alph"}),
        ]
        for case in editing_cases:
            before, after, events = press_key(*case)
            if "value" in case[4]:
                require(after["counts"]["input"] > before["counts"]["input"],
                        f"{case[0]}: deletion changed no observable native input state")
        report["checks"].append("SelectAll, line/document/word navigation and selection, line/word deletion verified by native state")

        for key in ("Meta+A", "Alt+Backspace"):
            before, after, events = press_key(key, "Canceled shortcut", "key-command-cancel", "Canceled shortcut",
                                             {"value": "alphabet", "start": 4, "end": 4})
            require(all(after["counts"][kind] == before["counts"][kind] for kind in ("beforeinput", "input")) and
                    any(event["type"] == "keydown" and event["defaultPrevented"] is True for event in events),
                    f"{key}: canceled keydown still executed an editing command")
        report["checks"].append("preventDefault blocks explicit SelectAll and word deletion commands")

        scoped("reload", tab)
        scoped("wait", tab, "load", 10000)
        page = poll(read, lambda value: receipt(value)["childReady"] is True, "Undo fixture did not become ready")
        fill = scoped("fill", tab, element(page, "Shortcut undo")["ref"], "undo draft")["page"]
        require(fill.get("ok") is True and fill.get("input") == "chromium" and fill.get("attempted") is True and
                fill.get("verified") is True and fill.get("dispatchAcknowledged") is True and "trusted" not in fill,
                "Undo setup did not use verified Chromium text input")
        require(receipt(read())["states"]["key-undo"]["value"] == "undo draft", "Undo setup text was not inserted")
        preserved()
        for key, value in (("Meta+Z", ""), ("Meta+Shift+Z", "undo draft")):
            before, after, events = press_key(key, "Shortcut undo", "key-undo", "Shortcut undo", {"value": value}, fresh=False)
            require(after["counts"]["input"] > before["counts"]["input"], f"{key}: history operation emitted no native input")
        current = info()
        require(current["status"] == "active" and current["revision"] == task["revision"],
                "Keyboard operations changed the AI task's status or revision")
        report["checks"].append("native insertText Undo/Redo restores exact text without reload; all 42 keys preserve background task ownership and user selection")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError, ValueError, RuntimeError) as error:
        report.update(error=str(error), lastResponse=last_response)
    finally:
        errors = []
        if task:
            try:
                current = info()
                if current["status"] != "finished":
                    call("task", "finish", current["id"], current["revision"])
                poll(info, lambda value: value["status"] == "finished" and not value["tabs"] and not value["closingTabs"],
                     "Owned task did not finish closing")
                poll(ids, lambda value: not (value & owned), "Owned fixture tab did not close")
                preserved()
            except (CheckFailed, OSError, KeyError, TypeError, ValueError) as error:
                errors.append(str(error))
        if server:
            for close in ((server.shutdown, server.server_close) if server_started else (server.server_close,)):
                try:
                    close()
                except (OSError, RuntimeError) as error:
                    errors.append(str(error))
        if server_started:
            server_thread.join(timeout=2)
            if server_thread.is_alive():
                errors.append("Fixture server did not stop")
        report["cleanupVerified"] = task is not None and not errors
        if errors:
            report.update(ok=False, cleanupErrors=errors)
        elif task:
            report["checks"].append("owned task/tab and fixture server removed; initial tabs and selection preserved")
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
