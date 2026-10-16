(() => {
  "use strict";

  const bridgeName = "__lightBrowserBridge";
  if (Object.prototype.hasOwnProperty.call(window, bridgeName)) {
    return window[bridgeName];
  }

  const documentId = Array.from(crypto.getRandomValues(new Uint32Array(4)), (n) =>
    n.toString(16).padStart(8, "0"),
  ).join("");
  const maxText = 12000;
  const maxInteractive = 120;
  const maxNodes = 10000;
  const maxShadowRoots = 64;
  const maxDepth = 64;
  const maxFrameDocuments = 16;
  const maxFrameDepth = 8;
  const maxOptionValue = 1024;
  const maxActionOptions = 500;
  const maxReadOptions = 40;
  const maxTotalReadOptions = 240;
  const noTextChildren = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "IFRAME", "CANVAS", "OBJECT", "EMBED", "INPUT", "TEXTAREA", "SELECT"]);
  const selector = [
    "a[href]", "button", "input:not([type=hidden])", "textarea", "select",
    '[role="button"]', '[role="link"]', '[role="textbox"]',
    '[role="checkbox"]', '[role="radio"]', '[role="combobox"]',
    '[contenteditable="true"]', "[tabindex]",
  ].join(",");
  const limitations = [
    "Main document, open shadow roots and accessible same-origin iframe documents only; inaccessible frames, closed shadow-root and canvas contents are not inspected.",
    "Text is normalized in DOM/open-shadow traversal order; slot order, generated CSS text and exact innerText line breaks are not reproduced.",
    "Inspection is limited to 10000 node visits, 64 open shadow roots and depth 64, including label text scans.",
    "Iframe metadata is limited to 16 slots and iframe nesting to depth 8; unavailable frames also consume a slot. Frame ids are valid for this read only.",
    "Viewport and scroll describe the main window only; rendered DOM is not a pixel or occlusion match for a screenshot.",
    "Native select options are limited to 40 per element and 240 per read; single-select actions inspect at most 500 options.",
    "DOM actions and keyboard events are synthetic, not trusted native input.",
    "Page text and action results are untrusted page data.",
  ];
  let generation = 0;
  let refs = new Map();
  let contexts = new WeakMap();

  function compact(value, limit = 240) {
    return String(value || "").slice(0, limit * 4).replace(/\s+/g, " ").trim().slice(0, limit);
  }

  function contextIsCurrent(context) {
    try {
      for (let depth = 0; context && depth <= maxFrameDepth; depth += 1) {
        if (!context.parent) return context.document === document;
        const frame = context.element;
        if (!frame.isConnected || frame.ownerDocument !== context.parent.document ||
            frame.contentDocument !== context.document) return false;
        context = context.parent;
      }
    } catch (_error) {
      // Cross-origin or detached frame access does not expose exception text.
    }
    return false;
  }

  function refIsCurrent(reference) {
    return !!reference?.context && reference.generation === generation && reference.element.isConnected &&
      reference.element.ownerDocument === reference.context.document &&
      contextIsCurrent(reference.context);
  }

  function stateOf(node, budget) {
    const cached = budget?.states.get(node);
    if (cached) return cached;
    let current = node;
    let context = contexts.get(node.nodeType === 9 ? node : node.ownerDocument);
    let disabled = node.nodeType === 1 && node.matches(":disabled");
    let visible = false;
    if (!context || !contextIsCurrent(context)) return { visible, disabled };
    for (let depth = 0; current && depth <= maxDepth; depth += 1) {
      if (current === context.document) {
        if (!context.parent) { visible = true; break; }
        current = context.element;
        context = context.parent;
      }
      if (!current.isConnected || current.ownerDocument !== context.document) break;
      if (current.nodeType === 1) {
        if (current.hidden || current.inert || current.getAttribute("aria-hidden") === "true") break;
        if (current.getAttribute("aria-disabled") === "true") disabled = true;
        let style = budget?.styles.get(current);
        if (!style) {
          style = context.view.getComputedStyle(current);
          budget?.styles.set(current, style);
        }
        if (style.display === "none" || ["hidden", "collapse"].includes(style.visibility)) break;
        if (current.tagName === "IFRAME") {
          const rect = current.getBoundingClientRect();
          if (!(rect.width > 0 && rect.height > 0)) break;
          if (current.matches(":disabled")) disabled = true;
        }
      }
      const parent = current.parentNode;
      // A light child with no assigned slot is not part of its host's rendered tree.
      if (!current.assignedSlot && parent?.nodeType === 1 && parent.shadowRoot) break;
      current = current.assignedSlot || parent || current.host;
      if (depth === maxDepth && current) {
        if (budget) budget.truncated.depth = true;
      }
    }
    const state = { visible, disabled };
    budget?.states.set(node, state);
    return state;
  }

  function isRendered(element, budget) {
    if (!stateOf(element, budget).visible) return false;
    const rect = element.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }

  function isDisabled(element, budget) {
    return stateOf(element, budget).disabled;
  }

  function supportsFill(element) {
    return element.tagName === "TEXTAREA" || (element.tagName === "INPUT" &&
      !["file", "checkbox", "radio", "button", "submit", "reset", "image", "hidden"].includes(element.type));
  }

  function optionState(option, budget) {
    const state = stateOf(option, budget);
    // Native options commonly have no layout box even when the menu can open.
    // Disabled optgroups affect their options without disabling the select.
    return {
      hidden: !state.visible,
      disabled: state.disabled || !!option.disabled ||
        (option.parentNode?.tagName === "OPTGROUP" && !!option.parentNode.disabled),
    };
  }

  function readOptions(element, item, budget) {
    const options = element.options;
    const count = Math.min(options.length, maxReadOptions, maxTotalReadOptions - budget.options);
    item.multiple = element.multiple;
    item.options = [];
    item.optionsTruncated = count < options.length;
    for (let index = 0; index < count; index += 1) {
      const option = options[index];
      const state = optionState(option, budget);
      const entry = { selected: option.selected, ...state };
      if (!state.hidden) {
        const value = option.value;
        if (value.length <= maxOptionValue) entry.value = value;
        else entry.valueTooLong = true;
        entry.label = compact(option.label);
      }
      item.options.push(entry);
      budget.options += 1;
    }
  }

  // Sibling cursors avoid materializing the document, child lists or assignedNodes().
  function* walk(root, budget, inspectFrames = false) {
    if (!root) return;
    const stack = [{ next: root, depth: 0, single: true }];
    while (stack.length) {
      const frame = stack[stack.length - 1];
      const node = frame.next;
      if (!node) { stack.pop(); continue; }
      frame.next = frame.single ? null : node.nextSibling;
      if (budget.visits >= maxNodes) { budget.truncated.nodes = true; return; }
      budget.visits += 1;
      if (frame.depth > maxDepth) { budget.truncated.depth = true; continue; }
      if (!stateOf(node, budget).visible) continue;
      yield node;
      if (inspectFrames && node.nodeType === 1 && node.tagName === "IFRAME") {
        const context = inspectFrame(node, budget);
        if (context) stack.push({ next: context.document.body, depth: frame.depth + 1, single: true });
        continue;
      }
      if (node.nodeType === 1 && noTextChildren.has(node.tagName)) continue;
      if (node.firstChild) stack.push({ next: node.firstChild, depth: frame.depth + 1 });
      const shadow = node.nodeType === 1 ? node.shadowRoot : null;
      if (shadow) {
        if (!budget.roots.has(shadow)) {
          if (budget.rootCount >= maxShadowRoots) { budget.truncated.shadowRoots = true; continue; }
          budget.roots.add(shadow);
          budget.rootCount += 1;
        }
        stack.push({ next: shadow, depth: frame.depth + 1, single: true });
      }
    }
  }

  function inspectFrame(element, budget) {
    if (budget.frames.length >= maxFrameDocuments) {
      budget.truncated.frames = true;
      return null;
    }
    const parent = contexts.get(element.ownerDocument);
    if (!parent || !contextIsCurrent(parent)) return null;
    const entry = {
      id: `frame-${budget.frames.length + 1}`,
      parent: parent.id,
      label: compact(element.getAttribute("aria-label") || element.getAttribute("title") || element.getAttribute("name")),
      status: "unavailable",
    };
    budget.frames.push(entry);
    if (parent.depth >= maxFrameDepth) {
      entry.status = "limit";
      budget.truncated.frameDepth = true;
      return null;
    }
    try {
      const child = element.contentDocument;
      const view = child?.defaultView;
      if (!child?.body || !view || !["interactive", "complete"].includes(child.readyState)) return null;
      const context = { id: entry.id, document: child, view, element, parent, depth: parent.depth + 1 };
      if (!contextIsCurrent(context)) return null;
      const url = String(child.URL).slice(0, 4096);
      const title = String(child.title).slice(0, 512);
      if (!contextIsCurrent(context)) return null;
      contexts.set(child, context);
      entry.status = "accessible";
      entry.url = url;
      entry.title = title;
      return context;
    } catch (_error) {
      // No contentWindow/document fallback or origin-policy bypass.
      return null;
    }
  }

  function textCollector(limit, budget) {
    let text = "";
    let truncated = false;
    return {
      add(node) {
        if (node.nodeType !== 3 || !node.length) return;
        const remaining = Math.max(0, limit - text.length - (text ? 1 : 0));
        let range = budget.ranges.get(node.ownerDocument);
        if (!range) {
          range = node.ownerDocument.createRange();
          budget.ranges.set(node.ownerDocument, range);
        }
        range.setStart(node, 0);
        range.setEnd(node, Math.min(node.length, remaining + 1));
        const rect = range.getBoundingClientRect();
        if (!rect.width || !rect.height) return;
        const sample = node.substringData(0, remaining + 1);
        const value = sample.replace(/\s+/g, " ").trim();
        if (node.length > remaining || value.length > remaining) truncated = true;
        if (value && remaining) text += (text ? " " : "") + value.slice(0, remaining);
      },
      result() { return { text, truncated }; },
    };
  }

  function textOf(element, budget, limit = 240) {
    const collector = textCollector(limit, budget);
    for (const node of walk(element, budget)) {
      collector.add(node);
      if (collector.result().text.length >= limit) break;
    }
    return collector.result().text;
  }

  function roleOf(element) {
    const explicit = element.getAttribute("role");
    if (explicit) return compact(explicit, 80);
    if (element.tagName === "A") return "link";
    if (element.tagName === "BUTTON") return "button";
    if (element.tagName === "SELECT") return element.multiple ? "listbox" : "combobox";
    if (element.tagName === "INPUT") {
      if (["checkbox", "radio"].includes(element.type)) return element.type;
      if (["button", "submit", "reset", "image"].includes(element.type)) return "button";
      if (element.type === "range") return "slider";
      if (element.type === "number") return "spinbutton";
    }
    if (["INPUT", "TEXTAREA"].includes(element.tagName) || element.isContentEditable) {
      return "textbox";
    }
    return "generic";
  }

  function labelOf(element, budget) {
    const explicit = compact(element.getAttribute("aria-label"));
    if (explicit) return explicit;
    const root = element.getRootNode();
    let labelledBy = "";
    const ids = (element.getAttribute("aria-labelledby") || "").slice(0, 1024).split(/\s+/).slice(0, 16);
    for (const id of ids) {
      const label = id && root.getElementById?.(id);
      if (label && labelledBy.length < 240) labelledBy += " " + textOf(label, budget, 240 - labelledBy.length);
    }
    if (compact(labelledBy)) return compact(labelledBy);
    let labels = "";
    for (let i = 0; i < Math.min(element.labels?.length || 0, 16) && labels.length < 240; i += 1) {
      labels += " " + textOf(element.labels[i], budget, 240 - labels.length);
    }
    const buttonValue = element.tagName === "INPUT" &&
      ["button", "submit", "reset"].includes(element.type) ? element.value : "";
    return compact(compact(labels) ||
      element.getAttribute("alt") || element.getAttribute("title") ||
      element.getAttribute("placeholder") || textOf(element, budget) || buttonValue);
  }

  function read() {
    generation += 1;
    refs = new Map();
    contexts = new WeakMap();
    contexts.set(document, { id: "main", document, view: window, parent: null, depth: 0 });
    const interactive = [];
    const elements = [];
    const budget = { visits: 0, rootCount: 0, options: 0, roots: new WeakSet(), states: new WeakMap(), styles: new WeakMap(),
      ranges: new WeakMap(), frames: [], truncated: { nodes: false, shadowRoots: false, depth: false, frames: false, frameDepth: false } };
    const text = textCollector(maxText, budget);
    let interactiveTruncated = false;
    for (const node of walk(document.body, budget, true)) {
      text.add(node);
      if (node.nodeType !== 1 || !node.matches(selector) || !isRendered(node, budget)) continue;
      if (elements.length < maxInteractive) elements.push(node);
      else interactiveTruncated = true;
    }
    for (const element of elements) {
      const ref = `${documentId}:${generation}:${interactive.length + 1}`;
      const context = contexts.get(element.ownerDocument);
      const reference = { element, context, generation };
      if (!refIsCurrent(reference)) continue;
      refs.set(ref, reference);
      const item = {
        ref,
        role: roleOf(element),
        label: labelOf(element, budget),
        disabled: isDisabled(element, budget),
      };
      if (context.parent) item.frame = context.id;
      if (element.tagName === "INPUT") item.type = element.type;
      if (["INPUT", "TEXTAREA", "SELECT"].includes(element.tagName) &&
          !(element.tagName === "INPUT" && element.type === "password")) {
        item.value = compact(element.value);
      }
      if (element.tagName === "INPUT" && ["checkbox", "radio"].includes(element.type)) {
        item.checked = element.checked;
        if (element.type === "checkbox") item.indeterminate = element.indeterminate;
      }
      if (element.tagName === "SELECT") readOptions(element, item, budget);
      interactive.push(item);
    }
    const bodyText = text.result();
    return {
      url: String(location.href).slice(0, 4096),
      title: String(document.title).slice(0, 512),
      documentId,
      readyState: document.readyState,
      viewport: { x: window.scrollX, y: window.scrollY, width: window.innerWidth, height: window.innerHeight },
      generation,
      text: bodyText.text,
      interactive,
      frames: budget.frames,
      truncated: { text: bodyText.truncated, interactive: interactiveTruncated, ...budget.truncated },
      limitations: limitations.slice(),
    };
  }

  function failure(action, error, detail) {
    return { ok: false, action, error, detail };
  }

  function refFailure(reference, action, detail = "The element is hidden, inert or disabled.", staleError = "STALE_REF") {
    if (!refIsCurrent(reference)) {
      return failure(action, staleError, staleError === "STALE_REF"
        ? "Read the page again to get a current element ref." : detail);
    }
    if (!isRendered(reference.element) || isDisabled(reference.element)) {
      return failure(action, "ELEMENT_UNAVAILABLE", detail);
    }
    return null;
  }

  function selectOption(element, value) {
    if (element.tagName !== "SELECT" || element.multiple) {
      return failure("select", "UNSUPPORTED_ELEMENT", "Select supports native single-select elements only.");
    }
    const options = element.options;
    if (options.length > maxActionOptions) {
      return failure("select", "OPTION_LIMIT", "The select has more than 500 options.");
    }
    let match = null;
    let index = -1;
    for (let position = 0; position < options.length; position += 1) {
      if (options[position].value !== value) continue;
      if (match) return failure("select", "AMBIGUOUS_OPTION", "More than one option has that exact value.");
      match = options[position];
      index = position;
    }
    if (!match) return failure("select", "OPTION_NOT_FOUND", "No option has that exact value.");
    const state = optionState(match);
    if (state.hidden || state.disabled) {
      return failure("select", "OPTION_UNAVAILABLE", "The option is hidden, inert or disabled.");
    }
    return { option: match, index };
  }

  function selectValue(reference, value) {
    const { element, context } = reference;
    if (typeof value !== "string" || value.length > maxOptionValue) {
      return failure("select", "INVALID_VALUE", "Value must be a string of at most 1024 UTF-16 code units.");
    }
    const before = selectOption(element, value);
    if (before.error) return before;
    if (element.selectedIndex === before.index) {
      return { ok: true, action: "select", trusted: false, changed: false, detail: "The option is already selected; no events dispatched." };
    }
    element.focus();
    const unavailable = refFailure(reference, "select", "The select changed while receiving focus; read again.", "ELEMENT_UNAVAILABLE");
    if (unavailable) return unavailable;
    const after = selectOption(element, value);
    if (after.error) return after;
    if (after.option !== before.option) {
      return failure("select", "ELEMENT_UNAVAILABLE", "The option was replaced while receiving focus; read again.");
    }
    // Recheck selectedIndex as a focus handler may already have selected it.
    const changed = element.selectedIndex !== after.index;
    if (changed) {
      const setter = Object.getOwnPropertyDescriptor(context.view.HTMLSelectElement.prototype, "selectedIndex").set;
      setter.call(element, after.index);
      element.dispatchEvent(new context.view.Event("input", { bubbles: true, composed: true }));
      if (refFailure(reference, "select")) {
        return { ok: true, action: "select", trusted: false, changed,
          detail: "Option selected and input event dispatched; the target changed before the change event, which was not dispatched. Read again to verify its effect." };
      }
      element.dispatchEvent(new context.view.Event("change", { bubbles: true }));
    }
    return { ok: true, action: "select", trusted: false, changed,
      detail: changed ? "Option selected and input/change events dispatched; read again to verify its effect." : "The option is already selected; no events dispatched." };
  }

  function checkState(element, checked) {
    if (element.tagName !== "INPUT" || !["checkbox", "radio"].includes(element.type)) {
      return failure("check", "UNSUPPORTED_ELEMENT", "Check supports native checkbox and radio inputs only.");
    }
    if ((element.type === "radio" && !checked) || (element.type === "checkbox" && element.indeterminate)) {
      return failure("check", "UNSUPPORTED_STATE", "Select a radio with true; mixed checkboxes require page-specific handling.");
    }
    return null;
  }

  function checkValue(reference, checked) {
    const { element } = reference;
    if (typeof checked !== "boolean") {
      return failure("check", "INVALID_VALUE", "Checked must be a boolean.");
    }
    const unsupported = checkState(element, checked);
    if (unsupported) return unsupported;
    const unchanged = () => ({ ok: true, action: "check", trusted: false, changed: false, checked,
      detail: "The requested state is already set; no click, input or change events dispatched by this action." });
    if (element.checked === checked) return unchanged();

    const originalType = element.type;
    element.focus();
    const unavailable = refFailure(reference, "check", "The input changed while receiving focus; read again.", "ELEMENT_UNAVAILABLE");
    if (unavailable) return unavailable;
    if (element.tagName !== "INPUT" || element.type !== originalType) {
      return failure("check", "ELEMENT_UNAVAILABLE", "The input type changed while receiving focus; read again.");
    }
    const afterFocus = checkState(element, checked);
    if (afterFocus) return afterFocus;
    if (element.checked === checked) return unchanged();

    // Native activation handles radio groups and cancellation. Do not assign
    // checked or manufacture input/change events; never retry a dispatched click.
    try {
      element.click();
      if (refFailure(reference, "check") || element.tagName !== "INPUT" || element.type !== originalType) {
        return { ...failure("check", "CHECK_STATE_UNCONFIRMED", "DOM click dispatched, but the input changed or became unavailable. Read again before deciding whether to retry."),
          trusted: false, attempted: true };
      }
      const current = { checked: element.checked };
      if (originalType === "checkbox") current.indeterminate = element.indeterminate;
      if (current.checked !== checked || current.indeterminate) {
        return { ...failure("check", "CHECK_STATE_NOT_REACHED", "DOM click dispatched, but the requested state was not reached. Page handlers may have canceled or changed it; read again."),
          trusted: false, attempted: true, ...current };
      }
      return { ok: true, action: "check", trusted: false, changed: true, ...current,
        detail: "DOM click dispatched and the requested state observed synchronously; read again to verify later page effects." };
    } catch (_error) {
      return { ...failure("check", "ACTION_FAILED", "The click or its state check raised an exception. Read again before deciding whether to retry."),
        trusted: false, attempted: true };
    }
  }

  function act(request) {
    if (!request || typeof request !== "object" || Array.isArray(request)) {
      return failure(null, "INVALID_REQUEST", "Expected an action object.");
    }
    const action = request.action;
    if (!["click", "fill", "press", "scroll", "select", "check"].includes(action)) {
      return failure(null, "UNSUPPORTED_ACTION", "Use click, fill, press, scroll, select or check.");
    }
    try {
      if (action === "scroll") {
        const x = request.x ?? 0;
        const y = request.y ?? Math.round(window.innerHeight * 0.8);
        if (typeof x !== "number" || typeof y !== "number" ||
            !Number.isFinite(x) || !Number.isFinite(y) || Math.abs(x) > 10000 || Math.abs(y) > 10000) {
          return failure(action, "INVALID_SCROLL", "Scroll offsets must be finite numbers within 10000 pixels.");
        }
        window.scrollBy({ left: x, top: y, behavior: "instant" });
        return { ok: true, action, x: window.scrollX, y: window.scrollY };
      }

      const reference = typeof request.ref === "string" ? refs.get(request.ref) : null;
      const unavailable = refFailure(reference, action);
      if (unavailable) return unavailable;
      const { element, context } = reference;
      if (action === "select") return selectValue(reference, request.value);
      if (action === "check") return checkValue(reference, request.checked);

      if (action === "click") {
        if (typeof element.click !== "function") {
          return failure(action, "UNSUPPORTED_ELEMENT", "This element has no DOM click method.");
        }
        element.click();
        return { ok: true, action, trusted: false, detail: "DOM click dispatched; read again to verify its effect." };
      }

      if (action === "fill") {
        if (typeof request.text !== "string") {
          return failure(action, "INVALID_TEXT", "Text must be a string.");
        }
        const isInput = element.tagName === "INPUT";
        if (!supportsFill(element)) {
          return failure(action, "UNSUPPORTED_ELEMENT", "Fill supports editable input and textarea values.");
        }
        if (element.readOnly) return failure(action, "READ_ONLY", "The element is read-only.");
        const prototype = isInput ? context.view.HTMLInputElement.prototype : context.view.HTMLTextAreaElement.prototype;
        const setter = Object.getOwnPropertyDescriptor(prototype, "value").set;
        element.focus();
        // A focus handler may replace, retype or disable this node before the write.
        const changed = refFailure(reference, action, "The element changed while receiving focus; read again.", "ELEMENT_UNAVAILABLE");
        if (changed) return changed;
        if (element.readOnly || !supportsFill(element)) {
          return failure(action, "ELEMENT_UNAVAILABLE", "The element changed while receiving focus; read again.");
        }
        setter.call(element, request.text);
        element.dispatchEvent(new context.view.Event("input", { bubbles: true, composed: true }));
        if (refFailure(reference, action)) {
          return { ok: true, action, trusted: false,
            detail: "Value set and input event dispatched; the target changed before the change event, which was not dispatched. Read again to verify its effect." };
        }
        element.dispatchEvent(new context.view.Event("change", { bubbles: true }));
        return { ok: true, action, trusted: false, detail: "Value set and input/change events dispatched." };
      }

      if (request.key !== "Enter") {
        return failure(action, "UNSUPPORTED_KEY", "Only Enter is supported; synthetic Tab does not move focus.");
      }
      element.focus();
      const changed = refFailure(reference, action, "The element changed while receiving focus; read again.", "ELEMENT_UNAVAILABLE");
      if (changed) return changed;
      const keyOptions = { key: "Enter", code: "Enter", bubbles: true, cancelable: true, composed: true };
      const allowDefault = element.dispatchEvent(new context.view.KeyboardEvent("keydown", keyOptions));
      const afterKey = refFailure(reference, action, "The element changed while handling Enter; read again.");
      if (afterKey) {
        return { ok: true, action, trusted: false, formSubmissionRequested: false,
          detail: "Synthetic Enter keydown dispatched; the target changed, so no form submission or keyup was requested. Read again to verify its effect." };
      }
      let formSubmissionRequested = false;
      if (allowDefault &&
          element.tagName === "INPUT" &&
          ["text", "search", "url", "tel", "email", "password", "number"].includes(element.type) &&
          element.form && typeof element.form.requestSubmit === "function") {
        element.form.requestSubmit();
        formSubmissionRequested = true;
      }
      if (refFailure(reference, action)) {
        return { ok: true, action, trusted: false, formSubmissionRequested,
          detail: formSubmissionRequested
            ? "Synthetic Enter keydown dispatched and form.requestSubmit() requested; the target changed before keyup, which was not dispatched. Read again to verify its effect."
            : "Synthetic Enter keydown dispatched; the target changed before keyup, which was not dispatched. Read again to verify its effect." };
      }
      element.dispatchEvent(new context.view.KeyboardEvent("keyup", keyOptions));
      return {
        ok: true, action, trusted: false, formSubmissionRequested,
        detail: formSubmissionRequested
          ? "Synthetic Enter dispatched and form.requestSubmit() requested; validation and page handlers determine the outcome."
          : "Synthetic Enter dispatched; native default behavior is not simulated.",
      };
    } catch (_error) {
      // Do not echo page errors: they can include input values or other page data.
      return failure(action, "ACTION_FAILED", "The page action raised an exception; read again to inspect the page.");
    }
  }

  const bridge = Object.freeze({ read, act });
  Object.defineProperty(window, bridgeName, { value: bridge, writable: false, configurable: false });
  return bridge;
})();
