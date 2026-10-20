#!/usr/bin/env python3
"""Local CEF check for dblclick, right click, wheel, drag, coordinate pointer and selector targets.

Requires an already-running compatible app and exclusive CLI access. Uses one owned
task/tab (background, never selected) and a localhost fixture, preserving the user's
selected space/tab. Does not launch an app, retry pointer actions or submit externally.
Dispatch acknowledgement never proves a page effect: every check reads the fixture's
own event receipt. The absence of a native context menu over the user's window is an
AppKit property that this CLI check cannot observe (see docs/pointer-ext.md).

Example:
  python3 tests/pointer_ext_smoke.py --cli target/release/lightbrowser --data-dir "$LIGHT_BROWSER_DATA_DIR"
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
from urllib.parse import quote, unquote, urlsplit
import uuid

from check_smoke import poll
from smoke import BrowserCli, CheckFailed, ROOT, element, require


FIXTURE = ROOT / "tests/fixtures/pointer_ext.html"


FRAME_DOCUMENT = b"""<!doctype html><meta charset=utf-8><title>pointer frame</title>
<button id=fb style="width:200px;height:40px">Frame button</button><pre id=r></pre>
<script>const s={};const r=document.getElementById('r');
const show=()=>{r.textContent='FRAME_STATE_BEGIN '+encodeURIComponent(JSON.stringify(s))+' FRAME_STATE_END'};
for(const t of ['mousedown','mouseup','click','dblclick','contextmenu','wheel'])
document.addEventListener(t,e=>{s[t]=(s[t]||0)+1;if(t==='contextmenu')e.preventDefault();show()},true);show();</script>"""


def make_routes(routes):
    class Handler(BaseHTTPRequestHandler):
        def setup(self):
            # Idle Chromium preconnect sockets must not strand fixture cleanup.
            self.request.settimeout(3)
            super().setup()

        def do_GET(self):
            body = routes.get(urlsplit(self.path).path)
            if body is None:
                self.send_response(204 if self.path == "/favicon.ico" else 404)
                self.end_headers()
                return
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            try:
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                pass

        def log_message(self, *_args):
            pass

    return Handler


def frame_receipt(page):
    match = re.search(r"FRAME_STATE_BEGIN (\S+) FRAME_STATE_END", page.get("text", ""))
    require(match is not None, "Frame receipt is missing or truncated")
    return json.loads(unquote(match.group(1)))


def receipt(page):
    match = re.search(r"POINTER_STATE_BEGIN (\S+) POINTER_STATE_END", page.get("text", ""))
    require(match is not None, "Pointer receipt is missing or truncated")
    return json.loads(unquote(match.group(1)))


def count(state, kind):
    return state["counts"].get(kind, 0)


def run(args):
    cli = BrowserCli(args.cli.resolve(), args.data_dir.resolve())
    report = {"ok": False, "scope": "local CEF dblclick/right click/wheel/drag/coordinates/selectors", "checks": []}
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
                "A pointer command changed the user's selected space/tab")
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
        path = f"/pointer-ext-{uuid.uuid4().hex}.html"
        frame_path = f"/pointer-frame-{uuid.uuid4().hex}.html"
        server = ThreadingHTTPServer(("127.0.0.1", 0), make_routes({path: FIXTURE.read_bytes(), frame_path: FRAME_DOCUMENT}))
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        server_started = True
        task = call("task", "create", "Pointer ext fixture")["task"]
        # localhost is a different origin from 127.0.0.1, so the child is a cross-origin frame.
        child_url = quote(f"http://localhost:{server.server_port}{frame_path}", safe="")
        tab = scoped("open", f"http://127.0.0.1:{server.server_port}{path}?frame={child_url}&slow=250")["tab"]
        require(isinstance(tab, int) and tab not in initial, "Fixture reused an initial tab")
        owned.add(tab)
        scoped("wait", tab, "load", 10000)

        def read():
            page = scoped("read", tab)["page"]
            require(page.get("title") == "Naru pointer ext fixture", "Unexpected fixture")
            return page

        def state():
            return receipt(read())

        poll(read, lambda page: receipt(page)["ready"] is True, "Fixture did not become ready")
        preserved()
        start = state()
        require(start["counts"] == {} and start["width"] > 300 and start["height"] > 200 and start["pad"],
                "Fixture must start without pointer events and with a usable viewport")

        def ack(result, action, **fields):
            page = result["page"]
            require(page.get("ok") is True and page.get("action") == action and page.get("input") == "chromium" and
                    page.get("attempted") is True and page.get("dispatchAcknowledged") is True and
                    "trusted" not in page and "prepared" not in page,
                    f"{action}: expected Chromium dispatch acknowledgement, got {page}")
            for key, value in fields.items():
                require(page.get(key) == value, f"{action}: expected {key}={value!r}, got {page.get(key)!r}")
            return page

        # ---- dblclick by ref -------------------------------------------------
        page = read()
        before = receipt(page)
        result = scoped("dblclick", tab, element(page, "Double target")["ref"])
        ack(result, "dblclick", clickCount=2)
        after = receipt(poll(read, lambda value: count(receipt(value), "dblclick") >= 1,
                             "dblclick acknowledgement did not reach the page"))
        require(count(after, "mousedown") - count(before, "mousedown") == 2 and
                count(after, "mouseup") - count(before, "mouseup") == 2 and
                count(after, "click") - count(before, "click") == 2 and
                count(after, "dblclick") - count(before, "dblclick") == 1,
                "dblclick must produce exactly two press/release/click pairs and one dblclick")
        recent = [event for event in after["events"] if event["id"] == "dbl"]
        require(all(event["tr"] is True for event in recent), "dblclick events were not trusted")
        require([event["d"] for event in recent if event["t"] == "mousedown"][-2:] == [1, 2] and
                recent[-1]["t"] == "dblclick" and recent[-1]["d"] == 2,
                "dblclick must report detail 1 then 2 and end with dblclick")
        require(after["clicks"].get("dbl") == 2, "dblclick target did not receive two click events")
        preserved()
        report["checks"].append("dblclick by ref: 2 trusted press/release/click pairs, detail 1 then 2, one dblclick; selection preserved")

        # ---- right click: native menu guard, context-menu counting ------------
        page = read()
        before = receipt(page)
        result = scoped("click", tab, element(page, "Context target")["ref"], "--button", "right")
        ack(result, "click", button="right")
        after = receipt(poll(read, lambda value: count(receipt(value), "contextmenu") >= 1,
                             "right click did not produce a contextmenu event"))
        context = [event for event in after["events"] if event["id"] == "ctx"]
        require(count(after, "contextmenu") == 1 and any(event["t"] == "mousedown" and event["b"] == 2 for event in context) and
                any(event["t"] == "mouseup" and event["b"] == 2 for event in context) and
                after["contextDefaultPrevented"].get("ctx") is True and
                count(after, "click") == count(before, "click"),
                "right click must press/release button 2, raise contextmenu and no click")
        require(all(event["tr"] is True for event in context), "right-click events were not trusted")
        preserved()
        # A page that does not cancel contextmenu must still not open native UI: the
        # app stays responsive (a modal native menu would stall the next CLI command).
        page = read()
        before = receipt(page)
        started = time.monotonic()
        ack(scoped("click", tab, "text=Context default", "--button", "right"), "click", button="right")
        after = receipt(poll(read, lambda value: count(receipt(value), "contextmenu") > count(before, "contextmenu"),
                             "uncanceled right click did not raise contextmenu"))
        require(after["contextDefaultPrevented"].get("ctxdefault") is False,
                "Fixture must leave this contextmenu uncanceled")
        scoped("tabs")
        require(time.monotonic() - started < 15, "The app stalled after an uncanceled right click")
        status = call("status")
        require(status["pendingWaits"] == 0 and status["dialogs"]["pending"] == 0, "Unexpected pending state after right click")
        preserved()
        report["checks"].append("right click by ref and selector: button 2 press/release + contextmenu, no click; uncanceled contextmenu leaves the app responsive; selection preserved (native menu absence is AppKit-only, unobserved)")

        # ---- wheel at an element center, nested link under the pointer --------
        page = read()
        before = receipt(page)
        result = scoped("wheel", tab, element(page, "Scroll area")["ref"], 0, 120)
        ack(result, "wheel", deltaX=0, deltaY=120)
        after = receipt(poll(read, lambda value: receipt(value)["scrollTop"] > 0 and receipt(value)["wheels"] >= 1,
                             "wheel did not scroll the target"))
        require(after["wheelEvents"][-1]["dy"] == 120 and after["wheelEvents"][-1]["tr"] is True,
                "The page must observe one trusted wheel event with the requested delta")
        require(count(after, "mousedown") == count(before, "mousedown") and count(after, "click") == count(before, "click"),
                "wheel must not press a button or click the nested link")
        scroll_top = after["scrollTop"]
        # `scroll` stays a distinct DOM scroll: no wheel event.
        wheels = after["wheels"]
        scoped("scroll", tab, element(read(), "Scroll area")["ref"], 0, 20)
        after = receipt(poll(read, lambda value: receipt(value)["scrollTop"] > scroll_top, "scroll did not move"))
        require(after["wheels"] == wheels, "scroll must not emit wheel events")
        preserved()
        report["checks"].append("wheel: one trusted wheel event with dx/dy at the element center, scrollTop moved, no press/click; scroll stays DOM-only")

        # ---- drag -------------------------------------------------------------
        page = read()
        before = receipt(page)
        result = scoped("drag", tab, element(page, "Drag card")["ref"], element(page, "Drop slot")["ref"])
        ack(result, "drag")
        after = receipt(poll(read, lambda value: receipt(value)["drag"]["ups"] == 1, "drag did not release"))
        drag = after["drag"]
        require(drag["down"] == 1 and drag["upTarget"] == "slot" and drag["moves"] >= 8 and drag["buttonsOk"] is True and
                drag["order"].startswith("DM") and drag["order"].endswith("MU") and drag["up"]["trusted"] is True,
                f"drag must press, move with the left button held and release on the slot: {drag}")
        require(abs(drag["lastMove"]["x"] - drag["up"]["x"]) < 1.5 and abs(drag["lastMove"]["y"] - drag["up"]["y"]) < 1.5,
                "The final move must land on the release point")
        preserved()
        # By selectors (one resolve pass shares the generation of both refs).
        before = state()
        result = scoped("drag", tab, "text=Drag card", "role=button[name=Drop slot]")
        ack(result, "drag")
        require([item["selector"] for item in result["resolved"]] == ["text=Drag card", "role=button[name=Drop slot]"],
                "A selector drag must report what each selector resolved to")
        after = receipt(poll(read, lambda value: receipt(value)["drag"]["ups"] == 2,
                             "selector drag did not reach the page"))
        require(after["drag"]["down"] == 2, "Selector drag must perform one more press")
        # A natively draggable source is refused before any event.
        before = state()
        response = scoped("drag", tab, "text=Native draggable", "text=Drop slot", expected="DRAG_UNSUPPORTED")
        after = state()
        require(after["counts"] == before["counts"], "Refused drag must not send pointer events")
        preserved()
        report["checks"].append("drag by refs and selectors: one press, >= 8 held-button moves, release on target, final move on release point; natively draggable source refused with no events")

        # ---- a drag aborted by task handoff must still release the button once ----
        before = state()
        box = {}

        def run_drag():
            box["response"] = cli.request("task", "run", task["id"], task["revision"], "drag", tab,
                                          "text=Drag card", "text=Drop slot")

        worker = threading.Thread(target=run_drag)
        worker.start()
        # Preparation takes a few round trips; each of the 10 moves then blocks the page 250 ms.
        time.sleep(1.2)
        call("task", "handoff", task["id"], task["revision"])
        worker.join(40)
        require(not worker.is_alive() and box["response"]["ok"] is False,
                f"The handed-off drag must fail: {box.get('response')}")
        current = info()
        require(current["status"] == "paused", "Handoff did not pause the task")
        task = call("task", "resume", current["id"], current["revision"])["task"]
        after = receipt(poll(read, lambda value: receipt(value)["drag"]["ups"] == before["drag"]["ups"] + 1,
                             "The aborted drag never released its button"))
        require(after["drag"]["down"] == before["drag"]["down"] + 1 and
                after["counts"]["mouseup"] == before["counts"]["mouseup"] + 1 and
                after["drag"]["moves"] < before["drag"]["moves"] + 10,
                f"An aborted drag must press once, stop early and release exactly once: {after['drag']}")
        time.sleep(1.5)
        again = state()
        require(again["counts"]["mouseup"] == after["counts"]["mouseup"] and again["drag"]["ups"] == after["drag"]["ups"],
                "The cleanup release must not repeat")
        preserved()
        report["checks"].append("drag aborted by task handoff mid-way: the page sees exactly one mouseup (cleanup release), no extra moves, no repeat")

        # ---- coordinates --------------------------------------------------------
        page = read()
        pad = receipt(page)["pad"]
        x, y = int(pad["left"] + pad["width"] / 2), int(pad["top"] + pad["height"] / 2)
        before = receipt(page)
        ack(scoped("move-at", tab, x, y), "move-at", x=x, y=y)
        after = receipt(poll(read, lambda value: receipt(value).get("lastMove", {}).get("id") == "pad", "move-at did not move the pointer"))
        require(abs(after["lastMove"]["x"] - x) <= 1 and abs(after["lastMove"]["y"] - y) <= 1 and
                count(after, "mousedown") == count(before, "mousedown"),
                "move-at must move to the point without pressing")
        before = after
        ack(scoped("click-at", tab, x, y), "click-at", x=x, y=y, button="left")
        after = receipt(poll(read, lambda value: count(receipt(value), "click") > count(before, "click"), "click-at did not click"))
        require(after["lastPad"]["type"] == "click" and abs(after["lastPad"]["x"] - x) <= 1 and abs(after["lastPad"]["y"] - y) <= 1,
                "click-at must click the requested point")
        before = after
        ack(scoped("click-at", tab, x, y, "--count", 2), "click-at", clickCount=2)
        after = receipt(poll(read, lambda value: count(receipt(value), "dblclick") > count(before, "dblclick"), "click-at --count 2 did not double click"))
        require(after["lastPad"]["type"] == "dblclick" and after["lastPad"]["detail"] == 2, "click-at --count 2 must double click")
        before = after
        ack(scoped("click-at", tab, x, y, "--button", "right"), "click-at", button="right")
        after = receipt(poll(read, lambda value: count(receipt(value), "contextmenu") > count(before, "contextmenu"), "click-at --button right did not raise contextmenu"))
        require(after["lastPad"]["type"] == "contextmenu" and after["lastPad"]["button"] == 2,
                "click-at --button right must raise contextmenu with button 2")
        # Outside the viewport (and negative) never reach the page.
        before = state()
        response = scoped("click-at", tab, before["width"] + 10, 5, expected="INVALID_POINT")
        response = scoped("move-at", tab, 5, before["height"] + 10, expected="INVALID_POINT")
        response = scoped("click-at", tab, before["width"], 5, expected="INVALID_POINT")
        after = state()
        require(after["counts"] == before["counts"], "Out-of-viewport coordinates must not send pointer events")
        require(call("task", "run", task["id"], task["revision"], "click-at", tab, -1, 5, expected="INVALID_ARGUMENTS") is not None,
                "Negative coordinates must be rejected by the CLI")
        preserved()
        report["checks"].append("move-at/click-at (left, --count 2, --button right) hit the requested CSS pixel; out-of-viewport and negative coordinates rejected with no events")

        # ---- selectors on every element command --------------------------------
        before = state()
        result = scoped("click", tab, "css=#save")
        require_clicked(result)
        require(result["resolved"][0]["selector"] == "css=#save" and result["resolved"][0]["label"] == "Save draft",
                "A selector click must report its resolution")
        poll(read, lambda value: receipt(value)["clicks"].get("save") == 1, "css= click did not click the button")
        scoped("click", tab, "role=button[name=Save draft]")
        poll(read, lambda value: receipt(value)["clicks"].get("save") == 2, "role= click did not click the button")
        scoped("click", tab, 'text="Save draft"')
        poll(read, lambda value: receipt(value)["clicks"].get("save") == 3, "quoted text= click did not click the button")
        scoped("click", tab, "text=save DRAFT")
        poll(read, lambda value: receipt(value)["clicks"].get("save") == 4, "text= is a case-insensitive substring")
        # Missing, ambiguous and invalid selectors act on nothing.
        before = state()
        missing = scoped("click", tab, "text=No such control", expected="TARGET_NOT_FOUND")
        details = missing["error"]["details"]
        require(details["matchCount"] == 0 and details["candidates"] == [] and details["outcomeUnknown"] is False and
                details["attempted"] is False, "TARGET_NOT_FOUND must report zero matches without acting")
        ambiguous = scoped("click", tab, "text=Buy", expected="TARGET_AMBIGUOUS")
        details = ambiguous["error"]["details"]
        require(details["matchCount"] == 3 and len(details["candidates"]) == 3 and
                [item["label"] for item in details["candidates"]] == ["Buy 1", "Buy 2", "Buy 3"],
                "TARGET_AMBIGUOUS must list the candidates")
        # A candidate ref works through the ordinary ref path, exactly once. Any read
        # (including state()) renumbers refs, so click it before reading again.
        scoped("click", tab, details["candidates"][1]["ref"])
        poll(read, lambda value: receipt(value)["clicks"].get("buy2") == 1, "Candidate ref did not click Buy 2")
        after = state()
        require(after["clicks"].get("buy1") is None and after["clicks"].get("buy3") is None, "Wrong Buy button clicked")
        scoped("click", tab, "css=[[", expected="INVALID_SELECTOR")
        require(state()["counts"] == after["counts"], "Failed selector resolution must not send input")
        # Resolution renumbers refs: an older read ref is stale afterwards.
        old = element(read(), "Save draft")["ref"]
        scoped("hover", tab, "text=Save draft")
        scoped("click", tab, old, expected="STALE_REF")
        # The unchanged hit test still protects a covered selector target.
        before = state()
        covered = scoped("click", tab, "text=Covered button", expected="CLICK_POINT_UNAVAILABLE")
        require(state()["counts"] == before["counts"], "A covered selector target must not receive input")
        preserved()
        report["checks"].append("css=/role=/text= click by selector resolves once and reports `resolved`; missing, ambiguous (3 candidates) and invalid selectors act on nothing; candidate ref clicks exactly one button; older ref STALE_REF; covered target keeps the hit-test rejection")

        before = state()
        ack(scoped("dblclick", tab, "text=Double target"), "dblclick", clickCount=2)
        poll(read, lambda value: count(receipt(value), "dblclick") == count(before, "dblclick") + 1, "dblclick by selector did not reach the page")
        scoped("fill", tab, "css=input[name=q]", "naru")
        poll(read, lambda value: element(value, "Query").get("value") == "naru", "fill by selector did not set the value")
        scoped("press", tab, "role=textbox[name=Query]", "Enter")
        poll(read, lambda value: "Enter" in receipt(value)["keys"], "press by selector did not deliver Enter")
        scoped("insert", tab, "css=#query", "!")
        poll(read, lambda value: element(value, "Query").get("value") == "naru!", "insert by selector did not append")
        scoped("check", tab, "role=checkbox[name=Agree]", "true")
        poll(read, lambda value: element(value, "Agree").get("checked") is True, "check by selector did not check")
        scoped("select", tab, "css=#choice", "b")
        poll(read, lambda value: element(value, "Choice").get("value") == "b", "select by selector did not select")
        top = state()["scrollTop"]
        scoped("scroll", tab, "text=Scroll area", 0, 15)
        poll(read, lambda value: receipt(value)["scrollTop"] > top, "scroll by selector did not scroll")
        before = state()
        ack(scoped("wheel", tab, "text=Scroll area", 0, 40), "wheel")
        poll(read, lambda value: receipt(value)["wheels"] == before["wheels"] + 1, "wheel by selector did not reach the page")
        ack(scoped("click", tab, "text=Context target", "--button", "right"), "click", button="right")
        preserved()
        report["checks"].append("selectors accepted by dblclick, fill, press, insert, check, select, scroll, wheel, right click; user selection preserved")

        # ---- cross-origin frame refs: same gestures through read-frame, but no drag ----
        def frame_page():
            frames = scoped("frames", tab)["frames"]
            child = [item for item in frames if frame_path in item.get("url", "")]
            require(len(child) == 1, "The cross-origin child frame was not discovered")
            return scoped("read-frame", tab, child[0]["frameId"])["page"]

        def frame_page_soon():
            try:
                return frame_page()
            except CheckFailed:
                return {"text": ""}

        poll(frame_page_soon, lambda page: "FRAME_STATE_BEGIN" in page.get("text", ""),
             "Cross-origin child did not load")
        require(frame_receipt(frame_page()) == {}, "The child must start without pointer events")
        ref = element(frame_page(), "Frame button")["ref"]
        require(ref.startswith("frame/"), "Expected a frame ref")
        ack(scoped("dblclick", tab, ref), "dblclick", clickCount=2)
        shown = frame_receipt(poll(frame_page, lambda page: frame_receipt(page).get("dblclick") == 1,
                                   "Frame dblclick did not reach the child"))
        require(shown["mousedown"] == 2 and shown["mouseup"] == 2 and shown["click"] == 2 and "contextmenu" not in shown,
                f"Frame dblclick must be two press/release pairs: {shown}")
        ref = element(frame_page(), "Frame button")["ref"]
        ack(scoped("click", tab, ref, "--button", "right"), "click", button="right")
        shown = frame_receipt(poll(frame_page, lambda page: frame_receipt(page).get("contextmenu") == 1,
                                   "Frame right click did not raise contextmenu"))
        require(shown["click"] == 2 and shown["mousedown"] == 3, f"Frame right click must not click: {shown}")
        ref = element(frame_page(), "Frame button")["ref"]
        ack(scoped("wheel", tab, ref, 0, 30), "wheel", deltaY=30)
        poll(frame_page, lambda page: frame_receipt(page).get("wheel") == 1, "Frame wheel did not reach the child")
        ref = element(frame_page(), "Frame button")["ref"]
        refused = scoped("drag", tab, ref, ref + "9", expected="UNSUPPORTED_TARGET")
        require(frame_receipt(frame_page()).get("mousedown") == 3, "Refused frame drag must send no input")
        preserved()
        report["checks"].append("cross-origin read-frame refs accept dblclick, right click and wheel through the frame pointer path; drag is refused with UNSUPPORTED_TARGET and no input")

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


def require_clicked(result):
    page = result["page"]
    require(page.get("ok") is True and page.get("action") == "click" and page.get("input") == "chromium" and
            page.get("dispatchAcknowledged") is True, f"click: expected Chromium acknowledgement, got {page}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
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
