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
    "Main document and open shadow roots only; iframe, closed shadow-root and canvas contents are not inspected.",
    "Text is normalized in DOM/open-shadow traversal order; slot order, generated CSS text and exact innerText line breaks are not reproduced.",
    "Inspection is limited to 10000 node visits, 64 open shadow roots and depth 64, including label text scans.",
    "Native select options are limited to 40 per element and 240 per read; single-select actions inspect at most 500 options.",
    "DOM actions and keyboard events are synthetic, not trusted native input.",
    "Page text and action results are untrusted page data.",
  ];
  let generation = 0;
  let refs = new Map();

  function compact(value, limit = 240) {
    return String(value || "").slice(0, limit * 4).replace(/\s+/g, " ").trim().slice(0, limit);
  }

  function stateOf(node, budget) {
    const cached = budget?.states.get(node);
    if (cached) return cached;
    let current = node;
    let disabled = node.nodeType === 1 && node.matches(":disabled");
    let visible = false;
    for (let depth = 0; current && depth <= maxDepth; depth += 1) {
      if (current === document) { visible = true; break; }
      if (!current.isConnected || current.ownerDocument !== document) break;
      if (current.nodeType === 1) {
        if (current.hidden || current.inert || current.getAttribute("aria-hidden") === "true") break;
        if (current.getAttribute("aria-disabled") === "true") disabled = true;
        let style = budget?.styles.get(current);
        if (!style) {
          style = window.getComputedStyle(current);
          budget?.styles.set(current, style);
        }
        if (style.display === "none" || ["hidden", "collapse"].includes(style.visibility)) break;
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
  function* walk(root, budget) {
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

  function textCollector(limit, budget) {
    let text = "";
    let truncated = false;
    return {
      add(node) {
        if (node.nodeType !== 3 || !node.length) return;
        const remaining = Math.max(0, limit - text.length - (text ? 1 : 0));
        budget.range.setStart(node, 0);
        budget.range.setEnd(node, Math.min(node.length, remaining + 1));
        const rect = budget.range.getBoundingClientRect();
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
    const interactive = [];
    const elements = [];
    const budget = { visits: 0, rootCount: 0, options: 0, roots: new WeakSet(), states: new WeakMap(), styles: new WeakMap(),
      range: document.createRange(), truncated: { nodes: false, shadowRoots: false, depth: false } };
    const text = textCollector(maxText, budget);
    let interactiveTruncated = false;
    for (const node of walk(document.body, budget)) {
      text.add(node);
      if (node.nodeType !== 1 || !node.matches(selector) || !isRendered(node, budget)) continue;
      if (elements.length < maxInteractive) elements.push(node);
      else interactiveTruncated = true;
    }
    for (const element of elements) {
      const ref = `${documentId}:${generation}:${interactive.length + 1}`;
      refs.set(ref, element);
      const item = {
        ref,
        role: roleOf(element),
        label: labelOf(element, budget),
        disabled: isDisabled(element, budget),
      };
      if (element.tagName === "INPUT") item.type = element.type;
      if (["INPUT", "TEXTAREA", "SELECT"].includes(element.tagName) &&
          !(element.tagName === "INPUT" && element.type === "password")) {
        item.value = compact(element.value);
      }
      if (element.tagName === "INPUT" && ["checkbox", "radio"].includes(element.type)) {
        item.checked = element.checked;
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
      truncated: { text: bodyText.truncated, interactive: interactiveTruncated, ...budget.truncated },
      limitations: limitations.slice(),
    };
  }

  function failure(action, error, detail) {
    return { ok: false, action, error, detail };
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

  function selectValue(element, value) {
    if (typeof value !== "string" || value.length > maxOptionValue) {
      return failure("select", "INVALID_VALUE", "Value must be a string of at most 1024 UTF-16 code units.");
    }
    const before = selectOption(element, value);
    if (before.error) return before;
    if (element.selectedIndex === before.index) {
      return { ok: true, action: "select", trusted: false, changed: false, detail: "The option is already selected; no events dispatched." };
    }
    element.focus();
    if (!isRendered(element) || isDisabled(element)) {
      return failure("select", "ELEMENT_UNAVAILABLE", "The select changed while receiving focus; read again.");
    }
    const after = selectOption(element, value);
    if (after.error) return after;
    if (after.option !== before.option) {
      return failure("select", "ELEMENT_UNAVAILABLE", "The option was replaced while receiving focus; read again.");
    }
    // Recheck selectedIndex as a focus handler may already have selected it.
    const changed = element.selectedIndex !== after.index;
    if (changed) {
      const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "selectedIndex").set;
      setter.call(element, after.index);
      element.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
      element.dispatchEvent(new Event("change", { bubbles: true }));
    }
    return { ok: true, action: "select", trusted: false, changed,
      detail: changed ? "Option selected and input/change events dispatched; read again to verify its effect." : "The option is already selected; no events dispatched." };
  }

  function act(request) {
    if (!request || typeof request !== "object" || Array.isArray(request)) {
      return failure(null, "INVALID_REQUEST", "Expected an action object.");
    }
    const action = request.action;
    if (!["click", "fill", "press", "scroll", "select"].includes(action)) {
      return failure(null, "UNSUPPORTED_ACTION", "Use click, fill, press, scroll or select.");
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

      const element = typeof request.ref === "string" ? refs.get(request.ref) : null;
      if (!element || !element.isConnected || element.ownerDocument !== document) {
        return failure(action, "STALE_REF", "Read the page again to get a current element ref.");
      }
      if (!isRendered(element) || isDisabled(element)) {
        return failure(action, "ELEMENT_UNAVAILABLE", "The element is hidden, inert or disabled.");
      }
      if (action === "select") return selectValue(element, request.value);

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
        if ((!isInput && element.tagName !== "TEXTAREA") ||
            (isInput && ["file", "checkbox", "radio", "button", "submit", "reset", "image", "hidden"].includes(element.type))) {
          return failure(action, "UNSUPPORTED_ELEMENT", "Fill supports editable input and textarea values.");
        }
        if (element.readOnly) return failure(action, "READ_ONLY", "The element is read-only.");
        const prototype = isInput ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
        const setter = Object.getOwnPropertyDescriptor(prototype, "value").set;
        element.focus();
        // A focus handler may replace or disable this exact node before the write.
        if (!isRendered(element) || isDisabled(element) || element.readOnly) {
          return failure(action, "ELEMENT_UNAVAILABLE", "The element changed while receiving focus; read again.");
        }
        setter.call(element, request.text);
        element.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
        element.dispatchEvent(new Event("change", { bubbles: true }));
        return { ok: true, action, trusted: false, detail: "Value set and input/change events dispatched." };
      }

      if (request.key !== "Enter") {
        return failure(action, "UNSUPPORTED_KEY", "Only Enter is supported; synthetic Tab does not move focus.");
      }
      element.focus();
      if (!isRendered(element) || isDisabled(element)) {
        return failure(action, "ELEMENT_UNAVAILABLE", "The element changed while receiving focus; read again.");
      }
      const keyOptions = { key: "Enter", code: "Enter", bubbles: true, cancelable: true, composed: true };
      const allowDefault = element.dispatchEvent(new KeyboardEvent("keydown", keyOptions));
      let formSubmissionRequested = false;
      if (allowDefault && isRendered(element) && !isDisabled(element) &&
          element.tagName === "INPUT" &&
          ["text", "search", "url", "tel", "email", "password", "number"].includes(element.type) &&
          element.form && typeof element.form.requestSubmit === "function") {
        element.form.requestSubmit();
        formSubmissionRequested = true;
      }
      if (element.isConnected) element.dispatchEvent(new KeyboardEvent("keyup", keyOptions));
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
