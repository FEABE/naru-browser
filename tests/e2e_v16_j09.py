#!/usr/bin/env python3
"""Journey 09 CLI-reachable probe: task-scoped download start/cancel against tests/download_fixture.py on PORT."""
import json, sys, time, urllib.request
from e2e_v16_cli import cli, code, OUT

port = sys.argv[1]
base = f"http://127.0.0.1:{port}"
st = lambda: json.load(urllib.request.urlopen(base + "/state"))
dl = lambda: cli("status")["result"]["downloads"]
res = []
def rec(s, **k):
    k["step"] = s; res.append(k); print(json.dumps(k, ensure_ascii=False))

t = cli("task", "create", "e2e-v16 dl")["result"]["task"]; tid, rev = t["id"], t["revision"]
run = lambda *a: cli("task", "run", tid, rev, *a)
tab = run("open", base + "/")["result"]["tab"]
rec("wait", code=code(run("wait", tab, "load", 15000)))
def click(label):
    p = run("read", tab)["result"]["page"]
    ref = next(e["ref"] for e in p["interactive"] if e.get("label") == label)
    return run("click", tab, ref)
# visible selection so the permission/save flow is not hidden-tab (explicit; user space is unchanged only if we restore it)
before = st()
rec("click small", code=code(click("작은 파일 저장")))
time.sleep(3)
rec("after small", downloads=dl(), server=st(), serverDelta={k: st()["requests"][k] - before["requests"][k] for k in before["requests"]})
rec("click slow", code=code(click("느린 파일 저장")))
time.sleep(3)
rec("after slow", downloads=dl(), server=st())
rec("tab close (cancel)", code=code(run("close", tab)))
time.sleep(4)
rec("after close", downloads=dl(), server=st())
f = cli("task", "finish", tid, rev); rec("finish", code=code(f))
rec("final", downloads=dl(), server=st())
json.dump(res, open(OUT / "j09.json", "w"), ensure_ascii=False, indent=1)
