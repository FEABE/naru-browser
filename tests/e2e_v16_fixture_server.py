#!/usr/bin/env python3
"""Loopback fixture for the e2e-v15 CLI journeys (05/13). Logs every request.

Usage: e2e_v15_fixture_server.py PORT ACCESS_LOG TOKEN
Cookies and localStorage are synthetic and keyed by TOKEN. Nothing leaves loopback.
"""
from http.cookies import SimpleCookie
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json, signal, sys, threading, time
from urllib.parse import urlsplit

PORT, LOG, TOKEN = int(sys.argv[1]), sys.argv[2], sys.argv[3]
LOCK = threading.Lock()


def page(title, body, script=""):
    return (f'<!doctype html><html><meta charset="utf-8"><title>{title}</title><body>'
            f'<h1>{title}</h1>{body}<script>{script}</script></body></html>').encode()


class H(BaseHTTPRequestHandler):
    def do_GET(self):
        path = urlsplit(self.path).path
        with LOCK, open(LOG, "a") as log:
            log.write(json.dumps({"t": round(time.time(), 3), "path": self.path}) + "\n")
        if path == "/favicon.ico":
            self.send_response(204); self.end_headers(); return
        extra = []
        if path == "/slow":
            time.sleep(8)
        if path in ("/user", "/task", "/a", "/b", "/slow", "/nav-last"):
            body = page(f"E2E {path[1:]} page",
                        f'<p>marker-{path[1:]}</p><input id="f" aria-label="field {path[1:]}" value="">'
                        f'<a href="/b">link to b</a>')
        elif path in ("/seed", "/check"):
            c = SimpleCookie(); c.load(self.headers.get("Cookie", ""))
            ok = lambda n: n in c and c[n].value == TOKEN
            body = page("E2E cookie fixture",
                        f'<p>Persistent cookie: {"present" if ok("e2e_persistent") else "missing"}</p>'
                        f'<p>Session cookie: {"present" if ok("e2e_session") else "missing"}</p>'
                        '<p id="s">Local storage: waiting</p>',
                        f'const t={json.dumps(TOKEN)};if({"true" if path=="/seed" else "false"})localStorage.setItem("e2e",t);'
                        'document.getElementById("s").textContent="Local storage: "+(localStorage.getItem("e2e")===t?"present":"missing");')
            if path == "/seed":
                extra = [f"e2e_persistent={TOKEN}; Path=/; Max-Age=3600; HttpOnly; SameSite=Lax",
                         f"e2e_session={TOKEN}; Path=/; HttpOnly; SameSite=Lax"]
        else:
            self.send_error(404); return
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        for e in extra:
            self.send_header("Set-Cookie", e)
        self.end_headers()
        try:
            self.wfile.write(body)
        except OSError:
            pass

    def log_message(self, *a):
        pass


srv = ThreadingHTTPServer(("127.0.0.1", PORT), H)
signal.signal(signal.SIGTERM, lambda *_: threading.Thread(target=srv.shutdown).start())
srv.serve_forever()
