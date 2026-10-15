#!/usr/bin/env python3
"""Loopback-only fixture for native navigation failure and recovery QA.

/unstable closes the connection without an HTTP response until POST
/control?allow=1 enables it. /state reports in-memory navigation attempt counts.
No request data is saved. SIGTERM/SIGINT shuts down and joins server threads.
"""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import signal
import socket
import threading
from urllib.parse import parse_qs, urlsplit


def page(title):
    return f"""<!doctype html>
<html lang="en">
<meta charset="utf-8">
<title>{title}</title>
<h1>{title}</h1>
<nav><a href="/ok">Open OK</a> <a href="/unstable">Open unstable</a></nav>
<p><label>Recovery input <input id="recovery-input" autocomplete="off"></label></p>
</html>
""".encode("utf-8")


OK_PAGE = page("Naru Recovery OK")
RESTORED_PAGE = page("Naru Recovery Restored")


class State:
    def __init__(self):
        self.stop = threading.Event()
        self.lock = threading.Lock()
        self.allow = False
        self.requests = {"/ok": 0, "/unstable": 0}
        self.successes = 0
        self.failures = 0

    def snapshot(self):
        with self.lock:
            return {"allow": self.allow, "requests": dict(self.requests),
                    "successes": self.successes, "failures": self.failures}


def handler(state):
    class Handler(BaseHTTPRequestHandler):
        # Covers request-line/header reads, including idle speculative sockets.
        timeout = 3

        def do_GET(self):
            path = urlsplit(self.path).path
            if path == "/state":
                self.send_payload(json.dumps(state.snapshot()).encode(), "application/json")
                return
            if path not in state.requests:
                self.send_payload(b"", "text/plain", 204 if path == "/favicon.ico" else 404)
                return
            with state.lock:
                state.requests[path] += 1
                allowed = path == "/ok" or state.allow
                if allowed:
                    state.successes += 1
                else:
                    state.failures += 1
            if not allowed:
                # Every retry fails while disabled. Sending no status line or
                # response bytes exercises Chromium's empty-response error.
                self.close_connection = True
                try:
                    self.connection.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
                self.connection.close()
                return
            self.send_payload(OK_PAGE if path == "/ok" else RESTORED_PAGE,
                              "text/html; charset=utf-8")

        def do_POST(self):
            parsed = urlsplit(self.path)
            if parsed.path != "/control":
                self.send_payload(b"", "text/plain", 404)
                return
            query = parse_qs(parsed.query, keep_blank_values=True)
            if set(query) != {"allow"} or query["allow"] not in (["0"], ["1"]):
                self.send_payload(b'{"error":"use allow=0 or allow=1"}',
                                  "application/json", 400)
                return
            with state.lock:
                state.allow = query["allow"] == ["1"]
            self.send_payload(json.dumps(state.snapshot()).encode(), "application/json")

        def send_payload(self, data, content_type, status=200):
            self.close_connection = True
            try:
                self.send_response(status)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(data)))
                self.send_header("Cache-Control", "no-store")
                self.send_header("Connection", "close")
                self.end_headers()
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
    print(json.dumps({"base": base, "ok": base + "/ok", "unstable": base + "/unstable",
                      "control": base + "/control", "state": base + "/state"}), flush=True)
    try:
        state.stop.wait()
    finally:
        state.stop.set()
        server.shutdown()
        thread.join(timeout=5)
        server.server_close()
        if thread.is_alive():
            raise RuntimeError("Navigation recovery fixture server thread did not stop")


if __name__ == "__main__":
    main()
