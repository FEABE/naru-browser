#!/usr/bin/env python3
"""Exercise open Shadow DOM snapshots and actions through an already running CEF app.

Uses one test-owned task/tab and localhost fixtures. Existing user tabs are never
navigated or closed. Does not launch the app or create/delete browser profiles.
Run without other concurrent CLI tests; UI panel focus is not used for page reads.
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

from smoke import BrowserCli, CheckFailed, ROOT, element, require


PASSWORD = "SHADOW_FAKE_PASSWORD_MUST_NOT_BE_EXPOSED"
QUERY = "shadow input 한글"
FORBIDDEN = (
    PASSWORD,
    "CLOSED_ROOT_TEXT_MUST_NOT_BE_EXPOSED",
    "IFRAME_TEXT_MUST_NOT_BE_EXPOSED",
    "HIDDEN_HOST_TEXT_MUST_NOT_BE_EXPOSED",
    "INERT_HOST_TEXT_MUST_NOT_BE_EXPOSED",
    "ARIA_HIDDEN_HOST_TEXT_MUST_NOT_BE_EXPOSED",
    "DISPLAY_NONE_HOST_TEXT_MUST_NOT_BE_EXPOSED",
)

MAIN_SCRIPT = r'''
const mount = document.getElementById('mount');
function host(id, parent = mount) {
  const node = document.createElement('div');
  node.id = id;
  parent.append(node);
  return node;
}
function shadow(node, markup, mode = 'open') {
  const root = node.attachShadow({mode});
  root.innerHTML = '<style>:host{display:block;margin:8px 0}button,input{font:inherit;margin:4px}</style>' + markup;
  return root;
}
function control(label, action) {
  const button = document.createElement('button');
  button.textContent = label;
  button.addEventListener('click', action);
  mount.append(button);
}
const root = shadow(host('form-host'), `
  <p>Open shadow visible text</p>
  <form><label for="query">Shadow query</label><input id="query" name="query">
    <label for="password">Shadow password</label>
    <input id="password" type="password" value="SHADOW_FAKE_PASSWORD_MUST_NOT_BE_EXPOSED">
    <span id="root-label">Root scoped action</span>
    <button type="button" aria-labelledby="root-label">Unlabelled action fallback</button>
    <button type="button" id="remove-target">Removable shadow target</button>
    <button type="submit">Submit shadow form</button>
  </form><p id="input-state">No input event</p><p id="submit-state">No submit event</p>
  <p id="click-state">No shadow click</p><div id="nested-host"></div>`);
root.getElementById('query').addEventListener('input', (event) => {
  root.getElementById('input-state').textContent = 'Input event: ' + event.target.value;
});
root.querySelector('form').addEventListener('submit', (event) => {
  event.preventDefault();
  root.getElementById('submit-state').textContent = 'Submitted shadow query: ' + root.getElementById('query').value;
});
root.querySelector('[aria-labelledby]').addEventListener('click', () => {
  root.getElementById('click-state').textContent = 'Shadow click observed';
});
const nested = shadow(root.getElementById('nested-host'), '<p>Nested shadow visible text</p><button>Nested action</button>');
nested.querySelector('button').addEventListener('click', (event) => { event.target.textContent = 'Nested action clicked'; });

const slotHost = host('slot-host');
shadow(slotHost, '<p>Slot host visible text</p><slot name="action"><button>Slot fallback must not be exposed</button></slot>');
const slotted = document.createElement('button');
slotted.slot = 'action';
slotted.textContent = 'Slotted action';
slotted.addEventListener('click', () => { slotted.textContent = 'Slotted action clicked'; });
slotHost.append(slotted);

const mutable = host('mutable-host');
shadow(mutable, '<button>Mutable host target</button>');
control('Hide mutable host', () => { mutable.hidden = true; });
control('Show mutable host', () => { mutable.hidden = false; });
control('Disable mutable host', () => { mutable.setAttribute('aria-disabled', 'true'); });
control('Enable mutable host', () => { mutable.removeAttribute('aria-disabled'); });
control('Make mutable host inert', () => { mutable.inert = true; });
control('Remove shadow target', () => { root.getElementById('remove-target').remove(); });

for (const [id, text, kind] of [
  ['hidden-host', 'HIDDEN_HOST_TEXT_MUST_NOT_BE_EXPOSED', 'hidden'],
  ['inert-host', 'INERT_HOST_TEXT_MUST_NOT_BE_EXPOSED', 'inert'],
  ['aria-hidden-host', 'ARIA_HIDDEN_HOST_TEXT_MUST_NOT_BE_EXPOSED', 'aria-hidden'],
  ['display-none-host', 'DISPLAY_NONE_HOST_TEXT_MUST_NOT_BE_EXPOSED', 'display']
]) {
  const node = host(id);
  shadow(node, '<p>' + text + '</p><button>' + text + ' action</button>');
  if (kind === 'display') node.style.display = 'none';
  else if (kind === 'aria-hidden') node.setAttribute('aria-hidden', 'true');
  else node[kind] = true;
}
const disabled = host('disabled-host');
disabled.setAttribute('aria-disabled', 'true');
shadow(disabled, '<button>Disabled host action</button>');
shadow(host('closed-host'), '<p>CLOSED_ROOT_TEXT_MUST_NOT_BE_EXPOSED</p><button>Closed root action</button>', 'closed');
const frame = document.createElement('iframe');
frame.title = 'Excluded embedded document';
frame.srcdoc = '<!doctype html><p>IFRAME_TEXT_MUST_NOT_BE_EXPOSED</p><button>Iframe action</button>';
mount.append(frame);
'''

CAP_SCRIPTS = {
    "items": """const root = document.getElementById('mount').attachShadow({mode:'open'});
