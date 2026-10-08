#!/usr/bin/env python3
"""Run the local CLI smoke test twice with one newly created browser profile.

Example:
  python3 tests/restart_smoke.py --app 'dist/Light Browser.app' \
      --cli target/debug/lightbrowser --data-dir .local-data/restart-new

The data directory must not exist, even if empty. It is retained for diagnosis.
No external sites, existing profiles, Keychain approval automation, or browser
security/storage overrides are used. Any macOS approval must be handled by the user.
"""

import argparse
from http.cookies import CookieError, SimpleCookie
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import threading
import time
from types import SimpleNamespace
from urllib.parse import urlsplit
import uuid

from smoke import BrowserCli, CheckFailed, require, run as run_smoke


def app_executable(path):
    path = path.resolve(strict=True)
    bundle = path if path.is_dir() else path.parent.parent.parent
    require(bundle.suffix == ".app", "Use the Light Browser bundle or its bundled executable")
    with (bundle / "Contents" / "Info.plist").open("rb") as stream:
        info = plistlib.load(stream)
    require(info.get("CFBundleIdentifier") == "local.lightbrowser.dev" and
            info.get("CFBundleExecutable") == "light-browser",
            "Refusing an unrelated app that may ignore the dedicated profile setting")
    executable = bundle / "Contents" / "MacOS" / "light-browser"
    require(path == bundle or path == executable, "Path is not the declared app executable")
    require(executable.is_file() and os.access(executable, os.X_OK), "App executable is unavailable")
    return executable


def persistence_handler(token):
    class PersistenceHandler(BaseHTTPRequestHandler):
        def do_GET(self):
            path = urlsplit(self.path).path
            if path == "/favicon.ico":
                self.send_response(204)
                self.end_headers()
                return
            if path not in ("/seed", "/check"):
                self.send_error(404)
                return
            cookies = SimpleCookie()
            try:
                cookies.load(self.headers.get("Cookie", ""))
            except CookieError:
                cookies = SimpleCookie()

            def present(name):
                return name in cookies and cookies[name].value == token

            persistent = "present" if present("light_restart_persistent") else "missing"
            session = "present" if present("light_restart_session") else "missing"
            seed = "true" if path == "/seed" else "false"
            body = f"""<!doctype html><html lang="en"><meta charset="utf-8">
<title>Restart persistence fixture</title><body>
<h1>Restart persistence fixture</h1>
<p>Persistent cookie: {persistent}</p><p>Session cookie: {session}</p>
<p id="storage">Local storage: waiting</p>
<script>
try {{
  const token = {json.dumps(token)};
  const key = "light-browser-restart-fixture";
  if ({seed}) localStorage.setItem(key, token);
  document.getElementById("storage").textContent =
    "Local storage: " + (localStorage.getItem(key) === token ? "present" : "missing");
}} catch (_) {{ document.getElementById("storage").textContent = "Local storage: error"; }}
</script></body></html>""".encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            if path == "/seed":
                # Only synthetic fixture cookies; persistence policy belongs to the app.
                self.send_header("Set-Cookie", f"light_restart_persistent={token}; Path=/; Max-Age=3600; HttpOnly; SameSite=Lax")
                self.send_header("Set-Cookie", f"light_restart_session={token}; Path=/; HttpOnly; SameSite=Lax")
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, _format, *_args):
            pass

    return PersistenceHandler


def wait_ready(cli, process, socket):
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        require(process.poll() is None, f"App exited during startup with code {process.returncode}")
        if socket.exists():
            response = cli.request("status")
            if response["ok"]:
                if cli.success("tabs")["tabs"]:
                    return
            else:
                code = response.get("error", {}).get("code")
                require(code in ("APP_NOT_RUNNING", "APP_NOT_READY"), f"Startup status failed: {code}")
        time.sleep(0.1)
    raise CheckFailed("App startup timed out; inspect the retained log and any macOS approval dialog")


def persistence_page(cli, url):
    tab = cli.success("open", url)["tab"]
    try:
        return cli.wait_for_page(
            tab,
            lambda page: page.get("title") == "Restart persistence fixture" and
                         "Local storage: waiting" not in page.get("text", ""),
            "persistence fixture",
        )
    finally:
        cli.success("close", tab)


def check_storage(page):
    text = page.get("text", "")
    for expected in ("Persistent cookie: present", "Session cookie: present", "Local storage: present"):
        require(expected in text, f"Persistence check failed: {expected}")


