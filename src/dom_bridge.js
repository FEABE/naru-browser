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
  const defaultReadLimits = Object.freeze({ text: maxText, elements: maxInteractive, nodes: maxNodes,
    roots: maxShadowRoots, options: maxTotalReadOptions, frames: maxFrameDocuments });
  const maxPointerCoordinate = 1000000;
  const maxEditingText = 1024 * 1024;
  const unsupportedGeometry = {};
  const pointerFractions = [[0.5, 0.5], [0.1, 0.1], [0.9, 0.1], [0.1, 0.9], [0.9, 0.9],
    [0.5, 0.1], [0.5, 0.9], [0.1, 0.5], [0.9, 0.5]];
  const noTextChildren = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "IFRAME", "CANVAS", "OBJECT", "EMBED", "INPUT", "TEXTAREA", "SELECT"]);
  const shadowHostTags = new Set(["ARTICLE", "ASIDE", "BLOCKQUOTE", "BODY", "DIV", "FOOTER", "H1", "H2", "H3", "H4", "H5", "H6", "HEADER", "MAIN", "NAV", "P", "SECTION", "SPAN"]);
  const selector = [
    "a[href]", "button", "input:not([type=hidden])", "textarea", "select",
    '[role="button"]', '[role="link"]', '[role="textbox"]',
    '[role="checkbox"]', '[role="radio"]', '[role="combobox"]',
    "[tabindex]",
  ].join(",");
  const limitations = [
    "Main document, open shadow roots and accessible same-origin iframe documents only; inaccessible frames, closed shadow-root and canvas contents are not inspected.",
    "Text is normalized in DOM/open-shadow traversal order; slot order, generated CSS text and exact innerText line breaks are not reproduced.",
    "Inspection is limited to 10000 node visits, 64 open shadow roots and depth 64, including label text scans.",
    "Iframe metadata is limited to 16 slots and iframe nesting to depth 8; unavailable frames also consume a slot. Frame ids are valid for this read only.",
    "Viewport and scroll describe the main window only; rendered DOM is not a pixel or occlusion match for a screenshot.",
    "focusedRef is a sampled logical DOM focus target from this read, not OS/window activation or an atomic snapshot. It is null outside returned refs or accessible traversal, and for shadow-capable hosts without an open root because a closed root may conceal the actual target.",
    "Native select options are limited to 40 per element and 240 per read; single-select actions inspect at most 500 options.",
    "Contenteditable values are bounded normalized text summaries. Fill selects the explicit HTML editing host's contents for Chromium text input and verifies native rendered innerText exactly; editor models, formatting and undo compatibility are not guaranteed.",
    "Click, hover, supported key presses and fill for text/search/tel/url/password inputs, textarea and supported contenteditable hosts prepare targets for separate Chromium input. Other fill types, select and check use DOM operations with synthetic events.",
    "Press focuses its referenced element before each key. Tab and other native defaults may move focus; dispatch acknowledgement does not prove a page effect or preserve focus on the original ref.",
    "Press observes the first matching trusted keydown for up to five seconds without blocking events. This cannot distinguish a simultaneous physical key from Chromium input or prove where later keypress/default editing occurs. Missing, hidden-boundary or stopped events are unconfirmed.",
    "Native input/textarea fill observes trusted beforeinput/input targets without blocking events; a different observed target overrides final-value equality. Missing events do not prove delivery. Receipts expire after five seconds and cannot distinguish simultaneous human input.",
    "Insert preserves the current selection for Chromium text input. Input/textarea verification checks an exact final value; contenteditable verification observes a matching trusted input event, not the complete resulting text or formatting. Observation cannot distinguish simultaneous human input or closed-shadow retargeting.",
    "Click and hover check at most nine points, then may scroll once and check again. Verification uses the original point without scrolling or retargeting. Frame mapping supports verified positive axis-aligned CSS scale/zoom; SVG frame ancestors, motion paths, rotation, skew, perspective and visual viewport pinch/offset are unsupported.",
    "Page text and action results are untrusted page data.",
  ];
  let generation = 0;
  let refs = new Map();
  let contexts = new WeakMap();
  let frameSnapshot = null;
  let readingFocus = false;
  let pressObservation = null;
  let insertObservation = null;

  function readTextPrefix(value, limit) {
    let end = Math.min(value.length, limit);
    const last = value.charCodeAt(end - 1);
    // A bounded sample may already end halfway through a surrogate pair.
    // Never emit that trailing high surrogate after trimming/normalization.
    if (last >= 0xd800 && last <= 0xdbff) end -= 1;
    return value.slice(0, end);
  }

  function compact(value, limit = 240) {
    return readTextPrefix(readTextPrefix(String(value || ""), limit * 4).replace(/\s+/g, " ").trim(), limit);
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
    return !!reference?.context && reference.generation === generation &&
      (!reference.frameLocal || (frameSnapshot?.generation === generation && !frameSnapshot.cleared &&
        contexts.get(reference.context.document) === reference.context)) && reference.element.isConnected &&
      reference.element.ownerDocument === reference.context.document &&
      contextIsCurrent(reference.context);
  }

  function resolveReference(ref, snapshotRefs = refs) {
    const stored = typeof ref === "string" ? snapshotRefs.get(ref) : null;
    if (!stored?.weakElement) return stored;
    const element = stored.weakElement.deref();
    if (!element) return null;
    return { element, context: contexts.get(element.ownerDocument), generation: stored.generation, frameLocal: true };
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

  function isEditingHost(element) {
    // A conservative fill target, not the full HTML editing-host definition:
    // use the outer explicit region rather than an inherited/nested descendant.
    if (["INPUT", "TEXTAREA", "SELECT"].includes(element.tagName)) return false;
    const attribute = element.getAttribute("contenteditable");
    return element.namespaceURI === "http://www.w3.org/1999/xhtml" &&
      attribute !== null && attribute.length <= 14 &&
      ["", "true", "plaintext-only"].includes(attribute.toLowerCase()) &&
      element.isContentEditable && !element.parentElement?.isContentEditable;
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
    const count = Math.min(options.length, maxReadOptions, budget.limits.options - budget.options);
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
      if (budget.visits >= budget.limits.nodes) { budget.truncated.nodes = true; return; }
      frame.next = frame.single ? null : node.nextSibling;
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
          if (budget.rootCount >= budget.limits.roots) { budget.truncated.shadowRoots = true; continue; }
          budget.roots.add(shadow);
          budget.rootCount += 1;
        }
        stack.push({ next: shadow, depth: frame.depth + 1, single: true });
      }
    }
  }

  function inspectFrame(element, budget) {
    if (budget.frames.length >= budget.limits.frames) {
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
    if (budget.localFrame) {
      // Describe the embedding element without reading its document, even when
      // same-origin access would be available. The native caller owns routing.
      entry.status = "not-inspected";
      return null;
    }
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
      const url = readTextPrefix(String(child.URL), 4096);
      const title = readTextPrefix(String(child.title), 512);
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
        if (limit === 0) { truncated = true; return; }
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
        const sample = readTextPrefix(node.substringData(0, remaining + 1), remaining + 1);
        const value = sample.replace(/\s+/g, " ").trim();
        if (node.length > remaining || value.length > remaining) truncated = true;
        const part = readTextPrefix(value, remaining);
        if (part) text += (text ? " " : "") + part;
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
    return readDocument(false);
  }

  function readFrame() {
    try {
      return readDocument(true);
    } finally {
      // Frame-local content does not expose actionable references. Release its
      // nodes and document contexts even when a page getter interrupts reading.
      refs = new Map();
      contexts = new WeakMap();
    }
  }

  function releaseFrameReferences() {
    clearPressObservation();
    clearInsertObservation();
    refs = new Map();
    contexts = new WeakMap();
  }

  function frameReadLimits(value) {
    if (value === undefined) return { ...defaultReadLimits };
    if (!value || typeof value !== "object" || Array.isArray(value) ||
        Object.getPrototypeOf(value) !== Object.prototype) return null;
    const limits = {};
    let count = 0;
    for (const name in value) {
      if (++count > 6 || !Object.prototype.hasOwnProperty.call(defaultReadLimits, name)) return null;
    }
    if (count !== 6) return null;
    for (const name in defaultReadLimits) {
      if (!Object.prototype.hasOwnProperty.call(value, name)) return null;
      const limit = value[name];
      if (!Number.isInteger(limit) || limit < 0 || limit > defaultReadLimits[name]) return null;
      limits[name] = limit;
    }
    return limits;
  }

  function readFrameActions(requestedLimits) {
    const previousGeneration = generation;
    const expectedGeneration = previousGeneration + 1;
    const reject = (error, detail) => {
      // Invalid reads expire the old binding, but never erase a newer read
      // initiated by a synchronous getter while validating the limits.
      if (generation === previousGeneration) {
        frameSnapshot = null;
        generation = expectedGeneration;
        releaseFrameReferences();
      }
      return failure("readFrameActions", error, detail);
    };
    let limits;
    try { limits = frameReadLimits(requestedLimits); } catch (_error) { limits = null; }
    if (!limits) return reject("INVALID_BUDGET", "Provide all six read limits as integers from zero to their default maximum, or omit the limits.");
    if (generation !== previousGeneration) {
      return failure("readFrameActions", "FRAME_READ_FAILED", "The frame snapshot changed while validating its read limits.");
    }
    if (typeof WeakRef !== "function") {
      return reject("WEAK_REFS_UNAVAILABLE", "Frame actions require weak element references.");
    }
    try {
      return readDocument(true, true, limits);
    } catch (_error) {
      // Do not erase a newer read that a synchronous getter may have started.
      if (generation === expectedGeneration) {
        frameSnapshot = null;
        releaseFrameReferences();
      }
      return failure("readFrameActions", "FRAME_READ_FAILED", "The frame snapshot changed or could not be read.");
    }
  }

  function frameBindingMatches(binding) {
    return !!binding && typeof binding === "object" && !Array.isArray(binding) &&
      binding.documentId === documentId && Number.isSafeInteger(binding.generation) &&
      binding.generation === generation && frameSnapshot?.generation === generation;
  }

  function clearFrame(binding) {
    if (!frameBindingMatches(binding)) {
      return failure("clearFrame", "STALE_FRAME_SNAPSHOT", "The frame snapshot binding is no longer current.");
    }
    frameSnapshot.cleared = true;
    releaseFrameReferences();
    // Keep only the primitive binding so repeated cleanup can acknowledge the
    // same generation, without allowing old cleanup to erase a newer snapshot.
    return { ok: true, action: "clearFrame", documentId, generation, cleared: true };
  }

  function frameAct(request, binding) {
    // Check the binding before act can consume or clear input observations.
    if (!frameBindingMatches(binding) || !frameSnapshot.ready || frameSnapshot.cleared) {
      return { ...failure(request?.action ?? null, "STALE_FRAME_SNAPSHOT", "Read the frame again to get a current snapshot binding."), attempted: false };
    }
    if (request?.action === "click" || request?.action === "hover") {
      clearPressObservation();
      clearInsertObservation();
      try {
        return prepareFramePointer(resolveReference(request.ref), request);
      } catch (error) {
        return { ...failure(request.action, error === unsupportedGeometry ? "UNSUPPORTED_GEOMETRY" : "ACTION_FAILED",
          "The frame pointer target could not be prepared safely."), attempted: false };
      }
    }
    return performAction(request);
  }

  function inspectFrameOwner(element) {
    const fail = () => ({ ...failure("inspectFrameOwner", "FRAME_OWNER_UNAVAILABLE", "The frame owner could not be inspected."), visible: false, disabled: false });
    const savedContexts = contexts;
    const savedRefs = refs;
    const savedGeneration = generation;
    const savedSnapshot = frameSnapshot;
    const inspectionContexts = new WeakMap();
    const validOwner = () => element && typeof element === "object" && element.nodeType === 1 &&
      element.namespaceURI === "http://www.w3.org/1999/xhtml" &&
      ["IFRAME", "FRAME"].includes(element.tagName) && element.isConnected && element.ownerDocument === document;
    try {
      if (!validOwner()) return fail();
      inspectionContexts.set(document, { id: "main", document, view: window, parent: null, depth: 0 });
      contexts = inspectionContexts;
      // Inspect this parent document's composed ancestors only. This is not a
      // pointer hit test and never accesses the frame's contentDocument.
      const visible = isRendered(element);
      const disabled = isDisabled(element);
      if (!validOwner() || contexts !== inspectionContexts || refs !== savedRefs ||
          generation !== savedGeneration || frameSnapshot !== savedSnapshot) return fail();
      return { ok: true, visible, disabled };
    } catch (_error) {
      return fail();
    } finally {
      // A reentrant newer read owns its new contexts; never replace them with
      // the old snapshot while restoring this temporary inspection scope.
      if (contexts === inspectionContexts) contexts = savedContexts;
    }
  }

  function inspectFramePointer(element, request) {
    const action = request?.action;
    const fail = (error, detail) => ({ ...failure(action ?? null, error, detail), attempted: false });
    const savedContexts = contexts;
    const savedRefs = refs;
    const savedGeneration = generation;
    const savedSnapshot = frameSnapshot;
    const inspectionContexts = new WeakMap();
    const validOwner = () => element && typeof element === "object" && element.nodeType === 1 &&
      element.namespaceURI === "http://www.w3.org/1999/xhtml" &&
      ["IFRAME", "FRAME"].includes(element.tagName) && element.isConnected && element.ownerDocument === document;
    const current = () => contexts === inspectionContexts && refs === savedRefs &&
      generation === savedGeneration && frameSnapshot === savedSnapshot;
    try {
      if (!request || typeof request !== "object" || Array.isArray(request) ||
          !["click", "hover"].includes(action) || !["geometry", "points"].includes(request.phase)) {
        return fail("INVALID_REQUEST", "Expected a frame pointer geometry or points request.");
      }
      if (request.phase === "points" && !validPointerCandidates(request.points)) {
        return fail("INVALID_POINT", "Use at most nine finite points with distinct indexes from zero to eight.");
      }
      if (!validOwner()) return fail("FRAME_OWNER_UNAVAILABLE", "The frame owner is unavailable.");
      const context = { id: "main", document, view: window, parent: null, depth: 0 };
      inspectionContexts.set(document, context);
      contexts = inspectionContexts;
      if (!isRendered(element) || isDisabled(element) || !validOwner() || !current()) {
        return fail("FRAME_OWNER_UNAVAILABLE", "The frame owner is hidden, disabled or changed.");
      }
      // Mapping uses only this parent's owner element and CSS geometry. Never
      // access the child document or retain the remote owner after this call.
      const mapping = frameMapping({ element, parent: context });
      if (![mapping.x, mapping.y].every(value => boundedPointerNumber(value) && value > 0) ||
          !validPointerRect(mapping.content)) throw unsupportedGeometry;
      const viewport = pointerRect(clickViewport(context));
      const geometry = { mapping, viewport };
      const candidates = [];
      if (request.phase === "points") {
        for (const point of request.points) {
          const mapped = { index: point.index, x: mapping.content.left + point.x * mapping.x,
            y: mapping.content.top + point.y * mapping.y };
          if (validClickPoint(mapped) && inside(mapped, viewport) && inside(mapped, mapping.content) &&
              deepHit(context, mapped, { left: maxDepth }) === element) candidates.push(mapped);
        }
      }
      if (!validOwner() || !isRendered(element) || isDisabled(element) || !current()) {
        return fail("FRAME_OWNER_UNAVAILABLE", "The frame owner changed during pointer inspection.");
      }
      return { ok: true, action, prepared: true, attempted: false, geometry,
        ...(request.phase === "points" ? { candidates } : {}) };
    } catch (error) {
      return fail(error === unsupportedGeometry ? "UNSUPPORTED_GEOMETRY" : "FRAME_OWNER_UNAVAILABLE",
        "The frame owner's pointer geometry could not be inspected safely.");
    } finally {
      if (contexts === inspectionContexts) contexts = savedContexts;
    }
  }

  function readDocument(localFrame, retainFrameRefs = false, limits = defaultReadLimits) {
    frameSnapshot = null;
    clearPressObservation();
    clearInsertObservation();
    generation += 1;
    const readGeneration = generation;
    const snapshot = retainFrameRefs ? { generation: readGeneration, ready: false, cleared: false } : null;
    frameSnapshot = snapshot;
    refs = new Map();
    contexts = new WeakMap();
    contexts.set(document, { id: "main", document, view: window, parent: null, depth: 0 });
    const interactive = [];
    const elements = [];
    const budget = { localFrame, limits, visits: 0, rootCount: 0, options: 0, roots: new WeakSet(), states: new WeakMap(), styles: new WeakMap(),
      ranges: new WeakMap(), frames: [], truncated: { nodes: false, shadowRoots: false, depth: false, frames: false, frameDepth: false } };
    const text = textCollector(limits.text, budget);
    let interactiveTruncated = false;
    for (const node of walk(document.body, budget, true)) {
      text.add(node);
      if (node.nodeType !== 1 || (!node.matches(selector) && !isEditingHost(node)) || !isRendered(node, budget)) continue;
      if (elements.length < limits.elements) elements.push(node);
      else interactiveTruncated = true;
    }
    for (const element of elements) {
      const ref = `${documentId}:${generation}:${interactive.length + 1}`;
      const context = contexts.get(element.ownerDocument);
      const reference = { element, context, generation };
      if (!refIsCurrent(reference)) continue;
      refs.set(ref, retainFrameRefs ? { weakElement: new WeakRef(element), generation: readGeneration } : reference);
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
      if (isEditingHost(element)) item.value = textOf(element, budget);
      interactive.push(item);
    }
    const bodyText = text.result();
    const readLimitations = limitations.slice();
    if (localFrame) {
      readLimitations[0] = "Current frame document and its open shadow roots only; child iframe documents, parent/sibling documents, closed shadow-root and canvas contents are not inspected.";
      readLimitations[2] = `This call permits ${limits.nodes} node visits and ${limits.roots} open shadow roots, including label text scans, with traversal depth limited to 64.`;
      readLimitations[3] = `Iframe element metadata is limited to ${limits.frames} slots and marked not-inspected without accessing contentDocument. Frame ids and parent main identify this local snapshot only; iframe nesting is not traversed.`;
      readLimitations[4] = "Viewport and scroll describe the current frame window only. Parent iframe visibility, clipping and occlusion are not checked; this local snapshot does not establish that a target is visible from the top window.";
      readLimitations[5] = "focusedRef is a sampled logical DOM focus target among this frame's returned refs, not OS/window activation or an atomic snapshot. It is null inside child iframes, outside returned refs, or for shadow-capable hosts without an open root because a closed root may conceal the actual target.";
      readLimitations[6] = `Native select options are limited to 40 per element and ${limits.options} for this call; single-select actions inspect at most 500 options.`;
      readLimitations.push(`This local-frame call permits ${limits.text} normalized body-text UTF-16 code units and ${limits.elements} interactive elements. Label, value, URL and title limits are separate from the body-text budget. Limits may be reduced from their defaults for a caller-managed aggregate read.`);
      if (retainFrameRefs) readLimitations.push("budgetUsed counts returned body-text UTF-16 code units, interactive elements, option entries and iframe metadata slots; nodes counts walk visits including repeated label scans, and roots counts distinct open shadow roots entered by those scans. Ancestor/style/focus checks are bounded separately and are not node visits. The caller must validate and debit usage before reading another frame.");
      readLimitations.push(retainFrameRefs
        ? "Local-frame action refs retain only weak element references and require the exact documentId/generation binding. They expire on the next read or explicit cleanup. Pointer preparation returns local geometry only; the caller must validate parent mappings and hit tests, then recheck the exact point before engine input."
        : "Local-frame refs and focusedRef are diagnostic only: their node/document bindings are released before return and cannot be used for actions. The native caller removes these refs from the public content-only result.");
    }
    const result = {
      ...(localFrame ? { scope: "local-frame" } : {}),
      url: readTextPrefix(String(location.href), 4096),
      title: readTextPrefix(String(document.title), 512),
      documentId,
      readyState: document.readyState,
      viewport: { x: window.scrollX, y: window.scrollY, width: window.innerWidth, height: window.innerHeight },
      generation,
      text: bodyText.text,
      interactive,
      focusedRef: readFocusedRef(interactive, budget, readGeneration),
      frames: budget.frames,
      truncated: { text: bodyText.truncated, interactive: interactiveTruncated, ...budget.truncated },
      limitations: readLimitations,
      ...(retainFrameRefs ? {
        budgetLimit: { ...limits },
        budgetUsed: { text: bodyText.text.length, elements: interactive.length, nodes: budget.visits,
          roots: budget.rootCount, options: budget.options, frames: budget.frames.length },
      } : {}),
    };
    if (retainFrameRefs) {
      if (generation !== readGeneration || frameSnapshot !== snapshot || snapshot.cleared) throw new Error();
      snapshot.ready = true;
    }
    return result;
  }

  function deepestReadFocus(budget) {
    let context = contexts.get(document);
    let root = document;
    for (let depth = 0; context && depth < maxDepth; depth += 1) {
      const element = root.activeElement;
      if (!element || element.nodeType !== 1 || !element.isConnected ||
          element.ownerDocument !== context.document || element.getRootNode() !== root ||
          !contextIsCurrent(context)) return null;
      if (element.tagName === "IFRAME") {
        if (budget.localFrame) return null;
        const child = element.contentDocument;
        const childContext = child && contexts.get(child);
        if (!childContext || childContext.parent !== context || childContext.element !== element ||
            childContext.depth > maxFrameDepth || !contextIsCurrent(childContext)) return null;
        context = childContext;
        root = child;
        continue;
      }
      const shadow = element.shadowRoot;
      if (shadow) {
        if (!budget.roots.has(shadow) || shadow.mode !== "open" || shadow.host !== element) return null;
        if (shadow.activeElement) {
          root = shadow;
          continue;
        }
      } else if (element.namespaceURI === "http://www.w3.org/1999/xhtml" &&
          (shadowHostTags.has(element.tagName) || element.tagName.includes("-"))) {
        // Public DOM focus retargets closed-shadow descendants to their host.
        // A missing open root cannot distinguish that case from direct focus.
        return null;
      }
      if (["OBJECT", "EMBED"].includes(element.tagName)) return null;
      return element;
    }
    return null;
  }

  function readFocusedRef(interactive, budget, readGeneration) {
    // Reuse only this read's bounded traversal. Logical DOM focus says nothing
    // about OS activation, and two matching samples are not an atomic snapshot.
    const readRefs = refs;
    if (readingFocus) return null;
    readingFocus = true;
    try {
      if (generation !== readGeneration) return null;
      const element = deepestReadFocus(budget);
      if (!element) return null;
      for (const item of interactive) {
        const reference = resolveReference(item.ref, readRefs);
        if (reference?.element !== element) continue;
        if (!refIsCurrent(reference) || deepestReadFocus(budget) !== element ||
            !refIsCurrent(reference) || refs !== readRefs || generation !== readGeneration) return null;
        return item.ref;
      }
    } catch (_error) {
      // Focus and frame getters may fail or change the document; expose neither
      // their exception contents nor a retargeted outer host as the focused ref.
    } finally {
      readingFocus = false;
    }
    return null;
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

  function hasPressFocus(reference) {
    let current = reference.element;
    let context = reference.context;
    // Follow the active target out through each open shadow root and embedding
    // frame. This is logical DOM focus, not document.hasFocus() or OS activation.
    for (let depth = 0; current && depth < maxDepth; depth += 1) {
      if (current.ownerDocument !== context.document) return false;
      const root = current.getRootNode();
      if (root.activeElement !== current) return false;
      if (root === context.document) {
        if (!context.parent) return root === document;
        current = context.element;
        context = context.parent;
      } else {
        if (root.nodeType !== 11 || root.mode !== "open" ||
            !root.host || root.host.shadowRoot !== root) return false;
        current = root.host;
      }
    }
    return false;
  }

  function composedParent(node) {
    return node.assignedSlot || node.parentNode || node.host;
  }

  function clickRect(element) {
    const rect = element.getBoundingClientRect();
    if (![rect.left, rect.top, rect.right, rect.bottom, rect.width, rect.height].every(Number.isFinite)) {
      throw unsupportedGeometry;
    }
    return rect;
  }

  function clickViewport(context) {
    const view = context.view;
    const visual = view.visualViewport;
    if (visual && (visual.scale !== 1 || visual.offsetLeft !== 0 || visual.offsetTop !== 0)) {
      throw unsupportedGeometry;
    }
    const root = context.document.compatMode === "BackCompat"
      ? context.document.body : context.document.documentElement;
    const width = Math.min(view.innerWidth, root?.clientWidth);
    const height = Math.min(view.innerHeight, root?.clientHeight);
    if (![width, height].every((value) => Number.isFinite(value) && value > 0 && value <= maxPointerCoordinate)) {
      throw unsupportedGeometry;
    }
    return { left: 0, top: 0, right: width, bottom: height };
  }

  function cssPixels(value) {
    if (typeof value !== "string" || value.length > 64 || !/^(?:\d+(?:\.\d*)?|\.\d+)px$/.test(value)) {
      throw unsupportedGeometry;
    }
    const number = Number(value.slice(0, -2));
    if (!Number.isFinite(number)) throw unsupportedGeometry;
    return number;
  }

  function frameScale(context) {
    let x = 1, y = 1;
    let node = context.element;
    for (let depth = 0; node && depth < maxDepth; depth += 1) {
      if (node === context.parent.document) return { x, y };
      if (node.nodeType === 1) {
        // SVG viewBox/foreignObject mappings are not CSS ancestor matrices.
        if (node.namespaceURI !== "http://www.w3.org/1999/xhtml") throw unsupportedGeometry;
        const style = context.parent.view.getComputedStyle(node);
        if (style.offsetPath && style.offsetPath !== "none") throw unsupportedGeometry;
        const zoomText = style.zoom;
        const zoom = zoomText === "normal" ? 1 : typeof zoomText === "string" && zoomText.length <= 64
          ? (zoomText.endsWith("%") ? Number(zoomText.slice(0, -1)) / 100 : Number(zoomText)) : NaN;
        if (!(zoom > 0 && Number.isFinite(zoom))) throw unsupportedGeometry;
        x *= zoom; y *= zoom;
        // display:contents has no box to transform. Its CSS zoom still applies.
        if (style.display !== "contents") {
          if (style.rotate !== "none" || style.perspective !== "none") throw unsupportedGeometry;
          if (style.transform !== "none") {
            const match = typeof style.transform === "string" && style.transform.length <= 256 &&
              /^matrix\(([^)]+)\)$/.exec(style.transform);
            const matrix = match ? match[1].split(",").map(Number) : [];
            if (matrix.length !== 6 || !matrix.every(Number.isFinite) || matrix[1] !== 0 || matrix[2] !== 0 ||
                !(matrix[0] > 0 && matrix[3] > 0)) throw unsupportedGeometry;
            x *= matrix[0]; y *= matrix[3];
          }
          if (style.scale !== "none") {
            const scale = typeof style.scale === "string" && style.scale.length <= 128
              ? style.scale.trim().split(/\s+/).map(Number) : [];
            if (scale.length < 1 || scale.length > 2 || !scale.every((value) => Number.isFinite(value) && value > 0)) {
              throw unsupportedGeometry;
            }
            x *= scale[0]; y *= scale[1] ?? scale[0];
          }
        }
        if (!(x > 0 && y > 0 && Number.isFinite(x) && Number.isFinite(y))) throw unsupportedGeometry;
      }
      node = composedParent(node);
    }
    throw unsupportedGeometry;
  }

  function frameMapping(context) {
    const element = context.element;
    const scale = frameScale(context);
    const rect = clickRect(element);
    const style = context.parent.view.getComputedStyle(element);
    const left = cssPixels(style.borderLeftWidth) + cssPixels(style.paddingLeft);
    const right = cssPixels(style.borderRightWidth) + cssPixels(style.paddingRight);
    const top = cssPixels(style.borderTopWidth) + cssPixels(style.paddingTop);
    const bottom = cssPixels(style.borderBottomWidth) + cssPixels(style.paddingBottom);
    const borderBox = style.boxSizing === "border-box";
    if (!borderBox && style.boxSizing !== "content-box") throw unsupportedGeometry;
    const width = cssPixels(style.width) + (borderBox ? 0 : left + right);
    const height = cssPixels(style.height) + (borderBox ? 0 : top + bottom);
    // Reject unmodelled mappings. A small CSS-pixel tolerance only accommodates
    // used-size/zoom quantization and CSSOM number serialization, not rotations.
    if (Math.abs(width * scale.x - rect.width) > 0.02 || Math.abs(height * scale.y - rect.height) > 0.02) {
      throw unsupportedGeometry;
    }
    const content = { left: rect.left + left * scale.x, top: rect.top + top * scale.y,
      right: rect.right - right * scale.x, bottom: rect.bottom - bottom * scale.y };
    if (!(content.left < content.right && content.top < content.bottom)) throw unsupportedGeometry;
    return { x: scale.x, y: scale.y, content };
  }

  function clickGeometry(reference) {
    const chain = [];
    let context = reference.context;
    for (let depth = 0; context && depth <= maxFrameDepth; depth += 1) {
      chain.push({ context, viewport: clickViewport(context), mapping: context.parent ? frameMapping(context) : null });
      if (!context.parent) {
        if (context.document !== document) throw unsupportedGeometry;
        return { chain, rect: clickRect(reference.element) };
      }
      context = context.parent;
    }
    throw unsupportedGeometry;
  }

  function inside(point, rect) {
    return point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom;
  }

  function deepHit(context, point, budget) {
    let root = context.document;
    let previous = null;
    while (budget.left > 0) {
      budget.left -= 1;
      const hit = root.elementFromPoint(point.x, point.y);
      if (!hit || hit === previous) return hit;
      if (!hit.shadowRoot) return hit;
      previous = hit;
      root = hit.shadowRoot;
    }
    throw unsupportedGeometry;
  }

  function clickDescendant(hit, target) {
    for (let depth = 0; hit && depth < maxDepth; depth += 1) {
      if (hit === target) return true;
      // Clicking a button's text/icon is valid; a nested link or separate input
      // is a different activation target, even if its click would bubble.
      if (hit.nodeType === 1 && (hit.matches(selector) || isEditingHost(hit))) return false;
      hit = composedParent(hit);
    }
    return false;
  }

  function clickPointMatches(reference, geometry, point) {
    const budget = { left: maxDepth };
    let local = point;
    for (let index = geometry.chain.length - 1; index >= 0; index -= 1) {
      const entry = geometry.chain[index];
      if (!inside(local, entry.viewport)) return false;
      const hit = deepHit(entry.context, local, budget);
      if (index === 0) return inside(local, geometry.rect) && clickDescendant(hit, reference.element);
      const child = geometry.chain[index - 1];
      if (hit !== child.context.element || !inside(local, child.mapping.content)) return false;
      local = { x: (local.x - child.mapping.content.left) / child.mapping.x,
        y: (local.y - child.mapping.content.top) / child.mapping.y };
    }
    return false;
  }

  function findClickPoint(reference, geometry) {
    const clip = { left: geometry.rect.left, top: geometry.rect.top,
      right: geometry.rect.right, bottom: geometry.rect.bottom };
    let x = 1, y = 1, left = 0, top = 0;
    const intersect = (rect) => {
      clip.left = Math.max(clip.left, (rect.left - left) / x);
      clip.top = Math.max(clip.top, (rect.top - top) / y);
      clip.right = Math.min(clip.right, (rect.right - left) / x);
      clip.bottom = Math.min(clip.bottom, (rect.bottom - top) / y);
    };
    for (const entry of geometry.chain) {
      intersect(entry.viewport);
      if (entry.mapping) {
        left = entry.mapping.content.left + left * entry.mapping.x;
        top = entry.mapping.content.top + top * entry.mapping.y;
        x *= entry.mapping.x; y *= entry.mapping.y;
        intersect(entry.mapping.content);
      }
    }
    if (!(clip.left < clip.right && clip.top < clip.bottom)) return null;
    // Center first, then a fixed 3x3 grid. Never enumerate fragment rectangles,
    // descendants or every point of a partially occluded element.
    for (const [fx, fy] of pointerFractions) {
      const point = { x: left + (clip.left + (clip.right - clip.left) * fx) * x,
        y: top + (clip.top + (clip.bottom - clip.top) * fy) * y };
      if (validClickPoint(point) && clickPointMatches(reference, geometry, point)) return point;
    }
    return null;
  }

  function validClickPoint(point) {
    return !!point && [point.x, point.y].every((value) => typeof value === "number" &&
      Number.isFinite(value) && value >= 0 && value <= maxPointerCoordinate);
  }

  function boundedPointerNumber(value) {
    return typeof value === "number" && Number.isFinite(value) && Math.abs(value) <= maxPointerCoordinate;
  }

  function validPointerRect(rect) {
    return !!rect && typeof rect === "object" && !Array.isArray(rect) &&
      [rect.left, rect.top, rect.right, rect.bottom].every(boundedPointerNumber) &&
      rect.left <= rect.right && rect.top <= rect.bottom;
  }

  function pointerRect(rect) {
    if (!validPointerRect(rect)) throw unsupportedGeometry;
    return { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom };
  }

  function validPointerCandidates(points) {
    if (!Array.isArray(points) || points.length > pointerFractions.length) return false;
    const indexes = new Set();
    for (const point of points) {
      if (!point || typeof point !== "object" || Array.isArray(point) ||
          !Number.isInteger(point.index) || point.index < 0 || point.index >= pointerFractions.length ||
          indexes.has(point.index) || !boundedPointerNumber(point.x) || !boundedPointerNumber(point.y)) return false;
      indexes.add(point.index);
    }
    return true;
  }

  function prepareFramePointer(reference, request) {
    const action = request.action;
    const fail = (error, detail) => ({ ...failure(action, error, detail), attempted: false });
    if (request.verifyOnly === true) return preparePointer(reference, request);
    if (!["geometry", "candidates"].includes(request.pointerPhase) ||
        (request.scroll !== undefined && typeof request.scroll !== "boolean") ||
        (request.scroll === true && request.pointerPhase !== "geometry")) {
      return fail("INVALID_REQUEST", "Use frame pointer geometry, candidates or exact-point verification.");
    }
    if (request.pointerPhase === "candidates" && !validPointerRect(request.clip)) {
      return fail("INVALID_POINT", "Provide a finite ordered local clip rectangle within the supported bounds.");
    }
    const unavailable = refFailure(reference, action);
    if (unavailable) return { ...unavailable, attempted: false };
    const changed = () => refFailure(reference, action, "The frame pointer target changed; read the frame again.", "ELEMENT_UNAVAILABLE");
    if (request.scroll === true) {
      reference.element.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
      const invalid = changed();
      if (invalid) return { ...invalid, attempted: false };
    }
    const geometry = clickGeometry(reference);
    // A frame snapshot contains only its own document and open shadow roots.
    // Parent transforms and hit tests are supplied by separate verified worlds.
    if (geometry.chain.length !== 1) throw unsupportedGeometry;
    const rect = pointerRect(geometry.rect);
    const viewport = pointerRect(geometry.chain[0].viewport);
    const candidates = [];
    if (request.pointerPhase === "candidates") {
      const clip = { left: Math.max(rect.left, viewport.left, request.clip.left),
        top: Math.max(rect.top, viewport.top, request.clip.top),
        right: Math.min(rect.right, viewport.right, request.clip.right),
        bottom: Math.min(rect.bottom, viewport.bottom, request.clip.bottom) };
      if (clip.left < clip.right && clip.top < clip.bottom) {
        for (let index = 0; index < pointerFractions.length; index += 1) {
          const [fx, fy] = pointerFractions[index];
          const point = { index, x: clip.left + (clip.right - clip.left) * fx,
            y: clip.top + (clip.bottom - clip.top) * fy };
          if (validClickPoint(point) && clickPointMatches(reference, geometry, point)) candidates.push(point);
        }
      }
    }
    const invalid = changed();
    if (invalid) return { ...invalid, attempted: false };
    return { ok: true, action, prepared: true, attempted: false,
      ...(request.pointerPhase === "geometry" ? { rect, viewport } : { candidates }) };
  }

  function preparePointer(reference, request) {
    const action = request.action;
    const fail = (error, detail) => ({ ...failure(action, error, detail), attempted: false });
    const changed = () => refFailure(reference, action, "The element changed while preparing a pointer target; read again.", "ELEMENT_UNAVAILABLE");
    const unavailable = refFailure(reference, action);
    if (unavailable) return { ...unavailable, attempted: false };
    const verify = request.verifyOnly === true;
    if (verify && !validClickPoint(request.point)) return fail("INVALID_POINT", "Provide a finite viewport point within the supported bounds.");
    try {
      let geometry = clickGeometry(reference);
      let point = verify ? (clickPointMatches(reference, geometry, request.point) ? request.point : null)
        : findClickPoint(reference, geometry);
      let invalid = changed();
      if (invalid) return { ...invalid, attempted: false };
      if (!point && !verify) {
        reference.element.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
        invalid = changed();
        if (invalid) return { ...invalid, attempted: false };
        for (let context = reference.context; context.parent; context = context.parent) {
          context.element.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
          invalid = changed();
          if (invalid) return { ...invalid, attempted: false };
        }
        geometry = clickGeometry(reference);
        point = findClickPoint(reference, geometry);
        invalid = changed();
        if (invalid) return { ...invalid, attempted: false };
      }
      if (!point) return fail("CLICK_POINT_UNAVAILABLE", "No supported hit-tested point reached this element. Inspect the page before retrying.");
      return { ok: true, action, prepared: true, attempted: false, point: { x: point.x, y: point.y } };
    } catch (error) {
      if (error === unsupportedGeometry) return fail("UNSUPPORTED_GEOMETRY", "The element's viewport or frame geometry is not supported safely.");
      throw error;
    }
  }

  const supportedPressKeys = new Set([
    "Enter", "Tab", "Escape", "ArrowLeft", "ArrowUp", "ArrowRight", "ArrowDown",
    "Home", "End", "PageUp", "PageDown", "Backspace", "Delete", "Space",
    "Shift+Enter", "Shift+Tab", "Shift+ArrowLeft", "Shift+ArrowUp", "Shift+ArrowRight", "Shift+ArrowDown",
    "Shift+Home", "Shift+End", "Shift+PageUp", "Shift+PageDown",
    "Meta+A", "Meta+Z", "Meta+Shift+Z",
    "Meta+ArrowLeft", "Meta+ArrowUp", "Meta+ArrowRight", "Meta+ArrowDown",
    "Meta+Shift+ArrowLeft", "Meta+Shift+ArrowUp", "Meta+Shift+ArrowRight", "Meta+Shift+ArrowDown",
    "Meta+Backspace", "Alt+ArrowLeft", "Alt+ArrowRight",
    "Alt+Shift+ArrowLeft", "Alt+Shift+ArrowRight", "Alt+Backspace", "Alt+Delete",
  ]);

  function pressDescriptor(key) {
    if (typeof key !== "string" || key.length > 32 || !supportedPressKeys.has(key)) return null;
    // Mirror keyboard.rs canonical/dom_key/code; only the bounded allowlist is
    // parsed, so aliases, modifier reordering and extra parts remain rejected.
    const parts = key.split("+");
    const base = parts.pop();
    const shift = parts.includes("Shift"), alt = parts.includes("Alt"), meta = parts.includes("Meta");
    const code = base === "A" ? "KeyA" : base === "Z" ? "KeyZ" : base;
    const domKey = base === "Space" ? " " : base === "A" ? "a" : base === "Z" ? (shift ? "Z" : "z") : code;
    return { domKey, code, shift, alt, meta, ctrl: false };
  }

  function clearPressObservation(observation = pressObservation) {
    if (!observation) return;
    if (pressObservation === observation) pressObservation = null;
    if (observation.timer !== null) {
      try { window.clearTimeout(observation.timer); } catch (_error) { /* The old realm may already be gone. */ }
    }
    for (const [target, listener] of observation.listeners) {
      try { observation.remove.call(target, "keydown", listener, true); } catch (_error) { /* A destroyed realm is already unavailable. */ }
    }
    observation.listeners.length = 0;
  }

  function armPressObservation(reference, ref, key, descriptor) {
    const { element, context } = reference;
    const root = element.getRootNode();
    let documentTarget = element;
    let currentRoot = root;
    for (let depth = 0; currentRoot !== context.document && depth < maxDepth; depth += 1) {
      if (currentRoot.nodeType !== 11 || currentRoot.mode !== "open" || !currentRoot.host) throw new Error();
      documentTarget = currentRoot.host;
      currentRoot = documentTarget.getRootNode();
    }
    if (currentRoot !== context.document) throw new Error();
    const prototype = context.view.EventTarget.prototype;
    if (typeof prototype.addEventListener !== "function" || typeof prototype.removeEventListener !== "function") throw new Error();
    const token = Array.from(crypto.getRandomValues(new Uint32Array(4)), n => n.toString(16).padStart(8, "0")).join("");
    const { domKey, code, shift, alt, meta, ctrl } = descriptor;
    const observation = { token, ref, key, generation, listeners: [], remove: prototype.removeEventListener,
      timer: null, expires: performance.now() + 5000, seen: false, awaitingRoot: false, target: "unconfirmed" };
    pressObservation = observation;
    const relevant = event => {
      if (pressObservation !== observation) return false;
      if (performance.now() >= observation.expires) { clearPressObservation(observation); return false; }
      return event.isTrusted === true && event.key === domKey && event.code === code &&
        event.shiftKey === shift && event.altKey === alt && event.ctrlKey === ctrl && event.metaKey === meta;
    };
    const targetResult = target => {
      if (target !== element) return "mismatched";
      // At this root event.target is exact except when the ref itself hosts a
      // shadow tree. A closed root cannot be distinguished from direct focus.
      const shadow = element.shadowRoot;
      if (shadow?.activeElement || (!shadow && element.namespaceURI === "http://www.w3.org/1999/xhtml" &&
          (shadowHostTags.has(element.tagName) || element.tagName.includes("-")))) return "unconfirmed";
      return "matched";
    };
    const onDocument = event => {
      if (!relevant(event)) return;
      if (observation.seen) { observation.awaitingRoot = false; return; }
      observation.seen = true;
      if (root === context.document) observation.target = targetResult(event.target);
      else if (event.target !== documentTarget) observation.target = "mismatched";
      else observation.awaitingRoot = true;
    };
    const onRoot = event => {
      if (!relevant(event) || !observation.awaitingRoot) return;
      observation.awaitingRoot = false;
      observation.target = targetResult(event.target);
    };
    try {
      for (const [target, listener] of root === context.document
        ? [[context.document, onDocument]] : [[context.document, onDocument], [root, onRoot]]) {
        prototype.addEventListener.call(target, "keydown", listener, { capture: true, passive: true });
        observation.listeners.push([target, listener]);
      }
      observation.timer = window.setTimeout(() => clearPressObservation(observation), 5000);
      if (pressObservation !== observation || refFailure(reference, "press")) throw new Error();
      return token;
    } catch (error) {
      clearPressObservation(observation);
      throw error;
    }
  }

  function verifyPress(request) {
    const token = typeof request.observation === "string" && /^[a-f0-9]{1,64}$/.test(request.observation)
      ? request.observation : "";
    let keydownTarget = "unconfirmed";
    const observation = pressObservation;
    if (observation && observation.token === token && observation.ref === request.ref && observation.key === request.key) {
      if (observation.generation === generation && performance.now() < observation.expires) keydownTarget = observation.target;
      clearPressObservation(observation);
    }
    return { ok: true, action: "press", key: request.key, input: "chromium", attempted: true,
      observation: token, keydownTarget };
  }

  function preparePress(reference, key, ref) {
    const fail = (error, detail) => ({ ...failure("press", error, detail), attempted: false });
    const descriptor = pressDescriptor(key);
    if (!descriptor) return fail("UNSUPPORTED_KEY", "Use a supported canonical key or editing shortcut.");
    const unavailable = refFailure(reference, "press");
    if (unavailable) return { ...unavailable, attempted: false };
    if (typeof reference.element.focus !== "function") {
      return fail("UNSUPPORTED_ELEMENT", "This element cannot receive keyboard focus.");
    }
    reference.element.focus();
    const detail = "The element changed while preparing keyboard focus; read again.";
    const changed = refFailure(reference, "press", detail, "ELEMENT_UNAVAILABLE");
    if (changed) return { ...changed, attempted: false };
    const focused = hasPressFocus(reference);
    const afterFocus = refFailure(reference, "press", detail, "ELEMENT_UNAVAILABLE");
    if (afterFocus) return { ...afterFocus, attempted: false };
    if (!focused) return fail("FOCUS_NOT_REACHED", "The element did not retain keyboard focus; no key was dispatched.");
    // Focus handlers may have run, but no keyboard or submit event is sent by
    // this bridge. Later Chromium input is a separate, non-atomic operation.
    const observation = armPressObservation(reference, ref, key, descriptor);
    return { ok: true, action: "press", key, prepared: true, attempted: false, observation };
  }

  // Bound the structure before native innerText can allocate a rendered string.
  // Include hidden/script/style text conservatively; do not build node arrays.
  function editingStructureWithinBudget(element, textLimit, selectionRange = null, intersectsNode = null) {
    let node = element, depth = 0, visits = 0, textLength = 0;
    while (node) {
      if (++visits > maxNodes || depth > maxDepth) return false;
      if (node.nodeType === 1 && (node.shadowRoot || ["IFRAME", "FRAME"].includes(node.tagName))) return false;
      if (selectionRange && node !== element && node.nodeType === 1 && !node.isContentEditable &&
          intersectsNode.call(selectionRange, node)) return false;
      if (node.nodeType === 3 || node.nodeType === 4) {
        textLength += node.length;
        if (textLength > textLimit) return false;
      }
      if (node.firstChild) { node = node.firstChild; depth += 1; continue; }
      while (node !== element && !node.nextSibling) { node = node.parentNode; depth -= 1; }
      if (node === element) break;
      node = node.nextSibling;
    }
    return true;
  }

  function selectEditingHost(reference) {
    const { element, context } = reference;
    const range = context.document.createRange();
    const selection = context.view.getSelection();
    const rangePrototype = context.view.Range.prototype;
    const selectionPrototype = context.view.Selection.prototype;
    if (!selection || ![rangePrototype.selectNodeContents, selectionPrototype.removeAllRanges,
      selectionPrototype.addRange, selectionPrototype.getRangeAt].every(method => typeof method === "function")) return false;
    rangePrototype.selectNodeContents.call(range, element);
    selectionPrototype.removeAllRanges.call(selection);
    selectionPrototype.addRange.call(selection, range);
    if (selection.rangeCount !== 1) return false;
    const selected = selectionPrototype.getRangeAt.call(selection, 0);
    return selected.startContainer === element && selected.startOffset === 0 &&
      selected.endContainer === element && selected.endOffset === element.childNodes.length;
  }

  function chromiumFillKind(element) {
    if (element?.tagName === "TEXTAREA") return "textarea";
    if (element?.tagName === "INPUT" && ["text", "search", "tel", "url", "password"].includes(element.type)) return element.type;
    return element && isEditingHost(element) ? "contenteditable" : null;
  }

  function chromiumFillReadOnly(element, kind) {
    return kind === "contenteditable" ? element.getAttribute("aria-readonly") === "true" : element.readOnly;
  }

  function fillRoute(element) {
    const kind = chromiumFillKind(element);
    if (kind !== null) return { input: "chromium", kind };
    return supportsFill(element) ? { input: "dom", kind: element.type } : null;
  }

  function inspectFill(reference) {
    const fail = (error, detail) => ({ ...failure("fill", error, detail), attempted: false });
    const unavailable = refFailure(reference, "fill");
    if (unavailable) return { ...unavailable, attempted: false };
    const route = fillRoute(reference.element);
    if (!route) return fail("UNSUPPORTED_ELEMENT", "Fill supports editable input/textarea values and explicit HTML editing hosts only.");
    if (chromiumFillReadOnly(reference.element, route.kind)) return fail("READ_ONLY", "The element is read-only.");
    const after = refFailure(reference, "fill");
    const current = fillRoute(reference.element);
    if (after || !current || route.input !== current.input || route.kind !== current.kind) {
      return fail("ELEMENT_UNAVAILABLE", "The fill target changed during inspection; read again.");
    }
    return { ok: true, action: "fill", ...route };
  }

  function chromiumFillAccessors(reference, kind) {
    const prototype = kind === "textarea" ? reference.context.view.HTMLTextAreaElement.prototype
      : reference.context.view.HTMLInputElement.prototype;
    return { prototype, value: Object.getOwnPropertyDescriptor(prototype, "value")?.get,
      start: Object.getOwnPropertyDescriptor(prototype, "selectionStart")?.get,
      end: Object.getOwnPropertyDescriptor(prototype, "selectionEnd")?.get };
  }

  function insertionSnapshot(reference, kind) {
    const { element, context } = reference;
    if (kind !== "contenteditable") {
      const access = chromiumFillAccessors(reference, kind);
      const direction = Object.getOwnPropertyDescriptor(access.prototype, "selectionDirection")?.get;
      if (![access.value, access.start, access.end, direction].every(method => typeof method === "function")) return null;
      const value = access.value.call(element);
      const start = access.start.call(element), end = access.end.call(element);
      const selectedDirection = direction.call(element);
      if (typeof value !== "string" || value.length > maxEditingText ||
          !Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end < start || end > value.length ||
          !["forward", "backward", "none"].includes(selectedDirection)) return null;
      return { value, start, end, direction: selectedDirection };
    }
    const selection = context.view.getSelection();
    const selectionPrototype = context.view.Selection.prototype;
    const rangePrototype = context.view.Range.prototype;
    const selectionValue = name => Object.getOwnPropertyDescriptor(selectionPrototype, name)?.get?.call(selection);
    if (!selection || selectionValue("rangeCount") !== 1 ||
        typeof selectionPrototype.getRangeAt !== "function" || typeof rangePrototype.intersectsNode !== "function") return null;
    const range = selectionPrototype.getRangeAt.call(selection, 0);
    const rangeValue = name => (Object.getOwnPropertyDescriptor(context.view.AbstractRange?.prototype || rangePrototype, name) ||
      Object.getOwnPropertyDescriptor(rangePrototype, name))?.get?.call(range);
    const snapshot = { startNode: rangeValue("startContainer"), start: rangeValue("startOffset"),
      endNode: rangeValue("endContainer"), end: rangeValue("endOffset"),
      anchorNode: selectionValue("anchorNode"), anchor: selectionValue("anchorOffset"),
      focusNode: selectionValue("focusNode"), focus: selectionValue("focusOffset") };
    const contains = context.view.Node.prototype.contains;
    if (typeof contains !== "function" || ![snapshot.startNode, snapshot.endNode, snapshot.anchorNode, snapshot.focusNode]
      .every(node => node && node.ownerDocument === context.document && contains.call(element, node))) return null;
    if (![snapshot.start, snapshot.end, snapshot.anchor, snapshot.focus].every(offset => Number.isInteger(offset) && offset >= 0)) return null;
    // Reject non-editable islands inside the same bounded structure walk.
    if (!editingStructureWithinBudget(element, maxEditingText, range, rangePrototype.intersectsNode)) return null;
    const getter = Object.getOwnPropertyDescriptor(context.view.HTMLElement.prototype, "innerText")?.get;
    if (typeof getter !== "function") return null;
    const value = getter.call(element);
    return typeof value === "string" && value.length <= maxEditingText ? { ...snapshot, value } : null;
  }

  function sameInsertionSnapshot(before, after, kind) {
    return !!before && !!after && before.value === after.value && before.start === after.start && before.end === after.end &&
      (kind !== "contenteditable" ? before.direction === after.direction :
        before.startNode === after.startNode && before.endNode === after.endNode &&
        before.anchorNode === after.anchorNode && before.anchor === after.anchor &&
        before.focusNode === after.focusNode && before.focus === after.focus);
  }

  function clearInsertObservation(observation = insertObservation) {
    if (!observation) return;
    if (insertObservation === observation) insertObservation = null;
    if (observation.timer !== null) {
      try { window.clearTimeout(observation.timer); } catch (_error) { /* The old realm may already be gone. */ }
    }
    for (const [target, type, listener] of observation.listeners) {
      try { observation.remove.call(target, type, listener, true); } catch (_error) { /* A destroyed realm is unavailable. */ }
    }
    observation.listeners.length = 0;
    observation.text = null;
    observation.expected = null;
  }

  // Native input/textarea fill shares insertion's single bounded receipt.
  // Neither trusted events nor this token distinguish human input from CDP.
  function armInsertObservation(reference, ref, kind, text, expected, action = "insert") {
    const { element, context } = reference;
    const root = element.getRootNode();
    let documentTarget = element, currentRoot = root;
    for (let depth = 0; currentRoot !== context.document && depth < maxDepth; depth += 1) {
      if (currentRoot.nodeType !== 11 || currentRoot.mode !== "open" || !currentRoot.host) throw new Error();
      documentTarget = currentRoot.host;
      currentRoot = documentTarget.getRootNode();
    }
    if (currentRoot !== context.document) throw new Error();
    const prototype = context.view.EventTarget.prototype;
    if (typeof prototype.addEventListener !== "function" || typeof prototype.removeEventListener !== "function") throw new Error();
    const token = Array.from(crypto.getRandomValues(new Uint32Array(4)), n => n.toString(16).padStart(8, "0")).join("");
    const observation = { token, ref, kind, action, generation, text, expected, listeners: [], remove: prototype.removeEventListener,
      timer: null, expires: performance.now() + 5000, seen: false, awaitingRoot: false,
      target: "unconfirmed", inputTypeMatches: false, dataMatches: false,
      beforeinput: action === "fill" ? { seen: false, awaitingRoot: false, target: "unconfirmed" } : null };
    insertObservation = observation;
    const relevant = event => {
      if (insertObservation !== observation) return false;
      if (performance.now() >= observation.expires) { clearInsertObservation(observation); return false; }
      return event.isTrusted === true;
    };
    try {
      for (const type of observation.beforeinput ? ["beforeinput", "input"] : ["input"]) {
        const phase = type === "beforeinput" ? observation.beforeinput : observation;
        const record = event => {
          phase.target = event.target === element ? "matched" : "mismatched";
          phase.inputTypeMatches = event.inputType === "insertText";
          phase.dataMatches = event.data === observation.text;
        };
        const onDocument = event => {
          if (!relevant(event)) return;
          if (phase.seen) { phase.awaitingRoot = false; return; }
          phase.seen = true;
          if (root === context.document) record(event);
          else if (event.target !== documentTarget) phase.target = "mismatched";
          else phase.awaitingRoot = true;
        };
        const onRoot = event => {
          if (!relevant(event) || !phase.awaitingRoot) return;
          phase.awaitingRoot = false;
          record(event);
        };
        for (const [target, listener] of root === context.document
          ? [[context.document, onDocument]] : [[context.document, onDocument], [root, onRoot]]) {
          prototype.addEventListener.call(target, type, listener, { capture: true, passive: true });
          observation.listeners.push([target, type, listener]);
        }
      }
      observation.timer = window.setTimeout(() => clearInsertObservation(observation), 5000);
      if (insertObservation !== observation || refFailure(reference, action)) throw new Error();
      return token;
    } catch (error) {
      clearInsertObservation(observation);
      throw error;
    }
  }

  function prepareInsert(reference, text, ref) {
    const fail = (error, detail) => ({ ...failure("insert", error, detail), input: "chromium", attempted: false });
    try {
      const invalid = refFailure(reference, "insert");
      if (invalid) return { ...invalid, input: "chromium", attempted: false };
      if (typeof text !== "string" || text.length > maxEditingText) return fail("INVALID_TEXT", "Text exceeds the supported insertion budget.");
      const { element } = reference;
      const kind = chromiumFillKind(element);
      if (kind === null) return fail("UNSUPPORTED_ELEMENT", "Insert supports text/search/tel/url/password inputs, textarea and explicit HTML editing hosts.");
      if (chromiumFillReadOnly(element, kind)) return fail("READ_ONLY", "The element is read-only.");
      const current = () => !refFailure(reference, "insert") && chromiumFillKind(element) === kind && !chromiumFillReadOnly(element, kind);
      const before = insertionSnapshot(reference, kind);
      if (!current()) return fail("ELEMENT_UNAVAILABLE", "The insertion target changed before focus; read again.");
      if (!before) return fail("SELECTION_NOT_REACHED", "A supported bounded selection within the target is required; no text was dispatched.");
      element.focus();
      if (!current()) return fail("ELEMENT_UNAVAILABLE", "The insertion target changed while receiving focus; read again.");
      const after = insertionSnapshot(reference, kind);
      const focused = hasPressFocus(reference);
      if (!current()) return fail("ELEMENT_UNAVAILABLE", "The insertion target changed while checking its selection; read again.");
      if (!focused) return fail("FOCUS_NOT_REACHED", "The insertion target did not retain keyboard focus; no text was dispatched.");
      if (!sameInsertionSnapshot(before, after, kind)) return fail("SELECTION_NOT_REACHED", "Focus changed the target's value or selection; no text was dispatched.");
      let expected = null;
      if (kind !== "contenteditable") {
        if (before.value.length - (before.end - before.start) + text.length > maxEditingText) {
          return fail("INVALID_TEXT", "The expected text exceeds the supported insertion budget.");
        }
        expected = before.value.slice(0, before.start) + text + before.value.slice(before.end);
      }
      const observation = armInsertObservation(reference, ref, kind, text, expected);
      if (!current() || !hasPressFocus(reference) || !current()) {
        clearInsertObservation();
        return fail("ELEMENT_UNAVAILABLE", "The target changed while arming insertion observation; no text was dispatched.");
      }
      return { ok: true, action: "insert", input: "chromium", prepared: true, attempted: false, kind, observation };
    } catch (_error) {
      clearInsertObservation();
      return fail("ACTION_FAILED", "Insertion preparation raised an exception; read again before retrying.");
    }
  }

  function verifyInsert(reference, request) {
    const token = typeof request.observation === "string" && /^[a-f0-9]{32}$/.test(request.observation) ? request.observation : "";
    const observation = insertObservation;
    const ownsReceipt = !!observation && observation.token === token;
    const fail = (error, detail) => ({ ...failure("insert", error, detail), input: "chromium", attempted: true, observation: token });
    try {
      if (!ownsReceipt || observation.action !== "insert" || observation.ref !== request.ref || observation.kind !== request.kind ||
          observation.text !== request.text || observation.generation !== generation || performance.now() >= observation.expires) {
        return fail("INSERT_STATE_UNCONFIRMED", "The insertion receipt is missing, expired or no longer current. Read again before retrying.");
      }
      if (observation.target === "mismatched") return fail("INPUT_TARGET_MISMATCH", "A trusted input event reached a different target. Read again before retrying.");
      const kind = observation.kind;
      const current = () => insertObservation === observation && performance.now() < observation.expires &&
        !refFailure(reference, "insert") && chromiumFillKind(reference.element) === kind &&
        !chromiumFillReadOnly(reference.element, kind);
      if (!current() || !hasPressFocus(reference) || !current()) {
        return fail("INSERT_STATE_UNCONFIRMED", "The original insertion target or focus changed. Read again before retrying.");
      }
      if (kind === "contenteditable") {
        if (!editingStructureWithinBudget(reference.element, maxEditingText) || !current() ||
            !hasPressFocus(reference) || !current() ||
            observation.target !== "matched" || !observation.inputTypeMatches || !observation.dataMatches) {
          return fail("INSERT_STATE_UNCONFIRMED", "A matching trusted input event in the original editing host was not confirmed. Read again before retrying.");
        }
        return { ok: true, action: "insert", input: "chromium", attempted: true, observation: token, verification: "input-event" };
      }
      const getter = chromiumFillAccessors(reference, kind).value;
      if (typeof getter !== "function") return fail("INSERT_STATE_UNCONFIRMED", "The native value accessor is unavailable. Read again before retrying.");
      const value = getter.call(reference.element);
      const focused = hasPressFocus(reference);
      if (!current() || !focused || typeof value !== "string" || value.length > maxEditingText) {
        return fail("INSERT_STATE_UNCONFIRMED", "The target changed while checking its final value. Read again before retrying.");
      }
      if (value !== observation.expected) return fail("INSERT_STATE_NOT_REACHED", "Chromium input was dispatched, but the final value differs from the expected insertion. Read again before retrying.");
      return { ok: true, action: "insert", input: "chromium", attempted: true, observation: token, verification: "value" };
    } catch (_error) {
      return fail("INSERT_STATE_UNCONFIRMED", "The insertion outcome could not be verified. Read again before retrying.");
    } finally {
      if (ownsReceipt) clearInsertObservation(observation);
    }
  }

  function prepareFill(reference, text, ref, expectedKind) {
    const fail = (error, detail) => ({ ...failure("fill", error, detail), input: "chromium", attempted: false });
    const { element } = reference;
    const tag = element.tagName;
    let receipt = null;
    const available = () => !refFailure(reference, "fill") && element.tagName === tag &&
      chromiumFillKind(element) !== null && !chromiumFillReadOnly(element, chromiumFillKind(element)) &&
      (expectedKind === undefined || chromiumFillKind(element) === expectedKind);
    try {
      if (chromiumFillReadOnly(element, chromiumFillKind(element))) return fail("READ_ONLY", "The element is read-only.");
      element.focus();
      if (!available()) return fail("ELEMENT_UNAVAILABLE", "The element changed while receiving focus; read again.");
      const kind = chromiumFillKind(element);
      const current = () => available() && chromiumFillKind(element) === kind;
      if (kind === "contenteditable") {
        if (!editingStructureWithinBudget(element, maxEditingText)) {
          return fail("UNSUPPORTED_ELEMENT", "The editing host exceeds the bounded structure/text budget or contains an open shadow root or frame.");
        }
        const selected = selectEditingHost(reference);
        const focused = hasPressFocus(reference);
        if (!current()) return fail("ELEMENT_UNAVAILABLE", "The editing host changed while selecting its contents; read again.");
        if (!focused) return fail("FOCUS_NOT_REACHED", "The editing host did not retain keyboard focus; no text was dispatched.");
        if (!selected) return fail("SELECTION_NOT_REACHED", "The editing host's complete contents were not selected; no text was dispatched.");
        return { ok: true, action: "fill", prepared: true, attempted: false, input: "chromium", kind };
      }
      if (typeof text !== "string" || text.length > maxEditingText) {
        return fail("INVALID_TEXT", "Text exceeds the supported fill observation budget.");
      }
      const access = chromiumFillAccessors(reference, kind);
      if (![access.value, access.start, access.end, access.prototype.setSelectionRange].every((method) => typeof method === "function")) {
        return fail("ACTION_FAILED", "The native selection accessors are unavailable; read again.");
      }
      const value = access.value.call(element);
      if (!current()) return fail("ELEMENT_UNAVAILABLE", "The element changed before selection; read again.");
      if (typeof value !== "string") return fail("ACTION_FAILED", "The native value could not be read; read again.");
      access.prototype.setSelectionRange.call(element, 0, value.length, "forward");
      if (!current()) return fail("ELEMENT_UNAVAILABLE", "The element changed while selecting its contents; read again.");
      const selectedValue = access.value.call(element);
      const start = access.start.call(element), end = access.end.call(element);
      const focused = hasPressFocus(reference);
      if (!current()) return fail("ELEMENT_UNAVAILABLE", "The element changed while checking its selection; read again.");
      if (!focused) return fail("FOCUS_NOT_REACHED", "The element did not retain keyboard focus; no text was dispatched.");
      if (typeof selectedValue !== "string" || selectedValue !== value || start !== 0 || end !== selectedValue.length) {
        return fail("SELECTION_NOT_REACHED", "The element's full value was not selected; no text was dispatched.");
      }
      const observation = armInsertObservation(reference, ref, kind, text, null, "fill");
      receipt = insertObservation;
      if (!current() || !hasPressFocus(reference) || !current() || insertObservation !== receipt) {
        clearInsertObservation(receipt);
        return fail("ELEMENT_UNAVAILABLE", "The target changed while arming fill observation; no text was dispatched.");
      }
      return { ok: true, action: "fill", prepared: true, attempted: false, input: "chromium", kind, observation };
    } catch (_error) {
      if (receipt) clearInsertObservation(receipt);
      return fail("ACTION_FAILED", "Text input preparation raised an exception; read again before retrying.");
    }
  }

  function verifyFill(request) {
    const { text, kind } = request;
    const token = typeof request.observation === "string" && /^[a-f0-9]{32}$/.test(request.observation) ? request.observation : "";
    const observation = insertObservation;
    const ownsReceipt = !!observation && observation.action === "fill" && observation.token === token;
    const needsReceipt = kind !== "contenteditable" || observation?.action === "fill" || token !== "";
    let reference = null;
    const fail = (error, detail) => ({ ...failure("fill", error, detail), input: "chromium", attempted: true });
    const current = () => !refFailure(reference, "fill") && chromiumFillKind(reference.element) === kind &&
      kind !== null && !chromiumFillReadOnly(reference.element, kind) &&
      (!needsReceipt || (insertObservation === observation && performance.now() < observation.expires));
    try {
      if (needsReceipt) {
        if (!ownsReceipt || observation.ref !== request.ref || observation.kind !== kind || observation.text !== text ||
            observation.generation !== generation || performance.now() >= observation.expires) {
          return fail("FILL_STATE_UNCONFIRMED", "The fill receipt is missing, expired or no longer current. Read again before retrying.");
        }
        // Preserve observed wrong-target evidence even if that input detached or
        // changed the original ref before final state/value verification.
        if (observation.target === "mismatched" || observation.beforeinput?.target === "mismatched") {
          return fail("INPUT_TARGET_MISMATCH", "A trusted text input event reached a different target. Read again before retrying.");
        }
      }
      reference = resolveReference(request.ref);
      if (typeof text !== "string" || !["text", "search", "tel", "url", "password", "textarea", "contenteditable"].includes(kind) || !current()) {
        return fail("FILL_STATE_UNCONFIRMED", "The original text target is no longer available for verification. Read again before retrying.");
      }
      if (kind === "contenteditable" && (text.length > maxEditingText ||
          !editingStructureWithinBudget(reference.element, Math.min(maxEditingText, text.length + maxNodes)))) {
        return fail("FILL_STATE_UNCONFIRMED", "The editing host exceeds the bounded verification budget or contains an open shadow root or frame. Read again before retrying.");
      }
      const getter = kind === "contenteditable"
        ? Object.getOwnPropertyDescriptor(reference.context.view.HTMLElement.prototype, "innerText")?.get
        : chromiumFillAccessors(reference, kind).value;
      if (typeof getter !== "function") return fail("FILL_STATE_UNCONFIRMED", "The native value accessor is unavailable. Read again before retrying.");
      if (!current()) return fail("FILL_STATE_UNCONFIRMED", "The original text target changed before its native value was read. Read again before retrying.");
      const value = getter.call(reference.element);
      const focused = hasPressFocus(reference);
      if (!current() || !focused || typeof value !== "string") {
        return fail("FILL_STATE_UNCONFIRMED", "The target changed while checking its final value. Read again before retrying.");
      }
      if (value !== text) return fail("FILL_STATE_NOT_REACHED", "Chromium input was dispatched, but the final value differs from the requested text. Read again before retrying.");
      return { ok: true, action: "fill", verified: true, input: "chromium", attempted: true,
        ...(needsReceipt ? { inputTarget: observation.target === "matched" ? "matched" : "unconfirmed" } : {}) };
    } catch (_error) {
      return fail("FILL_STATE_UNCONFIRMED", "The final text value could not be verified. Read again before retrying.");
    } finally {
      if (ownsReceipt) clearInsertObservation(observation);
    }
  }

  function fillNative(reference, text, expectedKind) {
    const { element, context } = reference;
    const tag = element.tagName;
    let attempted = false;
    const fail = (error, detail) => ({ ...failure("fill", error, detail), attempted, trusted: false });
    const available = () => !refFailure(reference, "fill") && element.tagName === tag &&
      supportsFill(element) && !element.readOnly && (expectedKind === undefined ||
        (element.type === expectedKind && chromiumFillKind(element) === null));
    try {
      if (element.readOnly) return fail("READ_ONLY", "The element is read-only.");
      element.focus();
      if (!available()) return fail("ELEMENT_UNAVAILABLE", "The element changed while receiving focus; read again.");
      // Focus may make a supported text/password conversion. After this point
      // a type change is a different control state, even if its value matches.
      const type = tag === "INPUT" ? element.type : null;
      const current = () => available() && (tag !== "INPUT" || element.type === type);
      const prototype = tag === "INPUT" ? context.view.HTMLInputElement.prototype : context.view.HTMLTextAreaElement.prototype;
      const descriptor = Object.getOwnPropertyDescriptor(prototype, "value");
      if (typeof descriptor?.set !== "function" || typeof descriptor?.get !== "function") {
        return fail("ACTION_FAILED", "The native value accessors are unavailable; read again.");
      }
      attempted = true;
      descriptor.set.call(element, text);
      if (!current()) return fail("FILL_STATE_UNCONFIRMED", "Value setting was attempted, but the target changed before input; no input/change events were dispatched. Read again before retrying.");
      element.dispatchEvent(new context.view.Event("input", { bubbles: true, composed: true }));
      if (!current()) return fail("FILL_STATE_UNCONFIRMED", "Value setting and input were attempted, but the target changed before change; change was not dispatched. Read again before retrying.");
      element.dispatchEvent(new context.view.Event("change", { bubbles: true }));
      if (!current()) return fail("FILL_STATE_UNCONFIRMED", "Value setting and input/change were attempted, but the target changed. Read again before retrying.");
      // Use the native getter, not a page-defined own value property. Exact
      // comparison deliberately does not reimplement type-specific sanitizers.
      const value = descriptor.get.call(element);
      if (!current()) return fail("FILL_STATE_UNCONFIRMED", "The target changed while checking the value. Read again before retrying.");
      if (typeof value !== "string") return fail("ACTION_FAILED", "The native value could not be verified; read again before retrying.");
      if (value !== text) return fail("FILL_STATE_NOT_REACHED", "Value setting and input/change were attempted, but the final value differs from the requested text. Read again before retrying.");
      return { ok: true, action: "fill", trusted: false,
        detail: "Value set and input/change events dispatched; the native value matched the requested text at the final synchronous check." };
    } catch (_error) {
      return fail("ACTION_FAILED", "The native fill action raised an exception; read again before retrying.");
    }
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
    if (frameSnapshot) {
      return { ...failure(request?.action ?? null, "FRAME_BINDING_REQUIRED", "Use frameAct with this frame snapshot's documentId/generation binding."), attempted: false };
    }
    return performAction(request);
  }

  function performAction(request) {
    const verifyingPress = request && typeof request === "object" && !Array.isArray(request) &&
      request.action === "press" && request.verifyOnly === true;
    const verifyingInsert = request && typeof request === "object" && !Array.isArray(request) &&
      request.action === "insert" && request.verifyOnly === true;
    const verifyingFill = request && typeof request === "object" && !Array.isArray(request) &&
      request.action === "fill" && request.verifyOnly === true;
    if (!verifyingPress) clearPressObservation();
    if (!(verifyingInsert || verifyingFill) || insertObservation?.action !== request.action) clearInsertObservation();
    if (!request || typeof request !== "object" || Array.isArray(request)) {
      return failure(null, "INVALID_REQUEST", "Expected an action object.");
    }
    const action = request.action;
    if (!["click", "hover", "fill", "insert", "press", "scroll", "select", "check"].includes(action)) {
      return failure(null, "UNSUPPORTED_ACTION", "Use click, hover, fill, insert, press, scroll, select or check.");
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

      if (action === "fill" && verifyingFill) return verifyFill(request);
      const reference = resolveReference(request.ref);
      if (action === "press") {
        if (verifyingPress) return verifyPress(request);
        return preparePress(reference, request.key, request.ref);
      }
      if (action === "click" || action === "hover") return preparePointer(reference, request);
      if (action === "insert") return verifyingInsert ? verifyInsert(reference, request) : prepareInsert(reference, request.text, request.ref);
      const unavailable = refFailure(reference, action);
      if (unavailable) return unavailable;
      const { element } = reference;
      if (action === "select") return selectValue(reference, request.value);
      if (action === "check") return checkValue(reference, request.checked);

      if (action === "fill") {
        const inspected = inspectFill(reference);
        if (!inspected.ok) return inspected;
        if (request.expectedInput !== undefined || request.expectedKind !== undefined) {
          if (request.expectedInput !== inspected.input || request.expectedKind !== inspected.kind) {
            return { ...failure(action, "ELEMENT_UNAVAILABLE", "The fill path or kind changed after inspection; no text was dispatched."), attempted: false };
          }
        }
        if (request.inspectOnly === true) return inspected;
        if (typeof request.text !== "string") {
          return failure(action, "INVALID_TEXT", "Text must be a string.");
        }
        if (inspected.input === "chromium") return prepareFill(reference, request.text, request.ref, request.expectedKind);
        return fillNative(reference, request.text, request.expectedKind);
      }
    } catch (_error) {
      // Do not echo page errors: they can include input values or other page data.
      const result = failure(action, "ACTION_FAILED", "The page action raised an exception; read again to inspect the page.");
      if (action === "insert") return { ...result, input: "chromium", attempted: verifyingInsert,
        ...(verifyingInsert ? { observation: typeof request.observation === "string" && /^[a-f0-9]{32}$/.test(request.observation) ? request.observation : "" } : {}) };
      return ["press", "click", "hover"].includes(action) ? { ...result, attempted: action === "press" && verifyingPress } : result;
    }
  }

  const bridge = Object.freeze({ read, readFrame, readFrameActions, frameAct, clearFrame, inspectFrameOwner, inspectFramePointer, act });
  Object.defineProperty(window, bridgeName, { value: bridge, writable: false, configurable: false });
  return bridge;
})();