root.innerHTML = Array.from({length:130}, (_, i) => '<button>Bounded action ' + i + '</button>').join('');""",
    "text": """const root = document.getElementById('mount').attachShadow({mode:'open'});
root.textContent = 'x'.repeat(12020);""",
    "nodes": """const root = document.getElementById('mount').attachShadow({mode:'open'});
const fragment = document.createDocumentFragment();
for (let i = 0; i < 10050; i++) fragment.append(document.createElement('span'));
const target = document.createElement('button'); target.textContent = 'NODE_BOUNDARY_UNSEEN';
fragment.append(target); root.append(fragment);""",
    "roots": """const mount = document.getElementById('mount');
for (let i = 0; i < 66; i++) {
  const host = document.createElement('div'); mount.append(host);
  const root = host.attachShadow({mode:'open'});
  const button = document.createElement('button'); button.textContent = 'Root action ' + i; root.append(button);
}""",
    "depth": """let container = document.getElementById('mount');
for (let i = 0; i < 70; i++) {
  const host = document.createElement('div'); container.append(host);
  container = host.attachShadow({mode:'open'});
}
const target = document.createElement('button'); target.textContent = 'DEPTH_BOUNDARY_UNSEEN'; container.append(target);""",
}


def fixture(script):
    return ('''<!doctype html><html lang="en"><head><meta charset="utf-8">
<title>Shadow fixture</title><style>body{font:16px system-ui;margin:24px}button{font:inherit;margin:4px}</style>
</head><body><h1>Local shadow fixture</h1><span id="root-label">Wrong document label</span>
<div id="mount"></div><script>''' + script + '</script></body></html>').encode('utf-8')


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
    report = {"ok": False, "scope": "real CEF open Shadow DOM through local CLI fixtures", "checks": []}
    initial = set()
    task = None
    tab = None
    last_response = None
    server = server_thread = None

    def call(*command, expected=None):
        nonlocal last_response
        last_response = cli.request(*command)
        if expected:
            require(not last_response["ok"] and last_response.get("error", {}).get("code") == expected,
                    f"Expected {expected}, received {json.dumps(last_response, ensure_ascii=False)}")
            return last_response
        require(last_response["ok"], f"CLI request failed: {json.dumps(last_response, ensure_ascii=False)}")
        return last_response["result"]

    def command(*operation, expected=None):
        return call("task", "run", task["id"], task["revision"], *operation, expected=expected)

    def tab_ids():
        return {value["tab"] for value in call("tabs")["tabs"]}

    def info():
        return call("task", "info", task["id"])["task"]

    def read():
        page = command("read", tab)["page"]
        require(len(page["interactive"]) <= 120 and len(page["text"]) <= 12000, "Snapshot exceeded its output caps")
        require(page.get("documentId"), "Snapshot is missing its document identity")
        require(all(marker not in json.dumps(page, ensure_ascii=False) for marker in FORBIDDEN),
                "Snapshot exposed excluded shadow, iframe, hidden/inert, or password content")
        return page

    def click(page, label):
        return command("click", tab, element(page, label)["ref"])

    def navigate(url):
        command("navigate", tab, url)
        command("wait", tab, "load", 5000)
        page = read()
        require(page["url"] == url, "Snapshot did not reach the requested fixture")
        return page

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            path = urlsplit(self.path).path
            script = MAIN_SCRIPT if path == "/fixture" else CAP_SCRIPTS.get(path.removeprefix("/caps/"))
            if script is None:
                self.send_response(204)
                self.end_headers()
                return
            body = fixture(script)
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
        require(call("status")["engine"] == "CEF/Chromium", "Expected a running real CEF app")
        initial = tab_ids()
        require(initial, "Keep an initial user tab open before running this test")
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        origin = f"http://127.0.0.1:{server.server_port}"
        task = call("task", "create", "Shadow fixture")["task"]
        opened = command("open", origin + "/fixture")
        tab = opened["tab"]
        require(isinstance(tab, int) and tab not in initial, "Open did not create a distinct fixture tab")
        command("wait", tab, "load", 5000)
        page = read()
        require("Open shadow visible text" in page["text"] and "Nested shadow visible text" in page["text"],
                "Open or nested Shadow DOM text is missing")
        element(page, "Shadow query")
        element(page, "Root scoped action")
        element(page, "Nested action")
        element(page, "Slotted action")
        require("Slot fallback must not be exposed" not in json.dumps(page), "Assigned slot exposed its fallback")
        require("value" not in element(page, "Shadow password"), "Password input exposed a value property")
        disabled = element(page, "Disabled host action")
        require(disabled["disabled"] is True, "Shadow host aria-disabled did not reach its control")
        command("click", tab, disabled["ref"], expected="ELEMENT_UNAVAILABLE")
        report["checks"].append("open/nested shadow text, root-local labels and assigned slots are readable; excluded content stays omitted")

        command("fill", tab, element(page, "Shadow query")["ref"], QUERY)
        page = read()
        require(element(page, "Shadow query").get("value") == QUERY and f"Input event: {QUERY}" in page["text"],
                "Shadow fill did not reach the input value and page input handler")
        pressed = command("press", tab, element(page, "Shadow query")["ref"], "Enter")
        require(pressed["page"].get("formSubmissionRequested") is True, "Enter did not request the shadow form submission")
        page = read()
        require(f"Submitted shadow query: {QUERY}" in page["text"], "Shadow form submit handler did not run")
        click(page, "Root scoped action")
        page = read()
        require("Shadow click observed" in page["text"], "Root-labelled shadow click did not run its handler")
        click(page, "Nested action")
        page = read()
        element(page, "Nested action clicked")
        click(page, "Slotted action")
        page = read()
        element(page, "Slotted action clicked")
        report["checks"].append("shadow refs support fill/input events, Enter/form submit, nested clicks and slotted clicks")

        removed_ref = element(page, "Removable shadow target")["ref"]
        click(page, "Remove shadow target")
        command("click", tab, removed_ref, expected="STALE_REF")
        page = read()
        require(not any(item["label"] == "Removable shadow target" for item in page["interactive"]),
                "Removed shadow element remains in the snapshot")
        hidden_ref = element(page, "Mutable host target")["ref"]
        click(page, "Hide mutable host")
        command("click", tab, hidden_ref, expected="ELEMENT_UNAVAILABLE")
        page = read()
        require("Mutable host target" not in json.dumps(page), "Hidden host still exposes its shadow target")
        click(page, "Show mutable host")
        page = read()
        disabled_ref = element(page, "Mutable host target")["ref"]
        click(page, "Disable mutable host")
        command("click", tab, disabled_ref, expected="ELEMENT_UNAVAILABLE")
        page = read()
        require(element(page, "Mutable host target")["disabled"] is True, "Host disabling is missing from the snapshot")
        click(page, "Enable mutable host")
        page = read()
        inert_ref = element(page, "Mutable host target")["ref"]
        click(page, "Make mutable host inert")
        command("click", tab, inert_ref, expected="ELEMENT_UNAVAILABLE")
        page = read()
        require("Mutable host target" not in json.dumps(page), "Inert host still exposes its shadow target")
        report["checks"].append("without another read, removed refs fail stale and hidden/disabled/inert host refs fail unavailable")

        item_page = navigate(origin + "/caps/items")
        require(len(item_page["interactive"]) == 120 and item_page["truncated"].get("interactive") is True,
                "Shadow interactive cap did not truncate at 120 items")
        text_page = navigate(origin + "/caps/text")
        require(len(text_page["text"]) == 12000 and text_page["truncated"].get("text") is True,
                "Shadow text cap did not truncate at 12000 characters")
        node_page = navigate(origin + "/caps/nodes")
        require(node_page["truncated"].get("nodes") is True and "NODE_BOUNDARY_UNSEEN" not in json.dumps(node_page),
                "Visited-node bound did not stop before the late shadow sentinel")
        roots_page = navigate(origin + "/caps/roots")
        require(roots_page["truncated"].get("shadowRoots") is True and len(roots_page["interactive"]) <= 64,
                "Open shadow root bound did not stop after at most 64 roots")
        depth_page = navigate(origin + "/caps/depth")
        require(depth_page["truncated"].get("depth") is True and "DEPTH_BOUNDARY_UNSEEN" not in json.dumps(depth_page),
                "Depth bound did not stop before the deeply nested shadow sentinel")
        report["checks"].append("120 items/12000 text and traversal bounds (10000 nodes, 64 roots/depth) report truncation on real DOM fixtures")
        report["ok"] = True
    except (CheckFailed, OSError, KeyError, TypeError) as error:
        report["error"] = str(error)
        report["lastResponse"] = last_response
    finally:
        cleanup = []
        if task is not None:
            try:
                task = info()
                if task["status"] != "finished":
                    finished = call("task", "finish", task["id"], task["revision"])
                    task = finished["task"]
                    require(set(finished["closeRequested"]) <= ({tab} if tab is not None else set()),
                            "Task finish requested closing an unexpected tab")
                    task = poll(info, lambda value: value["status"] == "finished", "Fixture task did not finish")
            except (CheckFailed, OSError, KeyError, TypeError) as error:
                cleanup.append(str(error))
        if tab is not None:
            try:
                require(tab not in initial, "Refusing to close an initial user tab")
                final = poll(tab_ids, lambda tabs: tab not in tabs, "Fixture tab did not close")
                require(initial <= final, "An initial user tab disappeared")
                report["checks"].append("only the fixture task/tab closed; initial user tabs preserved")
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
