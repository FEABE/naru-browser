#!/usr/bin/env python3
"""Journey 08 crash regressions against an already running Naru app (local CEF integration).

Repeats the scenarios that ended the v14 app once OOPIF child sessions were tracked:
frames then main navigate/reload, tracked OOPIF swapped to in-process (by click and
during a busy collection), OOPIF churn with repeated frames, alternating-site chain
depth 8/9/10, and 16/17 OOPIFs with three consecutive frames calls. Pass means the app
keeps answering and every frames answer is a known error code or a complete list
(or truncated:true). Same environment guard as frame_journey_smoke.py.

  python3 tests/frame_crash_smoke.py [--cli PATH] [--reps 5] [--only NAME ...]
"""

import argparse
import json
import re
import sys
import time

from frame_journey_smoke import ROOT, Cli, guard_environment, start_server

ALLOWED = {"ok", "FRAME_CHANGED", "FRAME_LIMIT", "FRAME_CONTROL_UNAVAILABLE", "STALE_FRAME_SNAPSHOT", "PAGE_LOADING", "FRAME_NOT_FOUND"}


def code(response):
    return "ok" if response.get("ok") else (response.get("error") or {}).get("code", "?")


def summary(response):
    if not response.get("ok"):
        return code(response)
    frames = response["result"]["frames"]
    return {"n": len(frames), "sessions": sum(bool(f["sessionId"]) for f in frames),
            "truncated": response["result"].get("truncated"),
            "names": sorted({m[1] for f in frames if (m := re.search(r"[?&]name=([A-Z_]+)", f["url"]))})}


