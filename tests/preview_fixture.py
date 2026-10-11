#!/usr/bin/env python3
"""Local gated pages for native preview and combined-search checks.

Usage:
  python3 tests/preview_fixture.py --port 0
  curl http://127.0.0.1:PORT/state
  curl -X POST http://127.0.0.1:PORT/release/headers
  curl -X POST http://127.0.0.1:PORT/release/image

The first stdout line is JSON with the actual address and fixed route URLs.
Open /fast, /delayed-headers, and /delayed-image in test-owned browser tabs.
The headers gate holds the main response; the image gate holds /image.svg after
the main HTML is sent. Each blocked request returns 504 after 120 seconds.
Gates are one-shot. Restart this harness for another run. SIGINT/SIGTERM releases
both gates, stops the server, and joins its threads. Request logs use stderr.

/user-control.html serves only the existing fixed fixture, including its label
query parameter. This harness never controls an app or serves arbitrary files.
/find-in-page.html and /find-in-page-frame.html serve the fixed find fixtures.
/ime-input.html serves the fixed local composition event receipt fixture.
/single-window.html serves the existing native popup/beforeunload fixture.
/select.html serves the fixed native HTML select and event receipt fixture.
/state records HTTP activity, not document readiness or screenshot success.
Its gate "completed" count means the wait ended, not that the browser loaded it.
Native UI and CLI observations must establish those separately; slow or missing
browser requests and the automatic 504 are not a successful preview check.
"""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import signal
import socket
import sys
import threading
from urllib.parse import urlsplit


WAIT_SECONDS = 120
GATE_NAMES = ("headers", "image")
GET_PATHS = (
    "/fast", "/delayed-headers", "/delayed-image", "/image.svg",
    "/user-control.html", "/find-in-page.html", "/find-in-page-frame.html", "/ime-input.html",
    "/single-window.html", "/select.html", "/state",
)


def page(title, color, description, image=False):
    resource = '<img src="/image.svg" width="240" height="120" alt="지연 이미지">' if image else ""
    return f"""<!doctype html>
<html lang="ko"><meta charset="utf-8"><title>{title}</title>
<style>body {{margin:48px;background:{color};color:white;font:24px system-ui}}
h1 {{font-size:40px}} img {{display:block;margin-top:24px}}</style>
<h1>{title}</h1><p>{description}</p>{resource}</html>""".encode("utf-8")


PAGES = {
    "/fast": page("미리보기 빠른 페이지", "#235d9e", "파란 배경, 지연 없이 문서가 끝납니다."),
    "/delayed-headers": page(
        "미리보기 헤더 대기", "#a24e24", "주황 배경, 헤더 대기를 해제한 뒤 도착한 문서입니다."
    ),
    "/delayed-image": page(
        "미리보기 이미지 대기", "#326b46", "초록 배경, 본문은 보이고 이미지는 해제를 기다립니다.", True
    ),
    "/find-in-page.html": (
        Path(__file__).resolve().parent / "fixtures" / "find-in-page.html"
    ).read_bytes(),
    "/find-in-page-frame.html": (
        Path(__file__).resolve().parent / "fixtures" / "find-in-page-frame.html"
    ).read_bytes(),
    "/ime-input.html": (
        Path(__file__).resolve().parent / "fixtures" / "ime-input.html"
    ).read_bytes(),
    "/single-window.html": (
        Path(__file__).resolve().parent / "fixtures" / "single-window.html"
    ).read_bytes(),
    "/select.html": (
        Path(__file__).resolve().parent / "fixtures" / "select.html"
    ).read_bytes(),
}
IMAGE = b"""<svg xmlns="http://www.w3.org/2000/svg" width="240" height="120">
<rect width="240" height="120" fill="#ffdb70"/>
<circle cx="60" cy="60" r="36" fill="#573a92"/>
<path d="M130 65l22 22 48-52" fill="none" stroke="#235d9e" stroke-width="12"/>
</svg>"""


