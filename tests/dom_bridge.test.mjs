import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";

const source = await readFile(new URL("../src/dom_bridge.js", import.meta.url), "utf8");

// A minimal DOM double exercises ref and disclosure boundaries without a browser.
// Browser compatibility, layout and real website behavior require a CEF smoke test.
function fixture() {
  const document = { title: "Fixture", body: null, elements: [] };
  document.querySelectorAll = () => document.elements;
  document.getElementById = (id) => document.elements.find((element) => element.id === id) || null;
  class Element {
    constructor(tagName, attributes = {}) {
      this.tagName = tagName;
      this.attributes = attributes;
      this.ownerDocument = document;
      this.isConnected = true;
      this.innerText = "";
      this.type = attributes.type || "text";
      this.events = [];
      this._value = "";
      this.style = { display: "block", visibility: "visible" };
      this.form = null;
    }
    getAttribute(name) { return this.attributes[name] ?? null; }
    closest(selector) {
      if (selector.includes("aria-disabled")) return this.attributes["aria-disabled"] === "true" ? this : null;
      return this.hidden || this.inert || this.attributes["aria-hidden"] === "true" ? this : null;
    }
    matches() { return !!this.disabled; }
    getClientRects() { return this.rects ?? [{}]; }
    click() { this.clicked = (this.clicked || 0) + 1; }
    focus() { this.onFocus?.(); }
    dispatchEvent(event) {
      this.events.push(event.type);
      this.onEvent?.(event);
      return !event.defaultPrevented;
    }
  }
  class Input extends Element {
    constructor(attributes) { super("INPUT", attributes); }
    get value() { return this._value; }
    set value(value) { this._value = value; }
  }
  class TextArea extends Element {
    constructor(attributes) { super("TEXTAREA", attributes); }
    get value() { return this._value; }
    set value(value) { this._value = value; }
  }
  document.body = new Element("BODY");
  document.body.innerText = "Fixture page";
  const window = {
    innerHeight: 800, scrollX: 0, scrollY: 0,
    getComputedStyle: (element) => element.style,
    scrollBy({ left, top }) { this.scrollX += left; this.scrollY += top; },
  };
  class KeyboardEvent extends Event {
    constructor(type, options) { super(type, options); this.key = options.key; }
  }
  const context = vm.createContext({
    window, document, location: { href: "https://example.test/" }, crypto: webcrypto,
    HTMLInputElement: Input, HTMLTextAreaElement: TextArea, Event, KeyboardEvent,
  });
  vm.runInContext(source, context);
  return {
    bridge: window.__lightBrowserBridge, document, window, context,
    add(tagName = "BUTTON", attributes = {}) {
      const element = tagName === "INPUT" ? new Input(attributes)
        : tagName === "TEXTAREA" ? new TextArea(attributes) : new Element(tagName, attributes);
      document.elements.push(element);
      return element;
    },
  };
}

test("refs expire after another read and never alias a new document", () => {
  const first = fixture();
  const button = first.add();
  const staleRef = first.bridge.read().interactive[0].ref;
  const currentRef = first.bridge.read().interactive[0].ref;
  assert.equal(first.bridge.act({ action: "click", ref: staleRef }).error, "STALE_REF");
  assert.equal(first.bridge.act({ action: "click", ref: currentRef }).ok, true);
  assert.equal(button.clicked, 1);
  const second = fixture();
  second.add();
  const secondRef = second.bridge.read().interactive[0].ref;
  assert.notEqual(currentRef, secondRef);
  assert.equal(second.bridge.act({ action: "click", ref: currentRef }).error, "STALE_REF");
});

test("removed, hidden, disabled and inert elements are checked at action time", () => {
  for (const state of ["isConnected", "hidden", "disabled", "inert"]) {
    const f = fixture();
    const button = f.add();
    const ref = f.bridge.read().interactive[0].ref;
    button[state] = state !== "isConnected";
    assert.equal(f.bridge.act({ action: "click", ref }).ok, false, state);
    assert.equal(button.clicked, undefined, state);
  }
});

