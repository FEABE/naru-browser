#!/usr/bin/env python3
"""Journey 05 CLI half (handoff/resume revision protection) against a running Naru.
Env must be the e2e-v17 profile. Usage: e2e_v17_j05.py PORT   (fixture server already running)
"""
import json, subprocess, sys, time
from e2e_v17_cli import cli, code, OUT, CLI

port = sys.argv[1]
base = f"http://127.0.0.1:{port}"
res = []


def rec(step, resp, expect, extra=None):
    c = code(resp)
    ok = (c == expect) if isinstance(expect, str) else expect(resp)
    res.append({"step": step, "code": c, "pass": bool(ok), "extra": extra})
    print(("PASS " if ok else "FAIL ") + step, c, extra or "")
    return resp


def user_state():
    s = cli("status")["result"]
    t = [(x["tab"], x["url"], x["active"]) for x in cli("tabs")["result"]["tabs"] if x["space"] == "user"]
    return {"activeSpace": s["activeSpace"], "activeTab": s["activeTab"], "userTabs": t}


def new_task(name):
    return cli("task", "create", name)["result"]["task"]


def run(t, rev, *a):
    return cli("task", "run", t, rev, *a)


def wait_ready(t, rev, tab):
    return run(t, rev, "wait", tab, "load", 15000)


before = user_state()
A, B = new_task("e2e-v17 A"), new_task("e2e-v17 B")
ia, ib = A["id"], B["id"]
ra, rb = A["revision"], B["revision"]
ta = run(ia, ra, "open", base + "/a")["result"]["tab"]
tb = run(ib, rb, "open", base + "/b")["result"]["tab"]
rec("A wait load", wait_ready(ia, ra, ta), "ok")
rec("B wait load", wait_ready(ib, rb, tb), "ok")
pa = run(ia, ra, "read", ta)
rec("A read (marker-a)", pa, lambda r: "marker-a" in json.dumps(r))
refa = [e["ref"] for e in pa["result"]["page"]["interactive"] if "field" in json.dumps(e)][0]
ref_dbg = list(pa["result"]["page"].keys())

h = cli("task", "handoff", ia, ra)
rec("A handoff rev=%s" % ra, h, "ok", h.get("result", {}).get("task", {}).get("revision"))
rh = h["result"]["task"]["revision"]
rec("A old-rev read rejected", run(ia, ra, "read", ta), lambda r: not r["ok"], code(run(ia, ra, "read", ta)))
rec("A old-rev fill rejected", run(ia, ra, "fill", ta, refa, "x"), lambda r: not r["ok"])
rec("A new-rev (handed-off) read", run(ia, rh, "read", ta), lambda r: True, code(run(ia, rh, "read", ta)))
rec("A task info status", cli("task", "info", ia), lambda r: r["ok"], cli("task", "info", ia)["result"]["task"]["status"])
# B keeps working
pb = run(ib, rb, "read", tb)
rec("B read during A handoff", pb, "ok")
refb = [e["ref"] for e in pb["result"]["page"]["interactive"] if "field" in json.dumps(e)][0]
rec("B fill during A handoff", run(ib, rb, "fill", tb, refb, "b-typed"), "ok")
rec("B read shows fill", run(ib, rb, "read", tb), lambda r: "b-typed" in json.dumps(r))
rec("B open second tab during A handoff", run(ib, rb, "open", base + "/user"), "ok")

r = cli("task", "resume", ia, rh)
rec("A resume with handoff rev", r, "ok", r.get("result", {}).get("task", {}).get("revision"))
rr = r["result"]["task"]["revision"]
rec("A pre-resume rev rejected after resume", run(ia, rh, "read", ta), lambda r: not r["ok"], code(run(ia, rh, "read", ta)))
rec("A original rev rejected after resume", run(ia, ra, "read", ta), lambda r: not r["ok"])
pa2 = run(ia, rr, "read", ta)
rec("A read with new rev", pa2, "ok")
refa2 = [e["ref"] for e in pa2["result"]["page"]["interactive"] if "field" in json.dumps(e)][0]
rec("A stale pre-handoff ref fill", run(ia, rr, "fill", ta, refa, "stale"), lambda r: True, code(run(ia, rr, "fill", ta, refa, "stale2")))
rec("A fresh-ref fill", run(ia, rr, "fill", ta, refa2, "a-typed"), "ok")
rec("A read shows fill", run(ia, rr, "read", ta), lambda r: "a-typed" in json.dumps(r))

# in-flight wait cancelled by handoff; slow page response must not be applied to the old revision
rec("A navigate /slow", run(ia, rr, "navigate", ta, base + "/slow"), "ok")
t0 = time.time()
p = subprocess.Popen([CLI, "task", "run", ia, str(rr), "wait", str(ta), "load", "25000"], stdout=subprocess.PIPE, text=True)
time.sleep(1.5)
h2 = cli("task", "handoff", ia, rr)
rh2 = h2["result"]["task"]["revision"]
out = json.loads(p.communicate(timeout=40)[0])
res.append({"step": "in-flight wait (25s limit) during handoff", "code": code(out), "elapsedMs": int((time.time() - t0) * 1000),
            "pass": not out["ok"] and (time.time() - t0) < 10})
print(res[-1])
time.sleep(9)
rec("A tab after /slow completes (read w/ old rev rejected)", run(ia, rr, "read", ta), lambda r: not r["ok"])
r = cli("task", "resume", ia, rh2)
rr2 = r["result"]["task"]["revision"]
rec("A resume after in-flight", r, "ok")
rec("A read after resume (page state)", run(ia, rr2, "read", ta), "ok",
    run(ia, rr2, "read", ta).get("result", {}).get("page", {}).get("title"))

after = user_state()
res.append({"step": "user space/selection unchanged", "before": before, "after": after, "pass": before == after})
print(res[-1])
for t, rv in ((ia, rr2), (ib, rb)):
    f = cli("task", "finish", t, rv)
    res.append({"step": "finish " + t, "code": code(f), "pass": f["ok"]})
final = user_state()
res.append({"step": "after finish user state", "after": final, "pass": final["userTabs"] == before["userTabs"]})
json.dump(res, open(OUT / "j05.json", "w"), ensure_ascii=False, indent=1)
print("ALL PASS" if all(x["pass"] for x in res) else "SOME FAIL")
