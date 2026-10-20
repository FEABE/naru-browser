#!/usr/bin/env python3
"""e2e-v17 journey 13 cookie/quit cycles. Usage: e2e_v17_j13.py MODE(closelasttab|sigterm) N BASEPORT
Own fixture server per cycle with a fresh token; app on .local-data/e2e-v17-cli only."""
import json, os, signal, subprocess, sys, time, uuid
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
D = os.environ["LIGHT_BROWSER_DATA_DIR"]; assert D == str(ROOT / ".local-data/e2e-v17-cli") and os.environ["NARU_MOCK_KEYCHAIN"] == "1"
APP = str(ROOT / "dist/Light Browser.app/Contents/MacOS/light-browser"); CLI = APP.replace("light-browser", "lightbrowser")
OUT = ROOT / "artifacts/e2e-v17/cli"
mode, n, base = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])

def cli(*a, t=20):
    p = subprocess.run([CLI, *a], capture_output=True, text=True, timeout=t)
    try: return json.loads(p.stdout)
    except ValueError: return {"ok": False, "raw": p.stdout}
def launch():
    p = subprocess.Popen([APP], stdout=open(D + "/app.out", "a"), stderr=subprocess.STDOUT, start_new_session=True)
    for _ in range(200):
        if cli("status").get("ok"): return p
        time.sleep(.3)
    raise SystemExit("no start")
def page(port, path):
    cli("navigate", "1", f"http://127.0.0.1:{port}{path}"); cli("wait", "1", "load", "10000")
    return cli("read", "1")["result"]["page"]["text"]
def gone(proc, lim=40):
    t0 = time.time()
    while time.time() - t0 < lim:
        if proc.poll() is not None: return round(time.time() - t0, 2)
        time.sleep(.02)
    return None
res = []
for i in range(n):
    port, tok = base + i, uuid.uuid4().hex[:12]
    srv = subprocess.Popen([sys.executable, str(ROOT / "tests/e2e_v17_fixture_server.py"), str(port), str(OUT / f"access-{mode}-{port}.log"), tok])
    time.sleep(.7)
    row = {"mode": mode, "cycle": i, "port": port}
    app = launch()
    try:
        s = page(port, "/seed"); row["seed"] = [l for l in s.splitlines() if "cookie" in l.lower() or "Local" in l]
        s = page(port, "/check"); row["same_session"] = [l for l in s.splitlines() if "cookie" in l.lower()]
        t0 = time.time()
        if mode == "sigterm": os.kill(app.pid, signal.SIGTERM)
        else: row["close_resp"] = "ok" if cli("close", "1", t=40).get("ok") else "fail/timeout"
        row["quit_s"] = gone(app); row["quit_wall"] = round(time.time() - t0, 2)
        row["rc"] = app.returncode
        row["sock_after_quit"] = (Path(D) / "control.sock").exists()
        row["children_left"] = int(subprocess.run(f"ps -axo pid,command | grep -F '{D}/profile-v1' | grep -v grep | wc -l", shell=True, capture_output=True, text=True).stdout)
        if row["quit_s"] is None: app.kill()
        app = launch()
        s = page(port, "/check"); row["after_restart"] = [l for l in s.splitlines() if "cookie" in l.lower() or "Local" in l]
        row["pass"] = row["quit_s"] is not None and all("present" in l for l in row["after_restart"])
    finally:
        cli("close", "1", t=40); gone(app, 15)
        app.kill()
        srv.terminate(); srv.wait()
    print(json.dumps(row, ensure_ascii=False)); res.append(row)
json.dump(res, open(OUT / f"j13-{mode}.json", "w"), ensure_ascii=False, indent=1)
