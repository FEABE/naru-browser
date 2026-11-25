// Naru 에이전트 커서 데모 — 사람처럼 커서 이동 + 링크 클릭 (Naru가 휴먼라이크 입력 실행)
import { writeFile } from "node:fs/promises";
const CTRL = "C:/Users/Lenovo/AppData/Local/Temp/naru_control.txt";
const sleep = ms => new Promise(r => setTimeout(r, ms));
const send = async s => { await writeFile(CTRL, s, "utf8"); };
const listTargets = async () => (await (await fetch("http://localhost:9222/json/list")).json()).filter(t => t.type === "page");

const before = new Set((await listTargets()).map(t => t.id));
await send("agent_tab|https://en.wikipedia.org/wiki/Web_browser|research · web browser");
console.log("[agent] 새 에이전트 탭 요청");
let target = null;
for (let i = 0; i < 40; i++) { const now = await listTargets(); target = now.find(t => !before.has(t.id)); if (target && target.webSocketDebuggerUrl) break; await sleep(500); }
if (!target) { console.log("타깃 없음"); process.exit(1); }
console.log("[agent] 탭 확보:", target.url);

const ws = new WebSocket(target.webSocketDebuggerUrl);
let id = 0; const pend = new Map();
const cdp = (m, p = {}) => new Promise(r => { const i = ++id; pend.set(i, r); ws.send(JSON.stringify({ id: i, method: m, params: p })); });
ws.addEventListener("message", e => { const m = JSON.parse(e.data.toString()); if (m.id && pend.has(m.id)) { pend.get(m.id)(m); pend.delete(m.id); } });
await new Promise((r, j) => { ws.addEventListener("open", r); ws.addEventListener("error", j); });
await cdp("Runtime.enable");
await sleep(2800);

// 사람처럼 커서를 움직이며 둘러봄 (라벨 = 'AI가 쓰는 느낌')
await send("agent_move|260|200|페이지 훑어보는 중…"); await sleep(1900);
await send("agent_move|640|430|본문 읽는 중…"); await sleep(1900);

// 링크 좌표를 CDP로 찾아 → Naru에 휴먼라이크 클릭 지시
const r = await cdp("Runtime.evaluate", { expression: `(()=>{const a=[...document.querySelectorAll('#bodyContent a[href^="/wiki/"]')].find(x=>{const b=x.getBoundingClientRect();return b.top>160&&b.top<560&&b.width>30;});if(!a)return null;const b=a.getBoundingClientRect();return JSON.stringify({x:Math.round(b.x+b.width/2),y:Math.round(b.y+b.height/2),t:a.innerText.slice(0,18)});})()`, returnByValue: true });
const link = JSON.parse(r.result?.result?.value || "null");
if (link) {
  console.log("[agent] 링크 클릭 지시:", link.t, "@", link.x, link.y);
  await send(`agent_click|${link.x}|${link.y}|링크 클릭: ${link.t}`);
  await sleep(3500);
  const title = (await cdp("Runtime.evaluate", { expression: "document.title", returnByValue: true })).result?.result?.value;
  console.log("[agent] 클릭 후 제목:", title);
} else { console.log("[agent] 적당한 링크 못 찾음"); }
ws.close();
console.log("[agent] 완료 — Naru에서 인디고 커서가 움직이며 클릭했는지 보세요.");
