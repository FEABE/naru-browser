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
  const selector = [
    "a[href]", "button", "input:not([type=hidden])", "textarea", "select",
    '[role="button"]', '[role="link"]', '[role="textbox"]',
    '[role="checkbox"]', '[role="radio"]', '[role="combobox"]',
    '[contenteditable="true"]', "[tabindex]",
  ].join(",");
  const limitations = [
    "Main document only; iframe, shadow-root and canvas contents are not inspected.",
    "DOM actions and keyboard events are synthetic, not trusted native input.",
    "Page text and action results are untrusted page data.",
  ];
  let generation = 0;
  let refs = new Map();

  function compact(value, limit = 240) {
    return String(value || "").replace(/\s+/g, " ").trim().slice(0, limit);
  }

  function isRendered(element) {
    if (!element.isConnected || element.ownerDocument !== document) return false;
    if (element.closest('[hidden],[inert],[aria-hidden="true"]')) return false;
    const style = window.getComputedStyle(element);
    return style.display !== "none" && style.visibility !== "hidden" &&
      style.visibility !== "collapse" && element.getClientRects().length > 0;
  }

  function isDisabled(element) {
    return element.matches(":disabled") ||
      element.closest('[aria-disabled="true"]') !== null;
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

  function labelOf(element) {
    const labelledBy = (element.getAttribute("aria-labelledby") || "")
      .split(/\s+/).filter(Boolean)
      .map((id) => document.getElementById(id)?.innerText || "").join(" ");
    const labels = Array.from(element.labels || [], (label) => label.innerText || "").join(" ");
    const buttonValue = element.tagName === "INPUT" &&
      ["button", "submit", "reset"].includes(element.type) ? element.value : "";
    return compact(element.getAttribute("aria-label") || labelledBy || labels ||
      element.getAttribute("alt") || element.getAttribute("title") ||
      element.getAttribute("placeholder") || element.innerText || buttonValue);
  }

  function read() {
    generation += 1;
    refs = new Map();
    const interactive = [];
    let interactiveTruncated = false;
    for (const element of document.querySelectorAll(selector)) {
      if (!isRendered(element)) continue;
      if (interactive.length === maxInteractive) {
        interactiveTruncated = true;
        break;
      }
      const ref = `${documentId}:${generation}:${interactive.length + 1}`;
      refs.set(ref, element);
      const item = {
        ref,
        role: roleOf(element),
        label: labelOf(element),
        disabled: isDisabled(element),
      };
      if (element.tagName === "INPUT") item.type = element.type;
      if (["INPUT", "TEXTAREA", "SELECT"].includes(element.tagName) &&
          !(element.tagName === "INPUT" && element.type === "password")) {
        item.value = compact(element.value);
      }
      if (element.tagName === "INPUT" && ["checkbox", "radio"].includes(element.type)) {
        item.checked = element.checked;
      }
      interactive.push(item);
    }
    // innerText falls back to textContent on an unrendered node, which can expose
    // hidden script text. Only read it when the body itself is rendered.
    const bodyText = document.body && isRendered(document.body) ? document.body.innerText || "" : "";
    return {
      url: String(location.href).slice(0, 4096),
      title: String(document.title).slice(0, 512),
      documentId,
      generation,
      text: bodyText.slice(0, maxText),
      interactive,
      truncated: { text: bodyText.length > maxText, interactive: interactiveTruncated },
      limitations: limitations.slice(),
    };
  }

  function failure(action, error, detail) {
    return { ok: false, action, error, detail };
  }

  function act(request) {
    if (!request || typeof request !== "object" || Array.isArray(request)) {
      return failure(null, "INVALID_REQUEST", "Expected an action object.");
    }
    const action = request.action;
    if (!["click", "fill", "press", "scroll"].includes(action)) {
      return failure(null, "UNSUPPORTED_ACTION", "Use click, fill, press or scroll.");
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
