#!/usr/bin/env python3
"""Local attachment server for native save, progress, cancellation and failure QA.

Only fixed fixtures are served. No uploads or arbitrary filesystem paths.
The first stdout line contains URLs, expected size and SHA-256 for small.bin.
SIGTERM/SIGINT stops slow responses and joins server threads. /state is read-only.
/recoverable.bin fails until POST /recover receives the exact JSON {"ready":true}.
Its later requests return the same bytes as small.bin, including their SHA-256.
"""

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import signal
import threading
from urllib.parse import urlsplit


PAGE = (Path(__file__).parent / "fixtures/downloads.html").read_bytes()
CHUNK = bytes(range(256)) * 256
SMALL = CHUNK * 2
SLOW_CHUNKS = 128
RECOVER_PAYLOAD = b'{"ready":true}'


class State:
    def __init__(self):
        self.stop = threading.Event()
        self.lock = threading.Lock()
        self.requests = {path: 0 for path in (
            "/", "/small.bin", "/slow.bin", "/broken.bin", "/recoverable.bin")}
        self.active = 0
        self.completed = 0
        self.disconnected = 0
        self.recover_ready = False
        self.recover_posts = 0

    def snapshot(self):
        with self.lock:
            return {"requests": dict(self.requests), "active": self.active,
                    "completed": self.completed, "disconnected": self.disconnected,
                    "recoverReady": self.recover_ready, "recoverPosts": self.recover_posts}


def handler(state):
    class Handler(BaseHTTPRequestHandler):
        # StreamRequestHandler applies this before reading the request line or
        # headers, so speculative idle connections cannot block server_close().
        timeout = 3

        def do_GET(self):
            path = urlsplit(self.path).path
            if path == "/state":
                self.send_payload(json.dumps(state.snapshot()).encode(), "application/json")
                return
            if path not in state.requests:
                self.send_response(204 if path == "/favicon.ico" else 404)
                self.end_headers()
                return
            with state.lock:
                state.requests[path] += 1
                recover_ready = state.recover_ready
            if path == "/":
                self.send_payload(PAGE, "text/html; charset=utf-8")
                return
            size = len(SMALL) if path in ("/small.bin", "/recoverable.bin") else len(CHUNK) * SLOW_CHUNKS
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(size))
            self.send_header("Content-Disposition", f'attachment; filename="naru-{path[1:]}"')
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            with state.lock:
                state.active += 1
            try:
                if path == "/small.bin" or (path == "/recoverable.bin" and recover_ready):
                    self.wfile.write(SMALL)
                elif path in ("/broken.bin", "/recoverable.bin"):
                    self.wfile.write(CHUNK)
                    self.wfile.flush()
                    self.close_connection = True
                    return
                else:
                    for _ in range(SLOW_CHUNKS):
                        if state.stop.wait(0.3):
                            return
                        self.wfile.write(CHUNK)
                        self.wfile.flush()
                with state.lock:
                    state.completed += 1
            except (OSError, TimeoutError):
                with state.lock:
                    state.disconnected += 1
            finally:
                with state.lock:
                    state.active -= 1

        def do_POST(self):
            if self.path != "/recover":
                self.send_error(404)
                return
            # Only this tiny, explicit fixture transition is accepted. Browser
            # origins and transfer encoding are unnecessary for the QA command.
            if (self.headers.get("Origin") is not None
                    or self.headers.get("Transfer-Encoding") is not None
                    or self.headers.get("Content-Type") != "application/json"
                    or self.headers.get("Content-Length") != str(len(RECOVER_PAYLOAD))):
                self.send_error(400, "Expected exact recovery JSON")
                return
            try:
                payload = self.rfile.read(len(RECOVER_PAYLOAD))
            except (OSError, TimeoutError):
                self.send_error(400, "Recovery payload not received")
                return
            if payload != RECOVER_PAYLOAD:
                self.send_error(400, "Expected exact recovery JSON")
                return
            with state.lock:
                state.recover_ready = True
                state.recover_posts += 1
            self.send_payload(json.dumps(state.snapshot()).encode(), "application/json")

        def send_payload(self, data, content_type):
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            try:
                self.wfile.write(data)
            except OSError:
                pass

        def log_message(self, *_args):
            pass

    return Handler


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=0)
    args = parser.parse_args()
    state = State()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler(state))
    server.daemon_threads = False
    server.block_on_close = True
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, lambda *_: state.stop.set())
    thread = threading.Thread(target=server.serve_forever)
    thread.start()
    base = f"http://127.0.0.1:{server.server_port}"
    print(json.dumps({"base": base, "page": base + "/", "state": base + "/state",
                      "smallBytes": len(SMALL), "smallSha256": hashlib.sha256(SMALL).hexdigest(),
                      "slowBytes": len(CHUNK) * SLOW_CHUNKS,
                      "recoverable": base + "/recoverable.bin",
                      "recover": base + "/recover"}), flush=True)
    try:
        state.stop.wait()
    finally:
        state.stop.set()
        server.shutdown()
        thread.join(timeout=5)
        server.server_close()
        if thread.is_alive():
            raise RuntimeError("Download fixture server thread did not stop")


if __name__ == "__main__":
    main()