def stop_app(cli, process):
    # Every tab belongs to the process launched with our brand-new test profile.
    for tab in cli.success("tabs")["tabs"]:
        response = cli.request("close", tab["tab"])
        require(response["ok"] or response.get("error", {}).get("code") == "TAB_NOT_FOUND",
                "App rejected graceful tab close")
    try:
        code = process.wait(timeout=15)
    except subprocess.TimeoutExpired as error:
        raise CheckFailed("App did not finish graceful shutdown within 15 seconds") from error
    require(code == 0, f"App shutdown returned code {code}")


def run(args):
    report = {"ok": False, "scope": "two app launches with one new local fixture profile", "cycles": []}
    data_dir = args.data_dir.absolute()
    created = False
    process = None
    server = None
    server_thread = None
    log = None
    try:
        executable = app_executable(args.app)
        cli_path = args.cli.resolve(strict=True)
        require(cli_path.is_file() and os.access(cli_path, os.X_OK), "CLI executable is unavailable")
        require(not data_dir.exists() and not data_dir.is_symlink(),
                "Refusing existing data directory; supply a new dedicated path")
        require(data_dir.parent.is_dir(), "Data directory parent must already exist")
        data_dir = data_dir.resolve()
        require(len(os.fsencode(data_dir / "control.sock")) < 104, "Data directory is too long for a macOS Unix socket")
        data_dir.mkdir(mode=0o700)
        created = True
        token = uuid.uuid4().hex
        (data_dir / ".restart-smoke-owner.json").write_text(
            json.dumps({"test": "restart_smoke", "run": token}), encoding="utf-8")
        report["dataDir"] = str(data_dir)
        cli = BrowserCli(cli_path, data_dir)
        environment = dict(os.environ, LIGHT_BROWSER_DATA_DIR=str(data_dir))
        environment.setdefault("MallocNanoZone", "0")
        server = ThreadingHTTPServer(("127.0.0.1", 0), persistence_handler(token))
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        origin = f"http://127.0.0.1:{server.server_port}"

        for cycle in (1, 2):
            print(f"Restart smoke: launching cycle {cycle} with the same dedicated profile", file=sys.stderr, flush=True)
            log_path = data_dir / f"launch-{cycle}.log"
            log = log_path.open("wb")
            process = subprocess.Popen([str(executable)], env=environment,
                                       stdout=log, stderr=subprocess.STDOUT)
            wait_ready(cli, process, data_dir / "control.sock")
            smoke_report = run_smoke(SimpleNamespace(cli=cli_path, data_dir=data_dir,
                                                    artifacts_dir=data_dir / f"cycle-{cycle}"))
            cycle_report = {"cycle": cycle, "smoke": smoke_report, "log": str(log_path)}
            report["cycles"].append(cycle_report)
            require(smoke_report["ok"], f"Cycle {cycle} local smoke failed: {smoke_report.get('error', 'cleanup failure')}")
            if cycle == 1:
                seeded = persistence_page(cli, origin + "/seed")
                require("Local storage: present" in seeded.get("text", ""), "Local storage seed failed")
            checked = persistence_page(cli, origin + "/check")
            check_storage(checked)
            cycle_report["storage"] = {"persistentCookie": True, "sessionCookie": True, "localStorage": True}
            stop_app(cli, process)
            cycle_report["gracefulShutdown"] = True
            process = None
            log.close()
            log = None
        report["ok"] = True
    except (CheckFailed, OSError, ValueError, KeyError, TypeError) as error:
        report["error"] = str(error)
    finally:
        if process is not None and process.poll() is None:
            # Failure cleanup targets only the exact subprocess created above.
            # A forced stop never counts as successful persistence verification.
            report["ok"] = False
            report["forcedCleanup"] = True
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        if log:
            log.close()
        if server:
            server.shutdown()
            server.server_close()
        if server_thread:
            server_thread.join(timeout=2)
        if created:
            (data_dir / "restart-report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--app", type=Path, required=True, help="App bundle or its main executable")
    parser.add_argument("--cli", type=Path, required=True, help="Public lightbrowser CLI executable")
    parser.add_argument("--data-dir", type=Path, required=True, help="New dedicated directory; existing paths are refused")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("This app restart test requires macOS")
    report = run(args)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
