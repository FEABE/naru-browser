// Naru 에이전트 데모 v2 (ego browser식) — 새 에이전트 탭을 열어 거기서 제어.
import { writeFile } from "node:fs/promises";
const CTRL = "C:/Users/Lenovo/AppData/Local/Temp/naru_control.txt";
const sleep = (ms) => new Promise(r => setTimeout(r, ms));
const listTargets = async () => (await (await fetch("http://localhost:9222/json/list")).json()).filter(t => t.type === "page");

// 0. 제어 전 기존 탭 스냅샷
const before = new Set((await listTargets()).map(t => t.id));

// 1. Naru에 "새 에이전트 탭 열어줘" 요청 (유저 탭은 안 건드림)
await writeFile(CTRL, "agent_tab|https://duckduckgo.com/?q=sf+parks+national|research · sf parks", "utf8");
console.log("[agent] Naru에 새 에이전트 탭 요청 → 하단에 'agent in control' 떠야 함");

// 2. 새로 생긴 탭의 CDP 타깃 찾기
let target = null;
for (let i = 0; i < 40; i++) {
  const now = await listTargets();
  target = now.find(t => !before.has(t.id));
  if (target && target.webSocketDebuggerUrl) break;
  await sleep(500);
}
if (!target) { console.log("[agent] 새 탭 타깃 못 찾음"); process.exit(1); }
console.log("[agent] 새 에이전트 탭 확보:", target.url);

// 3. 그 탭을 CDP로 제어
const ws = new WebSocket(target.webSocketDebuggerUrl);
let id = 0; const pending = new Map();
const send = (method, params = {}) => new Promise((res) => { const m = ++id; pending.set(m, res); ws.send(JSON.stringify({ id: m, method, params })); });
ws.addEventListener("message", (e) => { const m = JSON.parse(e.data.toString()); if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } });
await new Promise((r, j) => { ws.addEventListener("open", r); ws.addEventListener("error", j); });
await send("Page.enable"); await send("Runtime.enable");

await sleep(2500);
let title = (await send("Runtime.evaluate", { expression: "document.title", returnByValue: true })).result?.result?.value;
console.log("[agent] (에이전트 탭에서 작업 중) 현재 제목:", title);
console.log("[agent] → en.wikipedia.org/wiki/Golden_Gate_Park 로 이동");
await send("Page.navigate", { url: "https://en.wikipedia.org/wiki/Golden_Gate_Park" });
await sleep(3500);
title = (await send("Runtime.evaluate", { expression: "document.title", returnByValue: true })).result?.result?.value;
const first = (await send("Runtime.evaluate", { expression: "(document.querySelector('#firstHeading')||{}).innerText||''", returnByValue: true })).result?.result?.value;
console.log("[agent] 완료 — 제목:", title, "| 헤딩:", first);
ws.close();
console.log("[agent] Naru 사이드바에 새 'research · sf parks' 에이전트 탭 + 하단 pill 확인하세요.");
