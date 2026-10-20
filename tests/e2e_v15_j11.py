#!/usr/bin/env python3
"""Journey 11 public-page half (no account, read-only). Task-scoped; follows one link per page via click."""
import json, sys
from e2e_v15_cli import cli, code, OUT

URLS = ["https://example.com/", "https://www.wikipedia.org/", "https://developer.mozilla.org/en-US/"]
res = []
t = cli("task", "create", "e2e-v15 public")["result"]["task"]
tid, rev = t["id"], t["revision"]
run = lambda *a: cli("task", "run", tid, rev, *a)
for i, u in enumerate(URLS):
    row = {"url": u}
    o = run("open", u); row["open"] = code(o)
    tab = o["result"]["tab"]
    row["wait"] = code(run("wait", tab, "load", 30000))
    r = run("read", tab); row["read"] = code(r)
    if r["ok"]:
        p = r["result"]["page"]
        row["title"] = p.get("title"); row["url_after"] = p.get("url")
        row["textLen"] = len(p.get("text", "")); row["textHead"] = p.get("text", "")[:100]
        links = [e for e in p["interactive"] if e.get("role") == "link"]
        row["links"] = len(links); row["interactive"] = len(p["interactive"])
    shot = OUT / f"j11-{i}.png"
    s = run("screenshot", tab, str(shot)); row["screenshot"] = code(s)
    row["shotBytes"] = shot.stat().st_size if shot.exists() else 0
    if r["ok"] and links:
        # first same-site-ish link with a label
        link = links[0]
        row["clicked"] = link.get("label")
        c = run("click", tab, link["ref"]); row["click"] = code(c)
        import time
        for _ in range(30):
            time.sleep(0.5)
            tabs = [x for x in run("tabs")["result"]["tabs"]]
            cur = next((x for x in tabs if x["tab"] == tab), None)
            if cur and cur["url"] != row["url_after"] and not cur["loading"]:
                break
        # the click may open a new tab in the task; report both
        row["tabs_after"] = [(x["tab"], x["url"]) for x in tabs]
        row["urlChanged"] = any(x["url"] != row["url_after"] and x["tab"] == tab for x in tabs)
    print(json.dumps(row, ensure_ascii=False)); res.append(row)
f = cli("task", "finish", tid, rev); res.append({"finish": code(f)})
json.dump(res, open(OUT / "j11.json", "w"), ensure_ascii=False, indent=1)