def run(cli, origin, reps, only):
    results = []
    task = cli("task", "create", "frame crash smoke")["result"]["task"]
    tid, rev = task["id"], task["revision"]

    def task_run(*args, timeout=60):
        return cli("task", "run", tid, rev, *args, timeout=timeout)

    def alive():
        return cli("status").get("ok") is True

    def open_tab(path):
        tab = task_run("open", origin + path)["result"]["tab"]
        task_run("wait", tab, "load", 20000)
        return tab

    def frames_until(tab, done, tries=40):
        last = None
        for _ in range(tries):
            last = task_run("frames", tab)
            if last.get("ok") and done(last["result"]["frames"]):
                return last
            time.sleep(0.25)
        return last

    def has_cross(frames):
        return any("name=CROSS" in f["url"] and f["sessionId"] for f in frames)

    def root_click(tab, label):
        page = task_run("read", tab)["result"]["page"]
        ref = next(i["ref"] for i in page["interactive"] if i.get("label") == label)
        return code(task_run("click", tab, ref))

    def main_nav(kind):
        tab = open_tab("/f/root.html")
        steps = [("frames", summary(frames_until(tab, has_cross)))]
        response = task_run("navigate", tab, origin + "/f/root.html") if kind == "navigate" else task_run("reload", tab)
        steps.append((kind, code(response)))
        steps.append(("wait", code(task_run("wait", tab, "load", 20000))))
        steps.append(("frames-after", summary(frames_until(tab, has_cross))))
        return tab, steps, steps[-1][1] != "?" and isinstance(steps[-1][1], dict) and steps[-1][1]["sessions"] >= 2

    def swap_click():
        tab = open_tab("/f/root.html?same=&xo=")
        steps = [("frames", summary(frames_until(tab, has_cross)))]
        frame = next(f for f in task_run("frames", tab)["result"]["frames"] if "name=CROSS&" in f["url"])
        steps.append(("read-frame CROSS", code(task_run("read-frame", tab, frame["frameId"]))))
        steps.append(("click swap cross", root_click(tab, "swap cross")))
        settled = frames_until(tab, lambda fs: any("name=SWAPPED" in f["url"] for f in fs))
        steps.append(("frames-after", summary(settled)))
        again = task_run("read-frame", tab, frame["frameId"])
        # The frame ID survives the swap; a new read must return the current (SWAPPED) document only.
        steps.append(("read-frame old id", code(again) if not again.get("ok") else
                      sorted(set(re.findall(r"[A-Z]+_DOC_MARK", again["result"]["page"]["text"])))))
        return tab, steps, (isinstance(steps[-2][1], dict) and steps[-2][1]["sessions"] == 0
                            and steps[-1][1] in (["SWAPPED_DOC_MARK"],) + tuple(ALLOWED - {"ok"}))

    def swap_collect():
        # Cross leaf keeps its main thread busy; the root swaps it in-process 7 s after load.
        tab = open_tab("/f/root.html?same=&xo=&cross=name%3DBUSY%26busy%3D3000&act=swap%3Across%3A7000")
        steps = [("frames#%d" % i, summary(task_run("frames", tab))) for i in range(1, 4)]
        settled = frames_until(tab, lambda fs: any("name=SWAPPED" in f["url"] for f in fs))
        steps.append(("frames-settled", summary(settled)))
        return tab, steps, isinstance(steps[-1][1], dict)

    def churn():
        tab = open_tab("/f/many.html?n=2&churn=1&cross=1")
        steps = [("frames#%d" % i, summary(task_run("frames", tab))) for i in range(1, 11)]
        return tab, steps, True

    def chain(depth):
        tab = open_tab(f"/f/chain.html?d={depth}&alt=1")
        time.sleep(2)
        steps = [("frames#%d" % i, summary(task_run("frames", tab))) for i in range(1, 4)]
        # A complete answer lists depth+1 documents; anything shorter must say truncated.
        complete = all(not isinstance(s, dict) or s["n"] == depth + 1 or s["truncated"] for _, s in steps)
        return tab, steps, complete

    def many(count):
        tab = open_tab(f"/f/many.html?n={count}&cross=1")
        time.sleep(2)
        steps = [("frames#%d" % i, summary(task_run("frames", tab))) for i in range(1, 4)]
        complete = all(not isinstance(s, dict) or s["n"] == count + 1 or s["truncated"] for _, s in steps)
        return tab, steps, complete

    scenarios = {
        "frames-navigate": lambda: main_nav("navigate"),
        "frames-reload": lambda: main_nav("reload"),
        "oopif-swap-click": swap_click,
        "oopif-swap-during-collect": swap_collect,
        "oopif-churn-10x": churn,
        "chain-alt-d8": lambda: chain(8),
        "chain-alt-d9": lambda: chain(9),
        "chain-alt-d10": lambda: chain(10),
        "oopif-n16-3x": lambda: many(16),
        "oopif-n17-3x": lambda: many(17),
    }
    try:
        for name, scenario in scenarios.items():
            if only and name not in only:
                continue
            for rep in range(1, reps + 1):
                tab, steps, complete = scenario()
                codes = [s if isinstance(s, str) else "ok" for _, s in steps if not isinstance(s, list)]
                up = alive()
                results.append({"scenario": name, "rep": rep, "alive": up, "steps": steps,
                                "codesAllowed": all(c in ALLOWED for c in codes), "complete": complete,
                                "close": code(task_run("close", tab))})
                print(json.dumps(results[-1], ensure_ascii=False), file=sys.stderr, flush=True)
                if not up:
                    return results
                time.sleep(0.5)
    finally:
        if alive():
            info = cli("task", "info", tid)["result"]["task"]
            if info["status"] != "finished":
                cli("task", "finish", tid, info["revision"])
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", default=ROOT / "dist/Light Browser.app/Contents/MacOS/lightbrowser")
    parser.add_argument("--reps", type=int, default=5)
    parser.add_argument("--only", nargs="*")
    args = parser.parse_args()
    guard_environment()
    servers = start_server(0)
    cli = Cli(args.cli)
    before = cli("spaces")["result"]
    try:
        results = run(cli, f"http://127.0.0.1:{servers[0].server_port}", args.reps, set(args.only or ()))
    finally:
        for server in servers:
            server.shutdown()
    after = cli("spaces").get("result") or {}
    user = {"before": [before["activeSpace"], before["activeTab"]], "after": [after.get("activeSpace"), after.get("activeTab")]}
    ok = (all(r["alive"] and r["codesAllowed"] and r["complete"] for r in results) and user["before"] == user["after"])
    print(json.dumps({"ok": ok, "user": user, "results": results, "commands": cli.log}, ensure_ascii=False, indent=1))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