test("password values never appear in snapshots, fill results or exception results", () => {
  const f = fixture();
  const password = f.add("INPUT", { type: "password", "aria-label": "Password" });
  password.value = "old-secret";
  const snapshot = f.bridge.read();
  assert.equal(snapshot.interactive[0].value, undefined);
  assert.equal(JSON.stringify(snapshot).includes("old-secret"), false);
  const request = { action: "fill", ref: snapshot.interactive[0].ref, text: "new-secret" };
  assert.equal(f.bridge.act(request).ok, true);
  assert.equal(password.value, "new-secret");
  assert.equal(JSON.stringify(f.bridge.act(request)).includes("new-secret"), false);
  password.onEvent = () => { throw new Error("new-secret"); };
  const failure = f.bridge.act(request);
  assert.equal(failure.error, "ACTION_FAILED");
  assert.equal(JSON.stringify(failure).includes("new-secret"), false);
});

test("fill uses the native value setter and stops if focus replaces the element", () => {
  const f = fixture();
  const input = f.add("INPUT");
  Object.defineProperty(input, "value", {
    get() { return this._value; },
    set() { throw new Error("Framework setter must not intercept the native write"); },
  });
  const ref = f.bridge.read().interactive[0].ref;
  assert.equal(f.bridge.act({ action: "fill", ref, text: "hello" }).ok, true);
  assert.equal(input.value, "hello");
  assert.deepEqual(input.events, ["input", "change"]);
  input.onFocus = () => { input.isConnected = false; };
  assert.equal(f.bridge.act({ action: "fill", ref, text: "replacement" }).ok, false);
  assert.equal(input.value, "hello");
});

test("Enter respects cancelled keydown and reports a form submission request separately", () => {
  const f = fixture();
  const input = f.add("INPUT", { type: "search" });
  let submissions = 0;
  input.form = { requestSubmit() { submissions += 1; } };
  const ref = f.bridge.read().interactive[0].ref;
  assert.equal(f.bridge.act({ action: "press", ref, key: "Enter" }).formSubmissionRequested, true);
  assert.equal(submissions, 1);
  input.onEvent = (event) => { if (event.type === "keydown") event.preventDefault(); };
  assert.equal(f.bridge.act({ action: "press", ref, key: "Enter" }).formSubmissionRequested, false);
  assert.equal(submissions, 1);
  assert.equal(f.bridge.act({ action: "press", ref, key: "Tab" }).error, "UNSUPPORTED_KEY");
});

test("snapshot bounds text and interactive elements and excludes hidden content", () => {
  const f = fixture();
  f.document.body.innerText = "x".repeat(12001);
  f.add().hidden = true;
  for (let i = 0; i < 121; i += 1) f.add();
  const snapshot = f.bridge.read();
  assert.equal(snapshot.text.length, 12000);
  assert.equal(snapshot.interactive.length, 120);
  assert.equal(snapshot.truncated.text, true);
  assert.equal(snapshot.truncated.interactive, true);
});

test("an unrendered body cannot expose innerText's textContent fallback", () => {
  const f = fixture();
  f.document.body.style.display = "none";
  f.document.body.innerText = "hidden script text or password-like fixture value";
  assert.equal(f.bridge.read().text, "");
});

test("repeated injection preserves the current refs", () => {
  const f = fixture();
  const button = f.add();
  const ref = f.bridge.read().interactive[0].ref;
  vm.runInContext(source, f.context);
  assert.equal(f.window.__lightBrowserBridge.act({ action: "click", ref }).ok, true);
  assert.equal(button.clicked, 1);
});

test("scroll rejects nonfinite and excessive offsets", () => {
  const f = fixture();
  assert.equal(f.bridge.act({ action: "scroll", y: Infinity }).error, "INVALID_SCROLL");
  assert.equal(f.bridge.act({ action: "scroll", y: 10001 }).error, "INVALID_SCROLL");
  assert.equal(f.bridge.act({ action: "scroll", y: 400 }).ok, true);
  assert.equal(f.window.scrollY, 400);
});
