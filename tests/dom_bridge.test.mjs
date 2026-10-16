import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";

const source = await readFile(new URL("../src/dom_bridge.js", import.meta.url), "utf8");

// A minimal DOM double exercises ref and disclosure boundaries without a browser.
// Browser compatibility, layout and real website behavior require a CEF smoke test.
function fixture() {
  let document;
  const metrics = { largestTextSlice: 0, largestRange: 0 };
  class Node {
    constructor(nodeType) {
      this.nodeType = nodeType;
      this.ownerDocument = document;
      this.parentNode = null;
      this.children = [];
      this.nextSibling = null;
    }
    get firstChild() { return this.children[0] || null; }
    get parentElement() { return this.parentNode?.nodeType === 1 ? this.parentNode : null; }
    get isConnected() {
      if (this.disconnected) return false;
      if (this.nodeType === 9) return true;
      return !!(this.parentNode || this.host)?.isConnected;
    }
    set isConnected(value) { this.disconnected = !value; }
    getRootNode() {
      let node = this;
      while (node.parentNode) node = node.parentNode;
      return node;
    }
    append(...nodes) {
      for (const node of nodes) {
        node.remove();
        if (this.children.length) this.children.at(-1).nextSibling = node;
        this.children.push(node);
        node.parentNode = this;
      }
    }
    remove() {
      if (!this.parentNode) return;
      const siblings = this.parentNode.children;
      const index = siblings.indexOf(this);
      if (index > 0) siblings[index - 1].nextSibling = this.nextSibling;
      siblings.splice(index, 1);
      this.parentNode = this.nextSibling = null;
    }
    get textContent() { return this.children.map((node) => node.textContent).join(""); }
    set textContent(value) {
      for (const child of this.children) child.parentNode = child.nextSibling = null;
      this.children = [];
      if (value) this.append(new Text(String(value)));
    }
    getElementById(id) {
      const pending = [...this.children];
      while (pending.length) {
        const node = pending.shift();
        if (node.id === id) return node;
        pending.unshift(...node.children);
      }
      return null;
    }
  }
  class Text extends Node {
    constructor(value) { super(3); this.data = value; }
    get length() { return this.data.length; }
    get textContent() { return this.data; }
    substringData(start, length) {
      metrics.largestTextSlice = Math.max(metrics.largestTextSlice, length);
      return this.data.slice(start, start + length);
    }
  }
  // Layout is deliberately small: model absence of boxes, not pixel geometry.
  // Inert/aria-hidden still have boxes, so the bridge must inspect those states.
  function hasBox(node) {
    if (!node.isConnected) return false;
    for (let current = node; current && current !== document;
         current = current.assignedSlot || current.parentNode || current.host) {
      if (current.hidden || current.style?.display === "none") return false;
      const parent = current.parentNode;
      if (parent?._shadowRoot && !current.assignedSlot) return false;
      if (parent?.tagName === "SLOT" && parent.assigned.length) return false;
    }
    return true;
  }
  class Element extends Node {
    constructor(tagName, attributes = {}) {
      super(1);
      this.tagName = tagName;
      this.namespaceURI = "http://www.w3.org/1999/xhtml";
      this.attributes = { ...attributes };
      this.type = attributes.type || "text";
      this.events = [];
      this._value = "";
      this.style = { display: "block", visibility: "visible" };
      this.form = null;
      this.labels = [];
      this.assignedSlot = null;
      this.assigned = [];
    }
    get id() { return this.getAttribute("id"); }
    set id(value) { this.setAttribute("id", value); }
    get innerText() { return this.textContent; }
    set innerText(value) { this.textContent = value; }
    get isContentEditable() {
      const value = this.getAttribute("contenteditable")?.toLowerCase();
      if (["", "true", "plaintext-only"].includes(value)) return true;
      if (value === "false") return false;
      return this.parentElement?.isContentEditable ?? false;
    }
    replaceChildren(...nodes) {
      this.replacements = (this.replacements || 0) + 1;
      while (this.firstChild) this.firstChild.remove();
      this.append(...nodes);
      this.onReplace?.();
    }
    get shadowRoot() { return this._shadowRoot?.mode === "open" ? this._shadowRoot : null; }
    attachShadow({ mode }) {
      assert.equal(this._shadowRoot, undefined);
      this._shadowRoot = new Node(11);
      this._shadowRoot.host = this;
      this._shadowRoot.mode = mode;
      return this._shadowRoot;
    }
    assign(...nodes) {
      for (const node of this.assigned) node.assignedSlot = null;
      this.assigned = nodes;
      for (const node of nodes) node.assignedSlot = this;
    }
    assignedNodes() { return this.assigned.slice(); }
    getAttribute(name) { return this.attributes[name] ?? null; }
    setAttribute(name, value) { this.attributes[name] = String(value); }
    removeAttribute(name) { delete this.attributes[name]; }
    matches(selector) {
      if (selector === ":disabled") {
        if (this.disabled) return true;
        if (this.tagName === "OPTION") {
          for (let parent = this.parentElement; parent; parent = parent.parentElement) {
            if (parent.tagName === "OPTGROUP" && parent.disabled) return true;
            if (parent.tagName === "SELECT") break;
          }
        }
        return false;
      }
      return ["BUTTON", "TEXTAREA", "SELECT"].includes(this.tagName) ||
        (this.tagName === "INPUT" && this.type !== "hidden") ||
        (this.tagName === "A" && this.getAttribute("href") !== null) ||
        ["button", "link", "textbox", "checkbox", "radio", "combobox"].includes(this.getAttribute("role")) ||
        this.getAttribute("tabindex") !== null;
    }
    getClientRects() { return this.rects ?? (hasBox(this) ? [{}] : []); }
    getBoundingClientRect() {
      return hasBox(this) && this.rects?.length !== 0
        ? { width: 20, height: 10 } : { width: 0, height: 0 };
    }
    click() { this.clicked = (this.clicked || 0) + 1; }
    focus() {
      this.focused = (this.focused || 0) + 1;
      const focusable = ["INPUT", "TEXTAREA", "SELECT", "BUTTON"].includes(this.tagName) ||
        (this.tagName === "A" && this.getAttribute("href") !== null) ||
        this.getAttribute("tabindex") !== null ||
        (this.isContentEditable && !this.parentElement?.isContentEditable);
      if (this.ignoreFocus || !focusable || !this.isConnected || this.matches(":disabled")) return;
      // Model only logical focus retargeting across roots/documents. Real blur,
      // focus events, native key routing and window activation need Chromium.
      let current = this;
      for (let depth = 0; current && depth < 100; depth += 1) {
        const root = current.getRootNode();
        root.activeElement = current;
        current = root.host || root.defaultView?.frameElement;
      }
      this.onFocus?.();
    }
    dispatchEvent(event) {
      this.events.push(event.type);
      this.onEvent?.(event);
      return !event.defaultPrevented;
    }
  }
  class Input extends Element {
    constructor(attributes) {
      super("INPUT", attributes);
      this._checked = Object.hasOwn(attributes, "checked");
      this._indeterminate = false;
      this.checkedWrites = 0;
      this.indeterminateWrites = 0;
    }
    get value() { return this._value; }
    set value(value) { this._value = value; }
    get checked() { return this._checked; }
    set checked(value) { this.checkedWrites += 1; this._checked = !!value; }
    get indeterminate() { return this._indeterminate; }
    set indeterminate(value) { this.indeterminateWrites += 1; this._indeterminate = !!value; }
    click() {
      if (!["checkbox", "radio"].includes(this.type)) return super.click();
      this.clicked = (this.clicked || 0) + 1;
      if (this.matches(":disabled")) return;
      // Small activation double, not browser evidence: model pre-click toggle,
      // canceled activation rollback and resulting input/change only. Native
      // event trust, full radio-group rules and handler exceptions need CEF.
      const type = this.type;
      const checked = this._checked;
      const indeterminate = this._indeterminate;
      const peers = [];
      const name = this.getAttribute("name");
      if (type === "radio" && name) {
        const visit = (parent) => {
          for (const child of parent.children) {
            if (child instanceof Input && child !== this && child.type === "radio" &&
                child.getAttribute("name") === name && child.form === this.form) {
              peers.push([child, child._checked]);
            }
            visit(child);
          }
        };
        visit(this.getRootNode());
      }
      this._checked = type === "checkbox" ? !checked : true;
      if (type === "checkbox") this._indeterminate = false;
      else for (const [peer] of peers) peer._checked = false;
      const accepted = this.dispatchEvent(new Event("click", {
        bubbles: true, cancelable: true, composed: true,
      }));
      if (!accepted) {
        this._checked = checked;
        this._indeterminate = indeterminate;
        for (const [peer, wasChecked] of peers) peer._checked = wasChecked;
      } else if (this.isConnected && (type === "checkbox" || !checked)) {
        // Native activation stops before input/change when a click handler
        // removes the input from its document. The click already happened.
        this.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
        this.dispatchEvent(new Event("change", { bubbles: true }));
      }
    }
  }
  class TextArea extends Element {
    constructor(attributes) { super("TEXTAREA", attributes); }
    get value() { return this._value; }
    set value(value) { this._value = value; }
  }
  class Select extends Element {
    constructor(attributes) {
      super("SELECT", attributes);
      this.multiple = Object.hasOwn(attributes, "multiple");
      this.selectionWrites = 0;
      this.noSelection = false;
    }
    get options() {
      const options = [];
      const visit = (parent) => {
        for (const child of parent.children) {
          if (child.tagName === "OPTION") options.push(child);
          else if (child.tagName === "OPTGROUP") visit(child);
        }
      };
      visit(this);
      options.item = (index) => options[index] ?? null;
      return options;
    }
    get selectedIndex() {
      const options = this.options;
      const selected = options.findIndex((option) => option._selected);
      return selected >= 0 ? selected : this.noSelection || !options.length ? -1 : 0;
    }
    set selectedIndex(index) {
      const options = this.options;
      const selected = Number(index);
      this.selectionWrites += 1;
      this.noSelection = selected < 0 || selected >= options.length;
      options.forEach((option, position) => { option._selected = position === selected; });
    }
    get value() { return this.options[this.selectedIndex]?.value ?? ""; }
    set value(value) { this.selectedIndex = this.options.findIndex((option) => option.value === value); }
  }
  class Option extends Element {
    constructor(attributes) {
      super("OPTION", attributes);
      this._selected = Object.hasOwn(attributes, "selected");
      // Native options may have no independent layout box in a collapsed select.
      this.rects = [];
    }
    get value() { return this.getAttribute("value") ?? this.text; }
    set value(value) { this.setAttribute("value", value); }
    get text() { return this.textContent.replace(/\s+/g, " ").trim(); }
    get label() { return this.getAttribute("label") ?? this.text; }
    set label(value) { this.setAttribute("label", value); }
    get selected() {
      let parent = this.parentElement;
      while (parent && parent.tagName !== "SELECT") parent = parent.parentElement;
      return parent ? parent.options[parent.selectedIndex] === this : this._selected;
    }
  }
  document = new Node(9);
  document.ownerDocument = null;
  document.title = "Fixture";
  document.readyState = "interactive";
  document.body = new Element("BODY");
  document.append(document.body);
  document.activeElement = document.body;
  document.body.innerText = "Fixture page";
  document.createTextNode = (value) => new Text(value);
  document.createRange = () => ({
    setStart(node, offset) { this.node = node; this.start = offset; },
    setEnd(node, offset) {
      assert.equal(node, this.node);
      this.end = offset;
      metrics.largestRange = Math.max(metrics.largestRange, offset - this.start);
    },
    getBoundingClientRect() {
      return hasBox(this.node) && this.node.rects?.length !== 0 && this.end > this.start
        ? { width: this.end - this.start, height: 1 } : { width: 0, height: 0 };
    },
  });
  const window = {
    innerWidth: 1280, innerHeight: 800, scrollX: 0, scrollY: 0,
    getComputedStyle: (element) => element.style,
    scrollBy({ left, top }) { this.scrollX += left; this.scrollY += top; },
  };
  class KeyboardEvent extends Event {
    constructor(type, options) { super(type, options); this.key = options.key; }
  }
  class InputEvent extends Event {
    constructor(type, options) {
      super(type, options);
      this.data = options.data;
      this.inputType = options.inputType;
    }
  }
  document.defaultView = window;
  document.URL = "https://example.test/";
  window.document = document;
  window.location = { href: document.URL };
  Object.assign(window, {
    HTMLInputElement: Input, HTMLTextAreaElement: TextArea,
    HTMLSelectElement: Select, HTMLOptionElement: Option, Element, Event, KeyboardEvent, InputEvent,
  });
  const context = vm.createContext({
    window, document, location: { href: "https://example.test/" }, crypto: webcrypto,
    HTMLInputElement: Input, HTMLTextAreaElement: TextArea,
    HTMLSelectElement: Select, HTMLOptionElement: Option, Event, KeyboardEvent,
  });
  vm.runInContext(source, context);
  return {
    bridge: window.__lightBrowserBridge, document, window, context, metrics,
    add(tagName = "BUTTON", attributes = {}, parent = document.body) {
      const element = tagName === "INPUT" ? new Input(attributes)
        : tagName === "TEXTAREA" ? new TextArea(attributes)
          : tagName === "SELECT" ? new Select(attributes)
            : tagName === "OPTION" ? new Option(attributes) : new Element(tagName, attributes);
      parent.append(element);
      return element;
    },
    text(value, parent = document.body) {
      const text = new Text(value);
      parent.append(text);
      return text;
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

for (const type of ["checkbox", "radio", "button", "submit", "reset", "image", "hidden", "file"]) {
  test(`fill stops before the native write when focus changes an input to ${type}`, () => {
    const f = fixture();
    const input = f.add("INPUT", { type: "text" });
    input.value = "before";
    const prototype = Object.getPrototypeOf(input);
    const descriptor = Object.getOwnPropertyDescriptor(prototype, "value");
    let writes = 0;
    Object.defineProperty(prototype, "value", {
      ...descriptor,
      set(value) { writes += 1; descriptor.set.call(this, value); },
    });
    const ref = f.bridge.read().interactive[0].ref;
    input.onFocus = () => { input.type = type; };

    const result = f.bridge.act({ action: "fill", ref, text: "replacement" });

    assert.equal(writes, 0, "fill must reject the changed type before calling the native setter");
    assert.equal(input.value, "before");
    assert.deepEqual(input.events, []);
    assert.equal(result.error, "ELEMENT_UNAVAILABLE");
  });
}

test("fill preserves supported text, textarea and text/password focus conversions", () => {
  for (const [tagName, before, after] of [
    ["INPUT", "text", "text"], ["TEXTAREA", "text", "text"],
    ["INPUT", "text", "password"], ["INPUT", "password", "text"],
  ]) {
    const f = fixture();
    const element = f.add(tagName, { type: before });
    const ref = f.bridge.read().interactive[0].ref;
    if (tagName === "INPUT") element.onFocus = () => { element.type = after; };

    const result = f.bridge.act({ action: "fill", ref, text: "replacement" });

    assert.equal(result.ok, true, `${tagName} ${before} to ${after}`);
    assert.equal(element.value, "replacement");
    assert.deepEqual(element.events, ["input", "change"]);
  }
});

test("Enter prepares logical focus without dispatching keys or requesting form submission", () => {
  const f = fixture();
  const input = f.add("INPUT", { type: "search" });
  let submissions = 0;
  input.form = { requestSubmit() { submissions += 1; } };
  const ref = f.bridge.read().interactive[0].ref;
  assert.deepEqual(JSON.parse(JSON.stringify(f.bridge.act({ action: "press", ref, key: "Enter" }))),
    { ok: true, action: "press", key: "Enter", prepared: true, attempted: false });
  assert.equal(f.document.activeElement, input);
  assert.equal(submissions, 0);
  assert.deepEqual(input.events, []);
  input.onEvent = (event) => { if (event.type === "keydown") event.preventDefault(); };
  assert.equal(f.bridge.act({ action: "press", ref, key: "Enter" }).prepared, true);
  assert.equal(submissions, 0);
  assert.deepEqual(input.events, []);
  const unsupported = f.bridge.act({ action: "press", ref, key: "Tab" });
  assert.equal(unsupported.error, "UNSUPPORTED_KEY");
  assert.equal(unsupported.attempted, false);
});

test("Enter preparation rejects ignored focus, non-focusable refs and immediate focus theft", () => {
  for (const kind of ["ignored", "not-focusable", "redirected"]) {
    const f = fixture();
    const target = f.add(kind === "not-focusable" ? "DIV" : "INPUT", { role: "textbox", "aria-label": "Target" });
    const other = f.add("TEXTAREA");
    if (kind === "ignored") target.ignoreFocus = true;
    if (kind === "redirected") target.onFocus = () => other.focus();
    const ref = f.bridge.read().interactive.find((item) => item.label === "Target").ref;
    const result = f.bridge.act({ action: "press", ref, key: "Enter" });
    assert.equal(result.error, "FOCUS_NOT_REACHED", kind);
    assert.equal(result.attempted, false, kind);
    assert.deepEqual(target.events, [], kind);
    assert.deepEqual(other.events, [], kind);
    assert.equal(target.isConnected, true, kind);
  }
});

test("Enter preparation supports focusable controls without changing values or activating a window", () => {
  const f = fixture();
  f.document.hasFocus = () => assert.fail("Background documents do not need window focus");
  f.window.focus = () => assert.fail("Preparation must not activate a window");
  for (const [tag, attributes] of [
    ["INPUT", { type: "password" }], ["TEXTAREA", {}], ["BUTTON", {}],
    ["A", { href: "https://example.test/" }], ["DIV", { contenteditable: "true" }],
    ["DIV", { role: "combobox", tabindex: "0" }],
  ]) {
    const target = f.add(tag, { ...attributes, "aria-label": "Target" });
    target.value = "private-value";
    if (tag === "DIV" && attributes.contenteditable) target.textContent = "editor text";
    const ref = f.bridge.read().interactive.find((item) => item.label === "Target").ref;
    const result = f.bridge.act({ action: "press", ref, key: "Enter" });
    assert.equal(result.prepared, true, tag);
    assert.equal(result.attempted, false, tag);
    assert.equal(f.document.activeElement, target, tag);
    assert.equal(target.value, "private-value", tag);
    if (attributes.contenteditable) assert.equal(target.textContent, "editor text");
    assert.deepEqual(target.events, [], tag);
    assert.equal(target.clicked, undefined, tag);
    assert.ok(!JSON.stringify(result).includes("private-value"));
    target.remove();
  }
});

test("Enter preparation verifies every nested shadow and iframe focus boundary", () => {
  const f = fixture();
  const outer = f.add("DIV");
  const outerRoot = outer.attachShadow({ mode: "open" });
  const first = embed(f, {}, outerRoot);
  const inner = first.child.add("DIV");
  const innerRoot = inner.attachShadow({ mode: "open" });
  const second = embed(first.child, {}, innerRoot);
  const host = second.child.add("DIV");
  const root = host.attachShadow({ mode: "open" });
  const target = second.child.add("INPUT", { "aria-label": "Deep target" }, root);
  const ref = f.bridge.read().interactive.find((item) => item.label === "Deep target").ref;
  assert.equal(f.bridge.act({ action: "press", ref, key: "Enter" }).prepared, true);
  for (const [boundary, active] of [
    [root, target], [second.child.document, host], [innerRoot, second.frame],
    [first.child.document, inner], [outerRoot, first.frame], [f.document, outer],
  ]) {
    assert.equal(boundary.activeElement, active);
    target.onFocus = () => { boundary.activeElement = null; };
    const result = f.bridge.act({ action: "press", ref, key: "Enter" });
    assert.equal(result.error, "FOCUS_NOT_REACHED");
    assert.equal(result.attempted, false);
    assert.deepEqual(target.events, []);
    target.onFocus = null;
    target.focus();
  }
});

test("Enter preparation rechecks ref, document and control state after focus", () => {
  for (const mutate of [
    (f, target) => target.remove(),
    (f, target) => { target.hidden = true; },
    (f, target) => { target.disabled = true; },
    (f, target) => { target.parentElement.inert = true; },
    (f, target) => target.parentElement.setAttribute("aria-disabled", "true"),
    (f, target) => { target.ownerDocument = fixture().document; },
    (f) => f.bridge.read(),
  ]) {
    const f = fixture();
    const target = f.add("INPUT");
    const ref = f.bridge.read().interactive[0].ref;
    target.onFocus = () => mutate(f, target);
    const result = f.bridge.act({ action: "press", ref, key: "Enter" });
    assert.equal(result.error, "ELEMENT_UNAVAILABLE");
    assert.equal(result.attempted, false);
    assert.deepEqual(target.events, []);
  }
});

test("Enter preparation revalidates after focus getters and keeps exception contents private", () => {
  for (const failure of ["new-read", "detached", "exception"]) {
    const f = fixture();
    const target = f.add("INPUT");
    const ref = f.bridge.read().interactive[0].ref;
    target.onFocus = () => {
      Object.defineProperty(f.document, "activeElement", {
        configurable: true,
        get() {
          if (failure === "exception") throw new Error("private-keyboard-error");
          if (failure === "new-read") f.bridge.read();
          else target.remove();
          return target;
        },
      });
    };
    const result = f.bridge.act({ action: "press", ref, key: "Enter" });
    assert.equal(result.error, failure === "exception" ? "ACTION_FAILED" : "ELEMENT_UNAVAILABLE");
    assert.equal(result.attempted, false);
    assert.ok(!JSON.stringify(result).includes("private-keyboard-error"));
    assert.deepEqual(target.events, []);
  }
});

test("Enter preparation bounds focus-chain inspection and rejects stale refs before focusing", () => {
  const f = fixture();
  const target = f.add("INPUT");
  const oldRef = f.bridge.read().interactive[0].ref;
  const ref = f.bridge.read().interactive[0].ref;
  for (const stale of [oldRef, "unknown-ref"]) {
    const result = f.bridge.act({ action: "press", ref: stale, key: "Enter" });
    assert.equal(result.error, "STALE_REF");
    assert.equal(result.attempted, false);
    assert.equal(target.focused, undefined);
  }
  let reads = 0;
  const cyclicRoot = target.attachShadow({ mode: "open" });
  target.onFocus = () => {
    target.getRootNode = () => cyclicRoot;
    Object.defineProperty(cyclicRoot, "activeElement", { get() { reads += 1; return target; } });
  };
  const result = f.bridge.act({ action: "press", ref, key: "Enter" });
  assert.equal(result.error, "FOCUS_NOT_REACHED");
  assert.equal(result.attempted, false);
  assert.ok(reads > 0 && reads <= 64, `Focus chain used ${reads} reads`);
  assert.deepEqual(target.events, []);
});

test("snapshot bounds text and interactive elements and excludes hidden content", () => {
  const f = fixture();
  f.document.body.innerText = "x".repeat(12001);
  f.add().hidden = true;
  for (let i = 0; i < 121; i += 1) f.add();
  const snapshot = f.bridge.read();
  assert.equal(snapshot.text.length, 12000);
  assert.equal(snapshot.readyState, "interactive");
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

test("scroll rejects invalid offsets and snapshots report the resulting viewport", () => {
  const f = fixture();
  f.window.scrollX = 80;
  f.window.scrollY = 100;
  assert.equal(f.bridge.act({ action: "scroll", y: Infinity }).error, "INVALID_SCROLL");
  assert.equal(f.bridge.act({ action: "scroll", y: 10001 }).error, "INVALID_SCROLL");
  assert.deepEqual({ ...f.bridge.read().viewport }, { x: 80, y: 100, width: 1280, height: 800 });
  assert.deepEqual(
    { ...f.bridge.act({ action: "scroll", x: -30, y: 400 }) },
    { ok: true, action: "scroll", x: 50, y: 500 },
  );
  assert.equal(f.window.scrollX, 50);
  assert.equal(f.window.scrollY, 500);
  assert.deepEqual({ ...f.bridge.read().viewport }, { x: 50, y: 500, width: 1280, height: 800 });
});

test("open and nested shadow roots use root-local labels and preserve ref action semantics", () => {
  const f = fixture();
  f.add("SPAN", { id: "name" }).innerText = "Wrong document label";
  const host = f.add("DIV");
  const root = host.attachShadow({ mode: "open" });
  f.add("SPAN", { id: "name" }, root).innerText = "Root scoped name";
  const input = f.add("INPUT", { "aria-labelledby": "name" }, root);
  const label = f.add("LABEL", {}, root);
  label.innerText = "Native shadow label";
  const labelled = f.add("INPUT", {}, root);
  labelled.labels = [label];
  const nested = f.add("DIV", {}, root).attachShadow({ mode: "open" });
  const button = f.add("BUTTON", {}, nested);
  button.innerText = "Nested action";
  const password = f.add("INPUT", { type: "password", "aria-label": "Shadow password" }, root);
  password.value = "shadow-secret";
  let submissions = 0;
  input.form = { requestSubmit() { submissions += 1; } };
  const page = f.bridge.read();
  const named = page.interactive.find((item) => item.label === "Root scoped name");
  assert.ok(named);
  assert.ok(page.interactive.find((item) => item.label === "Native shadow label"));
  assert.equal(page.interactive.find((item) => item.label === "Shadow password").value, undefined);
  assert.equal(JSON.stringify(page).includes("shadow-secret"), false);
  assert.equal(f.bridge.act({ action: "fill", ref: named.ref, text: "hello shadow" }).ok, true);
  assert.equal(input.value, "hello shadow");
  assert.deepEqual(input.events, ["input", "change"]);
  assert.equal(f.bridge.act({ action: "press", ref: named.ref, key: "Enter" }).prepared, true);
  assert.equal(submissions, 0);
  assert.equal(root.activeElement, input);
  assert.equal(f.document.activeElement, host);
  assert.deepEqual(input.events, ["input", "change"]);
  const nestedRef = page.interactive.find((item) => item.label === "Nested action").ref;
  assert.equal(f.bridge.act({ action: "click", ref: nestedRef }).ok, true);
  assert.equal(button.clicked, 1);
  f.bridge.read();
  assert.equal(f.bridge.act({ action: "click", ref: nestedRef }).error, "STALE_REF");
});

test("shadow refs recheck host removal, visibility, inert and aria-disabled before acting", () => {
  const changes = [
    ["removed", (host) => host.remove(), "STALE_REF"],
    ["hidden", (host) => { host.hidden = true; }],
    ["inert", (host) => { host.inert = true; }],
    ["aria-hidden", (host) => host.setAttribute("aria-hidden", "true")],
    ["display", (host) => { host.style.display = "none"; }],
    ["visibility", (host) => { host.style.visibility = "hidden"; }],
    ["aria-disabled", (host) => host.setAttribute("aria-disabled", "true")],
  ];
  for (const [name, change, error = "ELEMENT_UNAVAILABLE"] of changes) {
    const f = fixture();
    const host = f.add("DIV");
    const button = f.add("BUTTON", {}, host.attachShadow({ mode: "open" }));
    button.innerText = "Host target";
    const ref = f.bridge.read().interactive[0].ref;
    change(host);
    assert.equal(f.bridge.act({ action: "click", ref }).error, error, name);
    assert.equal(button.clicked, undefined, name);
    const page = f.bridge.read();
    if (name === "aria-disabled") assert.equal(page.interactive[0].disabled, true);
    else {
      assert.equal(page.interactive.length, 0, name);
      assert.equal(page.text.includes("Host target"), false, name);
    }
  }
});

test("assigned slot controls appear once and recheck the slot ancestor", () => {
  const f = fixture();
  const host = f.add("DIV");
  const root = host.attachShadow({ mode: "open" });
  const slot = f.add("SLOT", {}, root);
  f.add("BUTTON", {}, slot).innerText = "Fallback hidden by assignment";
  const assigned = f.add("BUTTON", {}, host);
  assigned.innerText = "Slotted action";
  f.add("BUTTON", {}, host).innerText = "Unassigned light action";
  slot.assign(assigned);
  const page = f.bridge.read();
  assert.equal(page.interactive.length, 1);
  assert.equal(page.interactive[0].label, "Slotted action");
  assert.equal(page.text.includes("Fallback hidden by assignment"), false);
  assert.equal(page.text.includes("Unassigned light action"), false);
  assert.equal(f.bridge.act({ action: "click", ref: page.interactive[0].ref }).ok, true);
  slot.setAttribute("aria-disabled", "true");
  assert.equal(f.bridge.act({ action: "click", ref: page.interactive[0].ref }).error, "ELEMENT_UNAVAILABLE");
  assert.equal(assigned.clicked, 1);
  slot.removeAttribute("aria-disabled");
  slot.inert = true;
  assert.equal(f.bridge.act({ action: "click", ref: page.interactive[0].ref }).error, "ELEMENT_UNAVAILABLE");
  assert.equal(f.bridge.read().interactive.length, 0);
});

test("closed roots, embedded documents, canvas and script-like children are excluded", () => {
  const f = fixture();
  const host = f.add("DIV");
  const closed = host.attachShadow({ mode: "closed" });
  f.add("BUTTON", {}, closed).innerText = "SECRET_CLOSED_ROOT";
  f.add("BUTTON", {}, host).innerText = "SECRET_UNASSIGNED_CLOSED_LIGHT";
  for (const tag of ["IFRAME", "CANVAS", "SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "OBJECT", "EMBED"]) {
    const wrapper = f.add(tag);
    f.add("BUTTON", {}, wrapper).innerText = `SECRET_${tag}`;
  }
  const page = f.bridge.read();
  assert.equal(page.interactive.length, 0);
  assert.equal(page.text.includes("SECRET_"), false);
});

test("shadow output keeps the existing 120-item and 12000-character caps", () => {
  const f = fixture();
  f.document.body.textContent = "";
  const root = f.add("DIV").attachShadow({ mode: "open" });
  f.text("x".repeat(12001), root);
  for (let i = 0; i < 130; i += 1) f.add("BUTTON", { "aria-label": `Action ${i}` }, root);
  const page = f.bridge.read();
  assert.equal(page.text.length, 12000);
  assert.equal(page.interactive.length, 120);
  assert.equal(page.truncated.text, true);
  assert.equal(page.truncated.interactive, true);
  assert.equal(page.truncated.nodes, false);
  assert.equal(page.truncated.shadowRoots, false);
});

test("visited-node budget stops before a late shadow control and keeps earlier explicit labels", () => {
  const f = fixture();
  const root = f.add("DIV").attachShadow({ mode: "open" });
  f.add("BUTTON", { "aria-label": "Before node cap" }, root);
  for (let i = 0; i < 10050; i += 1) f.add("SPAN", {}, root);
  const late = f.add("BUTTON", { "aria-label": "Past node cap" }, root);
  late.getBoundingClientRect = () => { throw new Error("Late node was inspected beyond the cap"); };
  const page = f.bridge.read();
  assert.equal(page.truncated.nodes, true);
  assert.equal(page.interactive.length, 1);
  assert.equal(page.interactive[0].label, "Before node cap");
  assert.equal(f.bridge.act({ action: "click", ref: page.interactive[0].ref }).ok, true);
});

test("label scans share the node budget with the document traversal", () => {
  const f = fixture();
  const root = f.add("DIV").attachShadow({ mode: "open" });
  const label = f.add("DIV", { id: "large-label" }, root);
  for (let i = 0; i < 6000; i += 1) f.add("SPAN", {}, label);
  f.text("Label tail within the document budget", label);
  f.add("BUTTON", { "aria-labelledby": "large-label" }, root);
  const page = f.bridge.read();
  assert.equal(page.text.includes("Label tail within the document budget"), true);
  assert.equal(page.interactive.length, 1);
  // A second, unlimited label walk would incorrectly consume another 6000 nodes.
  assert.equal(page.truncated.nodes, true);
  assert.equal(page.interactive[0].label, "");
});

test("open root count is bounded independently from output item count", () => {
  const f = fixture();
  for (let i = 0; i < 66; i += 1) {
    const root = f.add("DIV").attachShadow({ mode: "open" });
    f.add("BUTTON", { "aria-label": `Root ${i}` }, root);
  }
  const page = f.bridge.read();
  assert.equal(page.interactive.length, 64);
  assert.equal(page.interactive.at(-1).label, "Root 63");
  assert.equal(page.truncated.shadowRoots, true);
  assert.equal(page.truncated.interactive, false);
  assert.equal(page.truncated.nodes, false);
});

test("nested roots stop at the depth bound without exposing the deep sentinel", () => {
  const f = fixture();
  let parent = f.document.body;
  f.add("BUTTON", { "aria-label": "Shallow action" });
  for (let i = 0; i < 70; i += 1) parent = f.add("DIV", {}, parent).attachShadow({ mode: "open" });
  f.add("BUTTON", { "aria-label": "Deep sentinel" }, parent);
  const page = f.bridge.read();
  assert.equal(page.truncated.depth, true);
  assert.equal(page.truncated.shadowRoots, false);
  assert.equal(page.interactive.length, 1);
  assert.equal(page.interactive[0].label, "Shallow action");
});

test("large text nodes are inspected through a bounded substring and bounded layout range", () => {
  const f = fixture();
  f.document.body.textContent = "";
  const root = f.add("DIV").attachShadow({ mode: "open" });
  f.text("x".repeat(200000), root);
  const page = f.bridge.read();
  assert.equal(page.text.length, 12000);
  assert.equal(page.truncated.text, true);
  assert.ok(f.metrics.largestTextSlice <= 12001);
  assert.ok(f.metrics.largestRange <= 12001);
});

test("invisible text ranges and hidden native labels do not leak or suppress a safe fallback", () => {
  const f = fixture();
  const root = f.add("DIV").attachShadow({ mode: "open" });
  const label = f.add("LABEL", {}, root);
  label.hidden = true;
  label.innerText = "Hidden native label";
  const input = f.add("INPUT", { placeholder: "Safe placeholder" }, root);
  input.labels = [label];
  f.text("Text without rendered boxes", root).rects = [];
  const page = f.bridge.read();
  assert.equal(page.interactive[0].label, "Safe placeholder");
  assert.equal(page.text.includes("Hidden native label"), false);
  assert.equal(page.text.includes("Text without rendered boxes"), false);
});

function checkFixture(type = "checkbox", checked = false) {
  const f = fixture();
  const input = f.add("INPUT", { type, "aria-label": "Native check" });
  input.checked = checked;
  input.checkedWrites = 0;
  return { ...f, input, ref: f.bridge.read().interactive[0].ref };
}

test("check toggles a native checkbox once without setters or manual events", () => {
  for (const checked of [false, true]) {
    const f = checkFixture("checkbox", !checked);
    const inputPrototype = Object.getPrototypeOf(f.input);
    for (const property of ["checked", "indeterminate"]) {
      const descriptor = Object.getOwnPropertyDescriptor(inputPrototype, property);
      Object.defineProperty(f.input, property, {
        get() { return descriptor.get.call(this); },
        set() { throw new Error(`Bridge must not assign ${property}`); },
      });
    }
    const observed = [];
    f.input.onEvent = (event) => {
      observed.push({ type: event.type, checked: f.input.checked });
      if (event.type === "click") assert.equal(event.isTrusted, false);
    };
    const result = f.bridge.act({ action: "check", ref: f.ref, checked });
    assert.equal(result.ok, true);
    assert.equal(result.action, "check");
    assert.equal(result.trusted, false);
    assert.equal(result.changed, true);
    assert.equal(result.checked, checked);
    assert.equal(f.input.checked, checked);
    assert.equal(f.input.focused, 1);
    assert.equal(f.input.clicked, 1);
    assert.equal(f.input.checkedWrites, 0);
    assert.equal(f.input.indeterminateWrites, 0);
    assert.deepEqual(observed, ["click", "input", "change"].map((type) => ({ type, checked })));
  }
});

test("check of the current state does not focus, click, set properties or dispatch events", () => {
  for (const [type, checked] of [["checkbox", false], ["checkbox", true], ["radio", true]]) {
    const f = checkFixture(type, checked);
    f.input.onFocus = () => assert.fail("A no-op must not focus the input");
    const result = f.bridge.act({ action: "check", ref: f.ref, checked });
    assert.equal(result.ok, true);
    assert.equal(result.changed, false);
    assert.equal(result.checked, checked);
    assert.equal(result.trusted, false);
    assert.equal(f.input.focused || 0, 0);
    assert.equal(f.input.clicked || 0, 0);
    assert.equal(f.input.checkedWrites, 0);
    assert.deepEqual(f.input.events, []);
  }
});

test("check only accepts boolean checked and supported native controls", () => {
  for (const checked of [undefined, null, 0, 1, "true", "false", {}, [true]]) {
    const f = checkFixture();
    const result = f.bridge.act({ action: "check", ref: f.ref, checked });
    assert.equal(result.error, "INVALID_VALUE");
    assert.equal(f.input.focused || 0, 0);
    assert.equal(f.input.clicked || 0, 0);
    assert.equal(f.input.checkedWrites, 0);
  }
  for (const [tag, attributes] of [
    ["INPUT", { type: "text" }], ["INPUT", { type: "password" }],
    ["INPUT", { type: "file" }], ["BUTTON", {}],
    ["DIV", { role: "checkbox" }], ["DIV", { role: "radio" }],
  ]) {
    const f = fixture();
    const input = f.add(tag, attributes);
    const ref = f.bridge.read().interactive[0].ref;
    assert.equal(f.bridge.act({ action: "check", ref, checked: true }).error, "UNSUPPORTED_ELEMENT");
    assert.equal(input.focused || 0, 0);
    assert.equal(input.clicked || 0, 0);
    assert.deepEqual(input.events, []);
  }
});

test("check refuses radio false and mixed checkboxes even when checked already matches", () => {
  for (const checked of [false, true]) {
    const radio = checkFixture("radio", checked);
    assert.equal(radio.bridge.act({ action: "check", ref: radio.ref, checked: false }).error, "UNSUPPORTED_STATE");
    assert.equal(radio.input.checked, checked);
    assert.equal(radio.input.focused || 0, 0);
    assert.equal(radio.input.clicked || 0, 0);
    for (const desired of [false, true]) {
      const f = checkFixture("checkbox", checked);
      f.input.indeterminate = true;
      f.input.indeterminateWrites = 0;
      assert.equal(f.bridge.act({ action: "check", ref: f.ref, checked: desired }).error, "UNSUPPORTED_STATE");
      assert.equal(f.input.checked, checked);
      assert.equal(f.input.indeterminate, true);
      assert.equal(f.input.indeterminateWrites, 0);
      assert.equal(f.input.checkedWrites, 0);
      assert.equal(f.input.focused || 0, 0);
      assert.equal(f.input.clicked || 0, 0);
    }
  }
});

test("check validates ref, visibility and disabled state before treating a request as a no-op", () => {
  for (const [name, mutate, error] of [
    ["new read", (f) => f.bridge.read(), "STALE_REF"],
    ["removed", (f) => f.input.remove(), "STALE_REF"],
    ["hidden", (f) => { f.input.hidden = true; }, "ELEMENT_UNAVAILABLE"],
    ["disabled", (f) => { f.input.disabled = true; }, "ELEMENT_UNAVAILABLE"],
    ["inert", (f) => { f.input.inert = true; }, "ELEMENT_UNAVAILABLE"],
    ["aria disabled", (f) => f.input.setAttribute("aria-disabled", "true"), "ELEMENT_UNAVAILABLE"],
  ]) {
    const f = checkFixture("checkbox", true);
    mutate(f);
    assert.equal(f.bridge.act({ action: "check", ref: f.ref, checked: true }).error, error, name);
    assert.equal(f.input.focused || 0, 0, name);
    assert.equal(f.input.clicked || 0, 0, name);
    assert.deepEqual(f.input.events, [], name);
  }
});

test("check revalidates reference, type and availability after focus before clicking", () => {
  for (const [name, mutate] of [
    ["new read", (f) => f.bridge.read()],
    ["remove", (f) => f.input.remove()],
    ["replace", (f) => { f.input.remove(); f.replacement = f.add("INPUT", { type: "checkbox" }); }],
    ["retype to text", (f) => { f.input.type = "text"; }],
    ["retype to radio", (f) => { f.input.type = "radio"; }],
    ["hide", (f) => { f.input.hidden = true; }],
    ["disable", (f) => { f.input.disabled = true; }],
  ]) {
    const f = checkFixture();
    f.input.onFocus = () => mutate(f);
    const result = f.bridge.act({ action: "check", ref: f.ref, checked: true });
    assert.equal(result.ok, false, name);
    assert.equal(f.input.focused, 1, name);
    assert.equal(f.input.clicked || 0, 0, name);
    assert.equal(f.input.checkedWrites, 0, name);
    assert.deepEqual(f.input.events, [], name);
    if (name.startsWith("retype")) assert.equal(result.error, "ELEMENT_UNAVAILABLE", name);
    if (f.replacement) assert.equal(f.replacement.clicked || 0, 0);
  }
});

test("check accepts a focus handler reaching the requested state without a click", () => {
  for (const checked of [false, true]) {
    const f = checkFixture("checkbox", !checked);
    f.input.onFocus = () => { f.input.checked = checked; };
    const result = f.bridge.act({ action: "check", ref: f.ref, checked });
    assert.equal(result.ok, true);
    assert.equal(result.changed, false);
    assert.equal(result.checked, checked);
    assert.equal(result.trusted, false);
    assert.equal(f.input.focused, 1);
    assert.equal(f.input.clicked || 0, 0);
    assert.equal(f.input.checkedWrites, 1); // Only the page's focus handler wrote.
    assert.deepEqual(f.input.events, []);
  }
  const mixed = checkFixture();
  mixed.input.onFocus = () => { mixed.input.indeterminate = true; };
  assert.equal(mixed.bridge.act({ action: "check", ref: mixed.ref, checked: true }).error, "UNSUPPORTED_STATE");
  assert.equal(mixed.input.clicked || 0, 0);
  assert.equal(mixed.input.indeterminate, true);
});

test("check reports canceled activation without retrying or manufacturing change events", () => {
  for (const type of ["checkbox", "radio"]) {
    const f = checkFixture(type);
    f.input.onEvent = (event) => { if (event.type === "click") event.preventDefault(); };
    const result = f.bridge.act({ action: "check", ref: f.ref, checked: true });
    assert.equal(result.error, "CHECK_STATE_NOT_REACHED", type);
    assert.equal(result.attempted, true, type);
    assert.equal(result.trusted, false, type);
    assert.equal(result.checked, false, type);
    assert.equal(f.input.checked, false, type);
    assert.equal(f.input.clicked, 1, type);
    assert.equal(f.input.checkedWrites, 0, type);
    assert.deepEqual(f.input.events, ["click"], type);
  }
});

test("check lets native radio activation uncheck only its own named group", () => {
  const f = checkFixture("radio");
  f.input.setAttribute("name", "group");
  const previous = f.add("INPUT", { type: "radio", name: "group", checked: "" });
  const unrelated = f.add("INPUT", { type: "radio", name: "other", checked: "" });
  const result = f.bridge.act({ action: "check", ref: f.ref, checked: true });
  assert.equal(result.ok, true);
  assert.equal(result.changed, true);
  assert.equal(f.input.checked, true);
  assert.equal(previous.checked, false);
  assert.equal(unrelated.checked, true);
  assert.deepEqual(previous.events, []);
  assert.deepEqual(unrelated.events, []);
  assert.equal(f.input.clicked, 1);
  assert.equal(previous.clicked || 0, 0);
  assert.equal(previous.checkedWrites, 0);
});

test("check reports the state left by page handlers without a second click", () => {
  for (const eventType of ["click", "input", "change"]) {
    const f = checkFixture();
    f.input.onEvent = (event) => { if (event.type === eventType) f.input.checked = false; };
    const result = f.bridge.act({ action: "check", ref: f.ref, checked: true });
    assert.equal(result.error, "CHECK_STATE_NOT_REACHED", eventType);
    assert.equal(result.attempted, true);
    assert.equal(result.trusted, false);
    assert.equal(result.checked, false);
    assert.equal(result.indeterminate, false);
    assert.equal(f.input.clicked, 1);
    assert.equal(f.input.checkedWrites, 1); // Only the page handler wrote.
    assert.deepEqual(f.input.events, ["click", "input", "change"]);
  }
  const mixed = checkFixture();
  mixed.input.onEvent = (event) => { if (event.type === "change") mixed.input.indeterminate = true; };
  const result = mixed.bridge.act({ action: "check", ref: mixed.ref, checked: true });
  assert.equal(result.error, "CHECK_STATE_NOT_REACHED");
  assert.equal(result.checked, true);
  assert.equal(result.indeterminate, true);
  assert.equal(result.attempted, true);
  assert.equal(mixed.input.clicked, 1);
  assert.equal(mixed.input.indeterminateWrites, 1);
});

test("check cannot confirm a clicked target replaced, retyped or made unavailable by a handler", () => {
  for (const [name, mutate] of [
    ["new read", (f) => f.bridge.read()],
    ["remove", (f) => f.input.remove()],
    ["replace", (f) => { f.input.remove(); f.replacement = f.add("INPUT", { type: "checkbox" }); }],
    ["retype", (f) => { f.input.type = "radio"; }],
    ["hide", (f) => { f.input.hidden = true; }],
    ["disable", (f) => { f.input.disabled = true; }],
  ]) {
    const f = checkFixture();
    f.input.onEvent = (event) => { if (event.type === "click") mutate(f); };
    const result = f.bridge.act({ action: "check", ref: f.ref, checked: true });
    assert.equal(result.error, "CHECK_STATE_UNCONFIRMED", name);
    assert.equal(result.attempted, true, name);
    assert.equal(result.trusted, false, name);
    assert.equal(Object.hasOwn(result, "checked"), false, name);
    assert.equal(Object.hasOwn(result, "indeterminate"), false, name);
    assert.equal(f.input.clicked, 1, name);
    assert.equal(f.input.checkedWrites, 0, name);
    if (name === "remove" || name === "replace") {
      assert.deepEqual(f.input.events, ["click"], name);
    }
    if (f.replacement) assert.equal(f.replacement.clicked || 0, 0);
  }
});

test("check marks a throwing click as attempted without disclosing exception data or retrying", () => {
  const f = checkFixture();
  f.input.click = () => { f.input.clicked = (f.input.clicked || 0) + 1; throw new Error("private-handler-data"); };
  const result = f.bridge.act({ action: "check", ref: f.ref, checked: true });
  assert.equal(result.error, "ACTION_FAILED");
  assert.equal(result.attempted, true);
  assert.equal(result.trusted, false);
  assert.equal(f.input.clicked, 1);
  assert.equal(f.input.checked, false);
  assert.equal(JSON.stringify(result).includes("private-handler-data"), false);
});

test("read exposes indeterminate only for native checkboxes without changing the state", () => {
  const f = fixture();
  const mixed = f.add("INPUT", { type: "checkbox", "aria-label": "Mixed checkbox" });
  mixed.checked = true;
  mixed.indeterminate = true;
  f.add("INPUT", { type: "checkbox", "aria-label": "Normal checkbox" });
  f.add("INPUT", { type: "radio", "aria-label": "Radio" });
  f.add("DIV", { role: "checkbox", "aria-checked": "mixed", "aria-label": "Custom checkbox" });
  const page = f.bridge.read();
  const item = (label) => page.interactive.find((entry) => entry.label === label);
  assert.equal(item("Mixed checkbox").checked, true);
  assert.equal(item("Mixed checkbox").indeterminate, true);
  assert.equal(item("Normal checkbox").indeterminate, false);
  assert.equal(Object.hasOwn(item("Radio"), "indeterminate"), false);
  assert.equal(Object.hasOwn(item("Custom checkbox"), "indeterminate"), false);
  assert.equal(mixed.focused || 0, 0);
  assert.equal(mixed.clicked || 0, 0);
});

test("check uses open-shadow and same-origin iframe refs and rejects a replaced frame document", () => {
  const f = fixture();
  const host = f.add("DIV");
  const shadow = host.attachShadow({ mode: "open" });
  const checkbox = f.add("INPUT", { type: "checkbox", "aria-label": "Shadow checkbox" }, shadow);
  const { frame, child } = embed(f);
  const radio = child.add("INPUT", { type: "radio", "aria-label": "Child radio" });
  const page = f.bridge.read();
  const item = (label) => page.interactive.find((entry) => entry.label === label);
  assert.equal(f.bridge.act({ action: "check", ref: item("Shadow checkbox").ref, checked: true }).ok, true);
  assert.equal(f.bridge.act({ action: "check", ref: item("Child radio").ref, checked: true }).ok, true);
  assert.equal(checkbox.clicked, 1);
  assert.equal(radio.clicked, 1);
  frame.contentDocument = fixture().document;
  assert.equal(f.bridge.act({ action: "check", ref: item("Child radio").ref, checked: true }).error, "STALE_REF");
  assert.equal(radio.clicked, 1);
});

test("check cannot confirm a child input after click replaces its iframe document", () => {
  const f = fixture();
  const { frame, child } = embed(f);
  const input = child.add("INPUT", { type: "checkbox" });
  const ref = f.bridge.read().interactive[0].ref;
  input.onEvent = (event) => { if (event.type === "click") frame.contentDocument = fixture().document; };
  const result = f.bridge.act({ action: "check", ref, checked: true });
  assert.equal(result.error, "CHECK_STATE_UNCONFIRMED");
  assert.equal(result.attempted, true);
  assert.equal(result.trusted, false);
  assert.equal(Object.hasOwn(result, "checked"), false);
  assert.equal(Object.hasOwn(result, "indeterminate"), false);
  assert.equal(input.clicked, 1);
});

function selectFixture(values = ["initial", "target"]) {
  const f = fixture();
  const select = f.add("SELECT", { "aria-label": "Choose a value" });
  const options = values.map((value, index) => {
    const option = f.add("OPTION", { value }, select);
    option.textContent = `Option ${index}`;
    return option;
  });
  select.selectedIndex = 0;
  return { ...f, select, options, ref: f.bridge.read().interactive[0].ref };
}

test("select uses the native selectedIndex setter and ordered untrusted events", () => {
  const f = selectFixture(["initial", "  exact\tvalue  "]);
  const selectedIndex = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(f.select), "selectedIndex");
  Object.defineProperty(f.select, "selectedIndex", {
    get() { return selectedIndex.get.call(this); },
    set() { throw new Error("Framework setter must not intercept the native write"); },
  });
  const events = [];
  f.select.onEvent = (event) => {
    events.push({ type: event.type, bubbles: event.bubbles, composed: event.composed, trusted: event.isTrusted });
    assert.equal(f.select.value, "  exact\tvalue  ");
  };
  const writes = f.select.selectionWrites;
  const result = f.bridge.act({ action: "select", ref: f.ref, value: "  exact\tvalue  " });
  assert.equal(result.ok, true);
  assert.equal(result.changed, true);
  assert.equal(result.trusted, false);
  assert.equal(Object.hasOwn(result, "value"), false);
  assert.equal(f.select.selectedIndex, 1);
  assert.equal(f.select.selectionWrites, writes + 1);
  assert.deepEqual(events, [
    { type: "input", bubbles: true, composed: true, trusted: false },
    { type: "change", bubbles: true, composed: false, trusted: false },
  ]);
});

test("select of the current option is a no-op without a setter or events", () => {
  const f = selectFixture();
  const writes = f.select.selectionWrites;
  const result = f.bridge.act({ action: "select", ref: f.ref, value: "initial" });
  assert.equal(result.ok, true);
  assert.equal(result.changed, false);
  assert.equal(result.trusted, false);
  assert.equal(f.select.selectionWrites, writes);
  assert.deepEqual(f.select.events, []);
});

test("select reports dispatch without claiming that page handlers preserved the value", () => {
  const f = selectFixture(["initial", "private-result-value"]);
  f.select.onEvent = (event) => {
    if (event.type === "input") f.select.selectedIndex = 0;
  };
  const result = f.bridge.act({ action: "select", ref: f.ref, value: "private-result-value" });
  assert.equal(result.ok, true);
  assert.equal(result.changed, true);
  assert.equal(result.trusted, false);
  assert.equal(f.select.value, "initial");
  assert.deepEqual(f.select.events, ["input", "change"]);
  assert.equal(JSON.stringify(result).includes("private-result-value"), false);
  assert.equal(Object.hasOwn(result, "value"), false);
  f.select.onEvent = () => { throw new Error("private-result-value"); };
  const failure = f.bridge.act({ action: "select", ref: f.ref, value: "private-result-value" });
  assert.equal(failure.error, "ACTION_FAILED");
  assert.equal(JSON.stringify(failure).includes("private-result-value"), false);
});

test("select matches exact empty and whitespace values within the UTF-16 bound", () => {
  for (const value of ["", " ", "  exact\tvalue  ", "x".repeat(1024), "😀".repeat(512)]) {
    const f = selectFixture(["initial", value]);
    assert.equal(f.bridge.act({ action: "select", ref: f.ref, value }).ok, true, `length ${value.length}`);
    assert.equal(f.select.value, value);
    assert.equal(f.select.selectedIndex, 1);
  }
  const f = selectFixture(["initial", " target "]);
  assert.equal(f.bridge.act({ action: "select", ref: f.ref, value: "target" }).error, "OPTION_NOT_FOUND");
  assert.equal(f.select.selectedIndex, 0);
});

test("select rejects invalid values before changing selection", () => {
  for (const value of [undefined, null, 7, {}, ["target"], "x".repeat(1025), "😀".repeat(513)]) {
    const f = selectFixture();
    const writes = f.select.selectionWrites;
    assert.equal(f.bridge.act({ action: "select", ref: f.ref, value }).error, "INVALID_VALUE");
    assert.equal(f.select.selectionWrites, writes);
    assert.deepEqual(f.select.events, []);
  }
});

test("select accepts the 500th option but refuses collections larger than 500", () => {
  const f = selectFixture(Array.from({ length: 500 }, (_, i) => `value-${i}`));
  assert.equal(f.bridge.act({ action: "select", ref: f.ref, value: "value-499" }).ok, true);
  assert.equal(f.select.selectedIndex, 499);
  f.add("OPTION", { value: "overflow" }, f.select);
  f.select.events.length = 0;
  const writes = f.select.selectionWrites;
  assert.equal(f.bridge.act({ action: "select", ref: f.ref, value: "value-0" }).error, "OPTION_LIMIT");
  assert.equal(f.select.selectionWrites, writes);
  assert.deepEqual(f.select.events, []);
});

test("select refuses missing and duplicate raw values even when one duplicate is hidden", () => {
  const f = selectFixture(["initial", "duplicate", "duplicate"]);
  f.options[2].hidden = true;
  assert.equal(f.bridge.act({ action: "select", ref: f.ref, value: "missing" }).error, "OPTION_NOT_FOUND");
  assert.equal(f.bridge.act({ action: "select", ref: f.ref, value: "duplicate" }).error, "AMBIGUOUS_OPTION");
  assert.equal(f.select.selectedIndex, 0);
  assert.deepEqual(f.select.events, []);
});

test("select rejects multiple and custom controls and does not reuse stale refs", () => {
  const multiple = selectFixture();
  multiple.select.multiple = true;
  assert.equal(multiple.bridge.act({ action: "select", ref: multiple.ref, value: "target" }).error, "UNSUPPORTED_ELEMENT");
  const custom = fixture();
  const combobox = custom.add("DIV", { role: "combobox" });
  const customRef = custom.bridge.read().interactive[0].ref;
  assert.equal(custom.bridge.act({ action: "select", ref: customRef, value: "target" }).error, "UNSUPPORTED_ELEMENT");
  assert.deepEqual(combobox.events, []);
  const stale = selectFixture();
  stale.bridge.read();
  assert.equal(stale.bridge.act({ action: "select", ref: stale.ref, value: "target" }).error, "STALE_REF");
  const removed = selectFixture();
  removed.select.remove();
  assert.equal(removed.bridge.act({ action: "select", ref: removed.ref, value: "target" }).error, "STALE_REF");
});

test("select rechecks its own availability before acting", () => {
  for (const [name, change] of [
    ["hidden", (select) => { select.hidden = true; }],
    ["disabled", (select) => { select.disabled = true; }],
    ["inert", (select) => { select.inert = true; }],
    ["aria-hidden", (select) => select.setAttribute("aria-hidden", "true")],
    ["display", (select) => { select.style.display = "none"; }],
  ]) {
    const f = selectFixture();
    change(f.select);
    assert.equal(f.bridge.act({ action: "select", ref: f.ref, value: "target" }).error, "ELEMENT_UNAVAILABLE", name);
    assert.equal(f.select.selectedIndex, 0, name);
    assert.deepEqual(f.select.events, [], name);
  }
});

test("select checks option and optgroup state without requiring option layout boxes", () => {
  const changes = [
    ["disabled", (node) => { node.disabled = true; }],
    ["hidden", (node) => { node.hidden = true; }],
    ["inert", (node) => { node.inert = true; }],
    ["aria-hidden", (node) => node.setAttribute("aria-hidden", "true")],
    ["aria-disabled", (node) => node.setAttribute("aria-disabled", "true")],
    ["display", (node) => { node.style.display = "none"; }],
    ["visibility", (node) => { node.style.visibility = "hidden"; }],
    ["collapsed", (node) => { node.style.visibility = "collapse"; }],
  ];
  for (const grouped of [false, true]) {
    for (const [name, change] of changes) {
      const f = selectFixture();
      const target = grouped ? f.add("OPTGROUP", {}, f.select) : f.options[1];
      if (grouped) target.append(f.options[1]);
      change(target);
      assert.equal(f.bridge.act({ action: "select", ref: f.ref, value: "target" }).error, "OPTION_UNAVAILABLE", `${grouped ? "group" : "option"}: ${name}`);
      assert.equal(f.select.selectedIndex, 0);
      assert.deepEqual(f.select.events, []);
    }
  }
  const f = selectFixture();
  assert.deepEqual(f.options[1].getBoundingClientRect(), { width: 0, height: 0 });
  assert.equal(f.bridge.act({ action: "select", ref: f.ref, value: "target" }).ok, true);
});

test("select stops when focus detaches, replaces, hides or disables the select", () => {
  const changes = [
    ["detach", (f) => f.select.remove()],
    ["replace", (f) => {
      f.select.remove();
      const replacement = f.add("SELECT", { "aria-label": "Choose a value" });
      f.add("OPTION", { value: "target" }, replacement);
      replacement.selectedIndex = -1;
      f.replacement = replacement;
    }],
    ["hidden", (f) => { f.select.hidden = true; }],
    ["disabled", (f) => { f.select.disabled = true; }],
    ["inert", (f) => { f.select.inert = true; }],
    ["multiple", (f) => { f.select.multiple = true; }],
  ];
  for (const [name, change] of changes) {
    const f = selectFixture();
    const writes = f.select.selectionWrites;
    f.select.onFocus = () => change(f);
    const result = f.bridge.act({ action: "select", ref: f.ref, value: "target" });
    assert.equal(result.ok, false, name);
    assert.equal(f.select.selectionWrites, writes, name);
    assert.deepEqual(f.select.events, [], name);
    if (f.replacement) {
      assert.equal(f.replacement.selectedIndex, -1);
      assert.deepEqual(f.replacement.events, []);
    }
  }
});

test("select stops when focus changes the target option or makes its value ambiguous", () => {
  const changes = [
    ["remove", (f) => f.options[1].remove()],
    ["replace", (f) => {
      f.options[1].remove();
      f.add("OPTION", { value: "target" }, f.select);
    }],
    ["move to another select", (f) => f.add("SELECT").append(f.options[1])],
    ["value", (f) => { f.options[1].value = "changed"; }],
    ["hidden", (f) => { f.options[1].hidden = true; }],
    ["disabled", (f) => { f.options[1].disabled = true; }],
    ["duplicate", (f) => f.add("OPTION", { value: "target" }, f.select)],
    ["option limit", (f) => {
      for (let i = 0; i < 499; i += 1) f.add("OPTION", { value: `added-${i}` }, f.select);
    }],
  ];
  for (const [name, change] of changes) {
    const f = selectFixture();
    const writes = f.select.selectionWrites;
    f.select.onFocus = () => change(f);
    assert.equal(f.bridge.act({ action: "select", ref: f.ref, value: "target" }).ok, false, name);
    assert.equal(f.select.selectionWrites, writes, name);
    assert.deepEqual(f.select.events, [], name);
  }
});

test("select allows a still-valid target to move or unrelated options to change during focus", () => {
  for (const [name, change] of [
    ["target reorders", (f) => f.select.append(f.options[0])],
    ["unrelated value changes", (f) => { f.options[2].value = "different"; }],
    ["unrelated option is added", (f) => f.add("OPTION", { value: "new" }, f.select)],
  ]) {
    const f = selectFixture(["initial", "target", "unrelated"]);
    const target = f.options[1];
    f.select.onFocus = () => change(f);
    const result = f.bridge.act({ action: "select", ref: f.ref, value: "target" });
    assert.equal(result.ok, true, name);
    assert.equal(result.changed, true, name);
    assert.equal(f.select.options[f.select.selectedIndex], target, name);
    assert.deepEqual(f.select.events, ["input", "change"], name);
  }
});

test("select snapshots preserve exact values and report option state without layout boxes", () => {
  const f = selectFixture(["initial", "  value\tkept  ", "", "disabled", "hidden"]);
  f.options[1].label = "  A   label\nwith\tspacing  ";
  f.select.selectedIndex = 1;
  const group = f.add("OPTGROUP", {}, f.select);
  group.disabled = true;
  group.append(f.options[3]);
  f.options[4].hidden = true;
  const snapshot = f.bridge.read().interactive[0];
  assert.equal(snapshot.multiple, false);
  assert.equal(snapshot.optionsTruncated, false);
  assert.equal(snapshot.options.length, 5);
  const exact = snapshot.options.find((option) => option.value === "  value\tkept  ");
  assert.ok(exact);
  assert.equal(exact.label, "A label with spacing");
  assert.equal(exact.selected, true);
  assert.equal(exact.disabled, false);
  assert.equal(exact.hidden, false);
  assert.ok(snapshot.options.some((option) => option.value === ""));
  assert.equal(snapshot.options.find((option) => option.value === "disabled").disabled, true);
  const hidden = snapshot.options.find((option) => option.hidden);
  assert.ok(hidden);
  assert.deepEqual(Object.keys(hidden).sort(), ["disabled", "hidden", "selected"]);
  f.select.multiple = true;
  assert.equal(f.bridge.read().interactive[0].multiple, true);
});

test("select snapshot labels and values have separate bounds without truncating a raw value", () => {
  const f = selectFixture(["initial", "x".repeat(1024), "x".repeat(1025), "😀".repeat(513)]);
  f.options[1].label = "z".repeat(500);
  const options = f.bridge.read().interactive[0].options;
  assert.equal(options[1].value.length, 1024);
  assert.equal(options[1].label.length, 240);
  for (const option of options.slice(2)) {
    assert.equal(Object.hasOwn(option, "value"), false);
    assert.equal(option.valueTooLong, true);
  }
});

test("hidden options do not disclose or read their value and label", () => {
  for (const grouped of [false, true]) {
    const f = selectFixture();
    const node = grouped ? f.add("OPTGROUP", {}, f.select) : f.options[1];
    if (grouped) node.append(f.options[1]);
    node.hidden = true;
    for (const property of ["value", "label"]) {
      Object.defineProperty(f.options[1], property, {
        get() { throw new Error(`Read hidden option ${property}`); },
      });
    }
    const snapshot = f.bridge.read().interactive[0];
    assert.deepEqual({ ...snapshot.options[1] }, { hidden: true, disabled: false, selected: false });
  }
});

test("select snapshots cap options per element and per read without reading late values", () => {
  const f = fixture();
  for (let i = 0; i < 7; i += 1) {
    const select = f.add("SELECT", { "aria-label": `Select ${i}` });
    for (let j = 0; j < 41; j += 1) {
      const option = f.add("OPTION", { value: `${i}-${j}` }, select);
      if (j === 40 || i === 6) {
        Object.defineProperty(option, "value", {
          get() { throw new Error("Read an option value beyond the snapshot budget"); },
        });
      }
    }
    // Avoid asking for a selected option value outside the global snapshot budget.
    if (i === 6) select.selectedIndex = -1;
  }
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const page = f.bridge.read();
    assert.equal(page.interactive.length, 7);
    assert.deepEqual(Array.from(page.interactive, (item) => item.options.length), [40, 40, 40, 40, 40, 40, 0]);
    assert.equal(page.interactive.reduce((total, item) => total + item.options.length, 0), 240);
    assert.equal(page.interactive.every((item) => item.optionsTruncated), true);
  }
});


// Independent document/Window doubles model embedding identity, not browser SOP,
// isolated-world wrappers or real iframe rendering. Those need a CEF run.
function embed(parent, attributes = {}, container = parent.document.body) {
  const frame = parent.add("IFRAME", attributes, container);
  const child = fixture();
  child.document.body.textContent = "";
  frame.contentDocument = child.document;
  frame.contentWindow = child.window;
  child.window.frameElement = frame;
  return { frame, child };
}

test("same-origin frames expose bounded context and flat refs with nested shadow labels", () => {
  const f = fixture();
  const { child } = embed(f, { title: "Embedded form" });
  child.document.title = "Child title";
  child.document.URL = "https://example.test/child";
  child.text("Child paragraph");
  const input = child.add("INPUT", { "aria-label": "Child input" });
  const secret = child.add("INPUT", { type: "password", "aria-label": "Child password" });
  secret.value = "must-not-disclose";
  const select = child.add("SELECT", { "aria-label": "Child choice" });
  child.add("OPTION", { value: "a" }, select).textContent = "Alpha";
  child.add("OPTION", { value: "b" }, select).textContent = "Beta";
  const nested = embed(child, { title: "Nested frame" });
  const host = nested.child.add("DIV");
  const shadow = host.attachShadow({ mode: "open" });
  const label = nested.child.add("SPAN", { id: "label" }, shadow);
  label.textContent = "Nested shadow action";
  const button = nested.child.add("BUTTON", { "aria-labelledby": "label" }, shadow);
  child.window.Event = class extends child.window.Event {
    constructor(...args) { super(...args); this.realm = "child"; }
  };
  child.window.KeyboardEvent = class extends child.window.KeyboardEvent {
    constructor(...args) { super(...args); this.realm = "child"; }
  };
  input.onEvent = (event) => assert.equal(event.realm, "child");
  const inputPrototype = child.window.HTMLInputElement.prototype;
  const nativeValue = Object.getOwnPropertyDescriptor(inputPrototype, "value");
  let childWrites = 0;
  Object.defineProperty(inputPrototype, "value", {
    ...nativeValue, set(value) { childWrites += 1; nativeValue.set.call(this, value); },
  });
  const page = f.bridge.read();
  assert.equal(page.frames.length, 2);
  assert.equal(page.frames[0].status, "accessible");
  assert.equal(page.frames[0].parent, "main");
  assert.equal(page.frames[0].title, "Child title");
  assert.equal(page.frames[0].url, "https://example.test/child");
  assert.equal(page.frames[1].parent, page.frames[0].id);
  assert.match(page.text, /Child paragraph/);
  assert.match(page.text, /Nested shadow action/);
  assert.ok(!JSON.stringify(page).includes("must-not-disclose"));
  const item = (label) => page.interactive.find((entry) => entry.label === label);
  assert.equal(item("Child input").frame, page.frames[0].id);
  assert.equal(item("Nested shadow action").frame, page.frames[1].id);
  assert.equal(f.bridge.act({ action: "fill", ref: item("Child input").ref, text: "changed" }).ok, true);
  assert.equal(input.value, "changed");
  assert.equal(childWrites, 1);
  assert.equal(f.bridge.act({ action: "select", ref: item("Child choice").ref, value: "b" }).ok, true);
  assert.equal(select.value, "b");
  assert.equal(f.bridge.act({ action: "click", ref: item("Nested shadow action").ref }).ok, true);
  assert.equal(button.clicked, 1);
  let submitted = 0;
  input.form = { requestSubmit() { submitted += 1; } };
  assert.equal(f.bridge.act({ action: "press", ref: item("Child input").ref, key: "Enter" }).prepared, true);
  assert.equal(submitted, 0);
  assert.equal(child.document.activeElement, input);
  assert.equal(f.document.activeElement, child.window.frameElement);
  assert.deepEqual(input.events, ["input", "change"]);
  assert.equal(page.viewport.width, f.window.innerWidth);
  f.bridge.act({ action: "scroll", y: 100 });
  assert.equal(f.window.scrollY, 100);
  assert.equal(child.window.scrollY, 0);
});

test("frame labels remain document-local despite duplicate ids", () => {
  const f = fixture();
  f.add("SPAN", { id: "same" }).textContent = "Parent label";
  f.add("BUTTON", { "aria-labelledby": "same" });
  const { child } = embed(f);
  child.add("SPAN", { id: "same" }).textContent = "Child label";
  child.add("BUTTON", { "aria-labelledby": "same" });
  const labels = f.bridge.read().interactive.map((entry) => entry.label);
  assert.ok(labels.includes("Parent label"));
  assert.ok(labels.includes("Child label"));
});

test("frame refs reject detached, replaced and inaccessible embedding documents", () => {
  const mutations = [
    (frame) => frame.remove(),
    (frame) => { frame.contentDocument = fixture().document; },
    (frame) => { frame.contentDocument = null; },
    (frame) => Object.defineProperty(frame, "contentDocument", { get() { throw new Error("private page exception"); } }),
  ];
  for (const mutate of mutations) {
    const f = fixture();
    const { frame, child } = embed(f);
    const button = child.add("BUTTON", { "aria-label": "Frame button" });
    const ref = f.bridge.read().interactive.find((entry) => entry.label === "Frame button").ref;
    mutate(frame);
    const result = f.bridge.act({ action: "click", ref });
    assert.equal(result.error, "STALE_REF");
    assert.equal(button.clicked, undefined);
    assert.ok(!JSON.stringify(result).includes("private page exception"));
  }
});

test("frame availability includes embedding ancestors, zero size and disabled state", () => {
  const mutations = [
    (frame) => { frame.hidden = true; },
    (frame) => { frame.inert = true; },
    (frame) => frame.setAttribute("aria-hidden", "true"),
    (frame) => frame.setAttribute("aria-disabled", "true"),
    (frame) => { frame.style.display = "none"; },
    (frame) => { frame.rects = []; },
  ];
  for (const mutate of mutations) {
    const f = fixture();
    const { frame, child } = embed(f);
    const nested = embed(child);
    const input = nested.child.add("INPUT", { "aria-label": "Nested editable" });
    const ref = f.bridge.read().interactive.find((entry) => entry.label === "Nested editable").ref;
    mutate(frame);
    assert.equal(f.bridge.act({ action: "fill", ref, text: "must-not-write" }).error, "ELEMENT_UNAVAILABLE");
    assert.equal(input.value, "");
    assert.deepEqual(input.events, []);
    const next = f.bridge.read().interactive;
    if (frame.getAttribute("aria-disabled") === "true") {
      assert.equal(next.find((entry) => entry.label === "Nested editable").disabled, true);
    } else {
      assert.ok(!next.some((entry) => entry.label === "Nested editable"));
    }
  }
});

test("focus handlers cannot redirect frame refs to another document", () => {
  for (const action of ["fill", "select", "press"]) {
    const f = fixture();
    const { frame, child } = embed(f);
    const element = child.add(action === "select" ? "SELECT" : "INPUT", { "aria-label": "Target" });
    if (action === "select") {
      child.add("OPTION", { value: "a" }, element);
      child.add("OPTION", { value: "b" }, element);
    }
    const ref = f.bridge.read().interactive.find((entry) => entry.label === "Target").ref;
    element.onFocus = () => { frame.contentDocument = fixture().document; };
    const result = f.bridge.act({ action, ref, text: "must-not-write", value: "b", key: "Enter" });
    assert.equal(result.error, "ELEMENT_UNAVAILABLE");
    if (action === "press") assert.equal(result.attempted, false);
    assert.deepEqual(element.events, []);
    assert.equal(element.value, action === "select" ? "a" : "");
  }
});

test("unavailable frames are explicit and metadata access stops at sixteen slots", () => {
  const f = fixture();
  let lateReads = 0;
  for (let index = 0; index < 18; index += 1) {
    const frame = f.add("IFRAME", { title: `Frame ${index}` });
    Object.defineProperty(frame, "contentDocument", { get() {
      if (index >= 16) lateReads += 1;
      if (index % 2) throw new Error("IFRAME_ACCESS_EXCEPTION_SENTINEL");
      return null;
    } });
  }
  const page = f.bridge.read();
  assert.equal(page.frames.length, 16);
  assert.ok(page.frames.every((frame) => frame.status === "unavailable"));
  assert.ok(page.frames.every((frame) => frame.url === undefined && frame.title === undefined));
  assert.equal(page.truncated.frames, true);
  assert.equal(lateReads, 0);
  assert.ok(!JSON.stringify(page).includes("IFRAME_ACCESS_EXCEPTION_SENTINEL"));
});

test("nested frame depth and global text, input and option budgets stay bounded", () => {
  const f = fixture();
  let parent = f;
  for (let index = 0; index < 10; index += 1) parent = embed(parent).child;
  parent.add("BUTTON", { "aria-label": "Too deep" });
  let page = f.bridge.read();
  assert.equal(page.frames.filter((entry) => entry.status === "accessible").length, 8);
  assert.equal(page.frames.at(-1).status, "limit");
  assert.equal(page.truncated.frameDepth, true);
  assert.ok(!page.interactive.some((entry) => entry.label === "Too deep"));
  const bounded = fixture();
  for (let index = 0; index < 4; index += 1) {
    const { child } = embed(bounded);
    child.text("x".repeat(5000));
    for (let button = 0; button < 40; button += 1) child.add("BUTTON");
  }
  page = bounded.bridge.read();
  assert.ok(page.text.length <= 12000);
  assert.equal(page.interactive.length, 120);
  assert.equal(page.truncated.text, true);
  assert.equal(page.truncated.interactive, true);
  const options = fixture();
  for (let index = 0; index < 8; index += 1) {
    const { child } = embed(options);
    const select = child.add("SELECT");
    for (let option = 0; option < 50; option += 1) child.add("OPTION", { value: String(option) }, select);
  }
  page = options.bridge.read();
  assert.equal(page.interactive.reduce((total, item) => total + (item.options?.length || 0), 0), 240);
});

test("frame references keep original documents and expire on a new read", () => {
  const f = fixture();
  const a = embed(f);
  const b = embed(f);
  const input = a.child.add("INPUT", { "aria-label": "Moved field" });
  const old = f.bridge.read().interactive[0].ref;
  const current = f.bridge.read().interactive[0].ref;
  assert.equal(f.bridge.act({ action: "fill", ref: old, text: "old" }).error, "STALE_REF");
  // Model adoptNode: both destination frames remain connected and accessible.
  b.child.document.body.append(input);
  input.ownerDocument = b.child.document;
  assert.equal(f.bridge.act({ action: "fill", ref: current, text: "must-not-follow" }).error, "STALE_REF");
  assert.equal(input.value, "");
});


test("frame changes after input report partial dispatch and stop the old change event", () => {
  for (const action of ["fill", "select"]) {
    const f = fixture();
    const { frame, child } = embed(f);
    const element = child.add(action === "fill" ? "INPUT" : "SELECT");
    if (action === "select") {
      child.add("OPTION", { value: "a" }, element);
      child.add("OPTION", { value: "b" }, element);
    }
    element.onEvent = (event) => { if (event.type === "input") frame.contentDocument = null; };
    const ref = f.bridge.read().interactive[0].ref;
    const result = f.bridge.act({ action, ref, text: "written", value: "b" });
    assert.equal(result.ok, action !== "fill");
    assert.equal(result.trusted, false);
    if (action === "fill") {
      assert.equal(result.error, "FILL_STATE_UNCONFIRMED");
      assert.equal(result.attempted, true);
    } else assert.match(result.detail, /input.*dispatched/i);
    assert.deepEqual(element.events, ["input"]);
    assert.equal(element.value, action === "fill" ? "written" : "b");
  }
});

function editableRef(f, label = "Editor") {
  return f.bridge.read().interactive.find((item) => item.label === label)?.ref;
}

test("contenteditable read exposes explicit editing hosts, not inherited or nested editables", () => {
  const f = fixture();
  for (const [index, value] of ["", "true", "TRUE", "plaintext-only"].entries()) {
    const host = f.add("DIV", { contenteditable: value, "aria-label": `Host ${index}` });
    host.textContent = "  old\n text  ";
    f.add("SPAN", {}, host).textContent = " inherited";
    f.add("SPAN", { contenteditable: "true" }, host).textContent = " nested";
  }
  const outer = f.add("DIV", { contenteditable: "true", "aria-label": "Outer" });
  const inherited = f.add("SPAN", { role: "textbox", "aria-label": "Inherited" }, outer);
  const nested = f.add("SPAN", { contenteditable: "plaintext-only", role: "textbox", "aria-label": "Nested" }, outer);
  const island = f.add("DIV", { contenteditable: "false" }, outer);
  f.add("DIV", { contenteditable: "true", "aria-label": "Island host" }, island);
  const shadow = f.add("DIV").attachShadow({ mode: "open" });
  f.add("DIV", { contenteditable: "true", "aria-label": "Shadow host" }, shadow);
  f.add("DIV", { contenteditable: "false" });
  f.add("DIV", { contenteditable: "invalid" });
  const page = f.bridge.read();
  assert.equal(page.interactive.length, 9);
  for (let index = 0; index < 4; index += 1) {
    const item = page.interactive.find((entry) => entry.label === `Host ${index}`);
    assert.equal(item.role, "textbox");
    assert.equal(item.value, "old text inherited nested");
  }
  for (const element of [inherited, nested]) {
    const ref = page.interactive.find((entry) => entry.label === element.getAttribute("aria-label")).ref;
    assert.equal(f.bridge.act({ action: "fill", ref, text: "new" }).error, "UNSUPPORTED_ELEMENT");
    assert.equal(element.replacements, undefined);
  }
  for (const label of ["Island host", "Shadow host"]) {
    const ref = page.interactive.find((entry) => entry.label === label).ref;
    assert.equal(f.bridge.act({ action: "fill", ref, text: "new" }).ok, true);
  }
});

test("contenteditable fill replaces markup with exact literal text and ordered synthetic input events", () => {
  const f = fixture();
  const host = f.add("DIV", { contenteditable: "", "aria-label": "Editor" });
  f.add("B", {}, host).textContent = "old";
  const text = "  <b>literal</b>\n다음 😀  ";
  const observed = [];
  host.onEvent = (event) => observed.push({ type: event.type, text: host.textContent,
    data: event.data, inputType: event.inputType, cancelable: event.cancelable,
    bubbles: event.bubbles, composed: event.composed, trusted: event.isTrusted });
  const result = f.bridge.act({ action: "fill", ref: editableRef(f), text });
  assert.equal(result.ok, true);
  assert.equal(result.changed, true);
  assert.equal(result.trusted, false);
  assert.equal(host.firstChild.nodeType, 3);
  assert.equal(host.firstChild.nextSibling, null);
  assert.equal(host.textContent, text);
  assert.deepEqual(observed, [
    { type: "beforeinput", text: "old", data: text, inputType: "insertText", cancelable: true, bubbles: true, composed: true, trusted: false },
    { type: "input", text, data: text, inputType: "insertText", cancelable: false, bubbles: true, composed: true, trusted: false },
  ]);
  assert.equal(f.bridge.act({ action: "fill", ref: editableRef(f), text: "" }).ok, true);
  assert.equal(host.textContent, "");
  assert.equal(host.firstChild.nodeType, 3);
});

test("contenteditable no-op is limited to canonical plain text and avoids focus/events", () => {
  for (const text of ["", "same", "x".repeat(20000)]) {
    const f = fixture();
    const host = f.add("DIV", { contenteditable: "true", "aria-label": "Editor" });
    if (text) host.textContent = text;
    const ref = editableRef(f);
    f.metrics.largestTextSlice = 0;
    const result = f.bridge.act({ action: "fill", ref, text });
    assert.equal(result.ok, true);
    assert.equal(result.changed, false);
    assert.equal(host.focused, undefined);
    assert.equal(host.replacements, undefined);
    assert.deepEqual(host.events, []);
    assert.ok(f.metrics.largestTextSlice <= 4096);
  }
  const f = fixture();
  const host = f.add("DIV", { contenteditable: "true", "aria-label": "Editor" });
  const bold = f.add("B", {}, host);
  bold.textContent = "same";
  const result = f.bridge.act({ action: "fill", ref: editableRef(f), text: "same" });
  assert.equal(result.changed, true);
  assert.equal(bold.isConnected, false);
  assert.equal(host.firstChild.nodeType, 3);
});

test("contenteditable beforeinput cancellation preserves the page handler's changes without replacing", () => {
  const f = fixture();
  const host = f.add("DIV", { contenteditable: "true", "aria-label": "Editor" });
  host.textContent = "old";
  host.onEvent = (event) => {
    if (event.type === "beforeinput") { event.preventDefault(); host.firstChild.data = "handler side effect"; }
  };
  const result = f.bridge.act({ action: "fill", ref: editableRef(f), text: "new" });
  assert.equal(result.error, "INPUT_CANCELED");
  assert.equal(result.attempted, false);
  assert.equal(result.trusted, false);
  assert.equal(host.textContent, "handler side effect");
  assert.equal(host.replacements, undefined);
  assert.deepEqual(host.events, ["beforeinput"]);
});

test("contenteditable readonly rejects before no-op and focus may reach the requested value", () => {
  const f = fixture();
  const host = f.add("DIV", { contenteditable: "true", "aria-readonly": "true", "aria-label": "Editor" });
  host.textContent = "same";
  const result = f.bridge.act({ action: "fill", ref: editableRef(f), text: "same" });
  assert.equal(result.error, "READ_ONLY");
  assert.equal(result.attempted, false);
  assert.equal(host.focused, undefined);
  host.removeAttribute("aria-readonly");
  host.onFocus = () => { host.firstChild.data = "new"; };
  const reached = f.bridge.act({ action: "fill", ref: editableRef(f), text: "new" });
  assert.equal(reached.ok, true);
  assert.equal(reached.changed, false);
  assert.equal(host.focused, 1);
  assert.deepEqual(host.events, []);
});

test("contenteditable focus and beforeinput revalidate host, ref, document and ancestor control state", () => {
  const mutations = [
    (_f, host) => host.remove(),
    (_f, host) => host.setAttribute("contenteditable", "false"),
    (_f, host) => host.setAttribute("aria-readonly", "true"),
    (_f, host) => { host.hidden = true; },
    (_f, host) => { host.parentElement.inert = true; },
    (_f, host) => host.parentElement.setAttribute("aria-disabled", "true"),
    (_f, host) => host.parentElement.setAttribute("contenteditable", "true"),
    (f) => f.bridge.read(),
    (_f, host) => { host.ownerDocument = fixture().document; },
  ];
  for (const phase of ["focus", "beforeinput"]) for (const mutate of mutations) {
    const f = fixture();
    const host = f.add("DIV", { contenteditable: "true", "aria-label": "Editor" });
    host.textContent = "old";
    if (phase === "focus") host.onFocus = () => mutate(f, host);
    else host.onEvent = (event) => { if (event.type === phase) mutate(f, host); };
    const result = f.bridge.act({ action: "fill", ref: editableRef(f), text: "new" });
    assert.equal(result.error, "ELEMENT_UNAVAILABLE");
    assert.equal(result.attempted, false);
    assert.equal(host.replacements, undefined);
    assert.equal(host.textContent, "old");
    assert.deepEqual(host.events, phase === "focus" ? [] : ["beforeinput"]);
  }
});

test("contenteditable replacement and input mutations report attempted without false success", () => {
  const cases = [
    ["replace", (host) => { host.hidden = true; }, "FILL_STATE_UNCONFIRMED"],
    ["input", (host) => host.remove(), "FILL_STATE_UNCONFIRMED"],
    ["input", (host) => host.setAttribute("aria-readonly", "true"), "FILL_STATE_UNCONFIRMED"],
    ["input", (host) => { host.textContent = "new"; }, "FILL_STATE_UNCONFIRMED"],
    ["input", (host) => { host.firstChild.data = "reverted"; }, "FILL_STATE_NOT_REACHED"],
  ];
  for (const [phase, mutate, error] of cases) {
    const f = fixture();
    const host = f.add("DIV", { contenteditable: "true", "aria-label": "Editor" });
    host.textContent = "old";
    if (phase === "replace") host.onReplace = () => mutate(host);
    else host.onEvent = (event) => { if (event.type === "input") mutate(host); };
    const result = f.bridge.act({ action: "fill", ref: editableRef(f), text: "new" });
    assert.equal(result.error, error);
    assert.equal(result.attempted, true);
    assert.equal(result.trusted, false);
    assert.equal(host.replacements, 1);
    assert.deepEqual(host.events, phase === "replace" ? ["beforeinput"] : ["beforeinput", "input"]);
  }
});

test("contenteditable exception receipts preserve only bounded attempted evidence", () => {
  for (const phase of ["focus", "replace", "input"]) {
    const f = fixture();
    const host = f.add("DIV", { contenteditable: "true", "aria-label": "Editor" });
    host.textContent = "old";
    const fail = () => { throw new Error("private-page-exception-text"); };
    if (phase === "focus") host.onFocus = fail;
    else if (phase === "replace") host.onReplace = fail;
    else host.onEvent = (event) => { if (event.type === "input") fail(); };
    const result = f.bridge.act({ action: "fill", ref: editableRef(f), text: "new" });
    assert.equal(result.error, "ACTION_FAILED");
    assert.equal(result.attempted, phase !== "focus");
    assert.equal(result.trusted, false);
    assert.ok(!JSON.stringify(result).includes("private-page-exception-text"));
  }
});

test("contenteditable read and equality remain bounded without whole-host text getters", () => {
  const f = fixture();
  const host = f.add("DIV", { contenteditable: "true", "aria-label": "Editor" });
  const long = f.text("x".repeat(1000000), host);
  for (const node of [host, long]) Object.defineProperty(node, "textContent", { get() { throw new Error("whole text read"); } });
  Object.defineProperty(host, "innerText", { get() { throw new Error("whole innerText read"); } });
  const page = f.bridge.read();
  assert.equal(page.interactive[0].value, "x".repeat(240));
  assert.ok(f.metrics.largestTextSlice <= 12001);
  f.metrics.largestTextSlice = 0;
  const result = f.bridge.act({ action: "fill", ref: page.interactive[0].ref, text: "short" });
  assert.equal(result.ok, true);
  assert.equal(host.firstChild.data, "short");
  assert.ok(f.metrics.largestTextSlice <= 4096);
});

test("contenteditable uses the accessible frame realm and rejects navigation around input", () => {
  const f = fixture();
  const { frame, child } = embed(f);
  const host = child.add("DIV", { contenteditable: "plaintext-only", "aria-label": "Editor" });
  host.textContent = "old";
  child.window.InputEvent = class extends child.window.InputEvent {
    constructor(...args) { super(...args); this.realm = "child"; }
  };
  host.onEvent = (event) => assert.equal(event.realm, "child");
  assert.equal(f.bridge.act({ action: "fill", ref: editableRef(f), text: "new" }).ok, true);
  assert.equal(host.firstChild.ownerDocument, child.document);
  host.onEvent = (event) => { if (event.type === "input") frame.contentDocument = fixture().document; };
  const result = f.bridge.act({ action: "fill", ref: editableRef(f), text: "later" });
  assert.equal(result.error, "FILL_STATE_UNCONFIRMED");
  assert.equal(result.attempted, true);
  assert.equal(host.firstChild.data, "later");
});

test("native form controls retain their existing fill policy despite a contenteditable attribute", () => {
  const f = fixture();
  const input = f.add("INPUT", { contenteditable: "true", "aria-label": "Input" });
  input.value = "native value";
  const file = f.add("INPUT", { type: "file", contenteditable: "true", "aria-label": "File" });
  const select = f.add("SELECT", { contenteditable: "true", "aria-label": "Select" });
  const page = f.bridge.read();
  assert.equal(page.interactive.find((item) => item.label === "Input").value, "native value");
  for (const element of [file, select]) {
    const ref = page.interactive.find((item) => item.label === element.getAttribute("aria-label")).ref;
    assert.equal(f.bridge.act({ action: "fill", ref, text: "new" }).error, "UNSUPPORTED_ELEMENT");
    assert.equal(element.replacements, undefined);
  }
  assert.equal(f.bridge.act({ action: "fill", ref: page.interactive.find((item) => item.label === "Input").ref, text: "new" }).ok, true);
  assert.equal(input.value, "new");
  assert.deepEqual(input.events, ["input", "change"]);
});

test("native fill detects setter normalization instead of claiming exact text success", () => {
  // Model observable native setter results; actual HTML type sanitization is
  // verified separately in Chromium, not implemented by this DOM double.
  for (const [tag, type, text, stored] of [
    ["INPUT", "number", "not-number", ""], ["INPUT", "date", "bad-date", ""],
    ["INPUT", "range", "999", "100"], ["INPUT", "text", "a\nb", "ab"],
    ["TEXTAREA", "textarea", "a\r\nb", "a\nb"],
  ]) {
    const f = fixture();
    const element = f.add(tag, { type });
    const prototype = Object.getPrototypeOf(element);
    const descriptor = Object.getOwnPropertyDescriptor(prototype, "value");
    Object.defineProperty(prototype, "value", { ...descriptor, set() { descriptor.set.call(this, stored); } });
    const ref = f.bridge.read().interactive[0].ref;
    const result = f.bridge.act({ action: "fill", ref, text });
    assert.equal(result.error, "FILL_STATE_NOT_REACHED", `${tag}/${type}`);
    assert.equal(result.attempted, true);
    assert.equal(result.trusted, false);
    assert.equal(element.value, stored);
    assert.deepEqual(element.events, ["input", "change"]);
  }
});

test("native fill observes handler rollback after input/change through the native getter", () => {
  for (const phase of ["input", "change"]) for (const tag of ["INPUT", "TEXTAREA"]) {
    const f = fixture();
    const element = f.add(tag);
    element.onEvent = (event) => { if (event.type === phase) element._value = "page rollback"; };
    Object.defineProperty(element, "value", { get() { return "requested"; } });
    const ref = f.bridge.read().interactive[0].ref;
    const result = f.bridge.act({ action: "fill", ref, text: "requested" });
    assert.equal(result.error, "FILL_STATE_NOT_REACHED");
    assert.equal(result.attempted, true);
    assert.equal(element.value, "requested", "the page's own getter lies about the final state");
    assert.equal(element._value, "page rollback");
    assert.deepEqual(element.events, ["input", "change"]);
  }
});

test("native fill stops remaining events when setter/input/change invalidate the target", () => {
  const mutations = [
    (_f, element) => element.remove(),
    (_f, element) => { element.hidden = true; },
    (_f, element) => { element.disabled = true; },
    (_f, element) => { element.readOnly = true; },
    (_f, element) => { element.parentElement.inert = true; },
    (_f, element) => { element.type = "password"; },
    (_f, element) => { element.type = "checkbox"; },
    (_f, element) => { element.ownerDocument = fixture().document; },
    (f) => f.bridge.read(),
  ];
  for (const phase of ["setter", "input", "change"]) for (const mutate of mutations) {
    const f = fixture();
    const element = f.add("INPUT");
    if (phase === "setter") {
      const prototype = Object.getPrototypeOf(element);
      const descriptor = Object.getOwnPropertyDescriptor(prototype, "value");
      Object.defineProperty(prototype, "value", { ...descriptor, set(value) {
        descriptor.set.call(this, value); mutate(f, this);
      } });
    } else element.onEvent = (event) => { if (event.type === phase) mutate(f, element); };
    const ref = f.bridge.read().interactive[0].ref;
    const result = f.bridge.act({ action: "fill", ref, text: "requested" });
    assert.equal(result.error, "FILL_STATE_UNCONFIRMED", phase);
    assert.equal(result.attempted, true);
    assert.equal(result.trusted, false);
    assert.deepEqual(element.events, phase === "setter" ? [] : phase === "input" ? ["input"] : ["input", "change"]);
  }
});

test("native fill verifies accessors and focus target before attempting the setter", () => {
  for (const change of ["getter", "setter", "tag", "readonly", "stale"]) {
    const f = fixture();
    const element = f.add("INPUT");
    element.value = "old";
    const ref = f.bridge.read().interactive[0].ref;
    const prototype = Object.getPrototypeOf(element);
    const descriptor = Object.getOwnPropertyDescriptor(prototype, "value");
    if (["getter", "setter"].includes(change)) {
      Object.defineProperty(prototype, "value", { ...descriptor, [change === "getter" ? "get" : "set"]: undefined });
    } else element.onFocus = () => {
      if (change === "tag") element.tagName = "TEXTAREA";
      if (change === "readonly") element.readOnly = true;
      if (change === "stale") f.bridge.read();
    };
    const result = f.bridge.act({ action: "fill", ref, text: "requested" });
    assert.equal(result.error, ["getter", "setter"].includes(change) ? "ACTION_FAILED" : "ELEMENT_UNAVAILABLE");
    assert.equal(result.attempted, false);
    assert.equal(element._value, "old");
    assert.deepEqual(element.events, []);
  }
});

test("native fill preserves attempted evidence on accessor exceptions without exposing password text", () => {
  for (const phase of ["focus", "setter", "input", "change", "getter"]) {
    const f = fixture();
    const element = f.add("INPUT", { type: "password" });
    const ref = f.bridge.read().interactive[0].ref;
    const secret = "private-native-fill-value";
    const fail = () => { throw new Error(secret); };
    if (phase === "focus") element.onFocus = fail;
    else if (["input", "change"].includes(phase)) element.onEvent = (event) => { if (event.type === phase) fail(); };
    else {
      const prototype = Object.getPrototypeOf(element);
      const descriptor = Object.getOwnPropertyDescriptor(prototype, "value");
      Object.defineProperty(prototype, "value", { ...descriptor, [phase === "getter" ? "get" : "set"]: fail });
    }
    const result = f.bridge.act({ action: "fill", ref, text: secret });
    assert.equal(result.error, "ACTION_FAILED");
    assert.equal(result.attempted, phase !== "focus");
    assert.equal(result.trusted, false);
    assert.ok(!JSON.stringify(result).includes(secret));
  }
});

test("native fill revalidates after reading the final value", () => {
  const f = fixture();
  const element = f.add("INPUT");
  const ref = f.bridge.read().interactive[0].ref;
  const prototype = Object.getPrototypeOf(element);
  const descriptor = Object.getOwnPropertyDescriptor(prototype, "value");
  Object.defineProperty(prototype, "value", { ...descriptor, get() {
    const value = descriptor.get.call(this);
    this.remove();
    return value;
  } });
  const result = f.bridge.act({ action: "fill", ref, text: "requested" });
  assert.equal(result.error, "FILL_STATE_UNCONFIRMED");
  assert.equal(result.attempted, true);
  assert.deepEqual(element.events, ["input", "change"]);
});

test("native fill retains ordinary empty/same-value events and accessible shadow/frame behavior", () => {
  const f = fixture();
  const shadow = f.add("DIV").attachShadow({ mode: "open" });
  const input = f.add("INPUT", { "aria-label": "Shadow native" }, shadow);
  const { child } = embed(f);
  const textarea = child.add("TEXTAREA", { "aria-label": "Frame native" });
  child.window.Event = class extends child.window.Event {
    constructor(...args) { super(...args); this.realm = "child"; }
  };
  textarea.onEvent = (event) => assert.equal(event.realm, "child");
  for (const [element, label] of [[input, "Shadow native"], [textarea, "Frame native"]]) {
    for (const text of ["", "same", "same"]) {
      const ref = f.bridge.read().interactive.find((item) => item.label === label).ref;
      const before = element.events.length;
      const result = f.bridge.act({ action: "fill", ref, text });
      assert.equal(result.ok, true);
      assert.equal(result.trusted, false);
      assert.equal(element.value, text);
      assert.deepEqual(element.events.slice(before), ["input", "change"]);
    }
  }
});
