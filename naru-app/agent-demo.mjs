// Naru 에이전트 데모 — CDP로 Naru 탭을 외부에서 조종 (ego lite식)
const list = await (await fetch("http://localhost:9222/json/list")).json();
const page = list.find(t => t.type === "page");
if (!page) { console.log("조종할 page 타깃 없음"); process.exit(1); }
console.log("[agent] 조종 대상 탭:", JSON.stringify({ title: page.title, url: page.url }));

const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0; const pending = new Map();
const send = (method, params = {}) => new Promise((res) => {
  const mid = ++id; pending.set(mid, res);
  ws.send(JSON.stringify({ id: mid, method, params }));
});
ws.addEventListener("message", (e) => {
  const m = JSON.parse(e.data.toString());
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
});
await new Promise((r, j) => { ws.addEventListener("open", r); ws.addEventListener("error", j); });

await send("Page.enable");
await send("Runtime.enable");
console.log("[agent] → github.com 으로 이동 (에이전트 제어)…");
await send("Page.navigate", { url: "https://github.com" });
await new Promise(r => setTimeout(r, 4500));
const title = (await send("Runtime.evaluate", { expression: "document.title", returnByValue: true })).result?.result?.value;
const href  = (await send("Runtime.evaluate", { expression: "location.href", returnByValue: true })).result?.result?.value;
const h1     = (await send("Runtime.evaluate", { expression: "(document.querySelector('h1')||{}).innerText||''", returnByValue: true })).result?.result?.value;
console.log("[agent] 이동 완료 — title:", title);
console.log("[agent]              url  :", href);
console.log("[agent]              h1   :", (h1||"").slice(0,60));
ws.close();
console.log("[agent] 완료. Naru 창에서 github로 바뀌었는지 보세요.");