class Gates:
    def __init__(self):
        self.lock = threading.Lock()
        self.events = {name: threading.Event() for name in GATE_NAMES}
        self.gates = {
            name: {"requested": 0, "waiting": 0, "completed": 0, "timedOut": 0,
                   "released": False, "releaseReason": None}
            for name in GATE_NAMES
        }
        self.requests = {"GET " + path: 0 for path in GET_PATHS}
        self.requests.update({"POST /release/" + name: 0 for name in GATE_NAMES})

    def record(self, method, path):
        with self.lock:
            self.requests[method + " " + path] += 1

    def wait(self, name):
        with self.lock:
            self.gates[name]["requested"] += 1
            self.gates[name]["waiting"] += 1
        released = self.events[name].wait(WAIT_SECONDS)
        with self.lock:
            self.gates[name]["waiting"] -= 1
            self.gates[name]["completed"] += 1
            if not released:
                self.gates[name]["timedOut"] += 1
        return released

    def release(self, name, reason="post"):
        with self.lock:
            if not self.gates[name]["released"]:
                self.gates[name]["released"] = True
                self.gates[name]["releaseReason"] = reason
            self.events[name].set()

    def release_all(self):
        for name in GATE_NAMES:
            self.release(name, "shutdown")

    def snapshot(self):
        with self.lock:
            return {
                "waitTimeoutSeconds": WAIT_SECONDS,
                "requests": self.requests.copy(),
                "gates": {name: values.copy() for name, values in self.gates.items()},
            }


def make_handler(gates, user_control):
    class Handler(BaseHTTPRequestHandler):
        def setup(self):
            super().setup()
            # Bound request-header reads and writes during shutdown as well.
            self.connection.settimeout(5)

        def respond(self, status, body, content_type):
            try:
                self.send_response(status)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Cache-Control", "no-store")
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError, socket.timeout):
                pass  # Closing an owned test tab can abandon its pending request.
            finally:
                self.close_connection = True

        def json_response(self, status, payload):
            self.respond(status, json.dumps(payload).encode(), "application/json; charset=utf-8")

        def do_GET(self):
            path = urlsplit(self.path).path
            if path not in GET_PATHS:
                self.json_response(404, {"error": "unknown fixture path"})
                return
            gates.record("GET", path)
            if path == "/state":
                self.json_response(200, gates.snapshot())
                return
            gate = {"/delayed-headers": "headers", "/image.svg": "image"}.get(path)
            if gate and not gates.wait(gate):
                self.json_response(504, {"error": "fixture gate timed out", "gate": gate})
                return
            if path == "/image.svg":
                self.respond(200, IMAGE, "image/svg+xml")
            elif path == "/user-control.html":
                self.respond(200, user_control, "text/html; charset=utf-8")
            else:
                self.respond(200, PAGES[path], "text/html; charset=utf-8")

        def do_POST(self):
            path = urlsplit(self.path).path
            gate = {"/release/" + name: name for name in GATE_NAMES}.get(path)
            if gate is None:
                self.json_response(404, {"error": "unknown fixture path"})
                return
            gates.record("POST", path)
            gates.release(gate)
            self.json_response(200, gates.snapshot())

        def log_message(self, fmt, *args):
            print(f"{self.log_date_time_string()} {fmt % args}", file=sys.stderr, flush=True)

    return Handler


def port_number(value):
    port = int(value)
    if not 0 <= port <= 65535:
        raise argparse.ArgumentTypeError("port must be between 0 and 65535")
    return port


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--port", type=port_number, default=0)
    args = parser.parse_args()
    user_control = (Path(__file__).resolve().parent / "fixtures" / "user-control.html").read_bytes()
    gates = Gates()
    stopped = threading.Event()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), make_handler(gates, user_control))
    server.daemon_threads = False
    server.block_on_close = True
    thread = threading.Thread(target=server.serve_forever, name="naru-preview-fixture")
    previous_signals = {}
    started = False
    try:
        for signum in (signal.SIGINT, signal.SIGTERM):
            previous_signals[signum] = signal.signal(signum, lambda _signal, _frame: stopped.set())
        thread.start()
        started = True
        address = f"http://127.0.0.1:{server.server_port}"
        print(json.dumps({
            "address": address,
            "waitTimeoutSeconds": WAIT_SECONDS,
            "urls": {path: address + path for path in GET_PATHS},
            "release": {name: address + "/release/" + name for name in GATE_NAMES},
        }), flush=True)
        stopped.wait()
    finally:
        gates.release_all()
        if started:
            server.shutdown()
            thread.join()
        server.server_close()
        for signum, handler in previous_signals.items():
            signal.signal(signum, handler)


if __name__ == "__main__":
    main()
