// Small non-monetary surprise discoveries. Optional; never blocks reading.
(() => {
  "use strict";
  if (!/^\/$|^\/books\/\d+$|^\/reader\/\d+$/.test(location.pathname)) return;
  if (window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    // Stationary gifts are allowed; no animation ever runs.
  }
  const main = document.getElementById("main");
  if (!main || document.visibilityState !== "visible") return;
  const POST = async (name, csrf, data) => {
    const response = await fetch("/api/v1/me/surprises/" + name, {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
      headers: { "Content-Type": "application/json", "X-CSRF-Token": csrf },
      body: JSON.stringify(data),
    });
    if (!response.ok) return null;
    if (response.status === 204) return {};
    return response.json();
  };
  function safeContext() {
    if (document.visibilityState !== "visible" || !document.hasFocus()) return false;
    if (window.getSelection && String(window.getSelection()).length > 0) return false;
    if (document.querySelector('dialog[open], [role="dialog"]:not([hidden]), .support-widget__panel:not([hidden])')) return false;
    const focus = document.activeElement;
    if (focus && focus.matches("input,textarea,select,[contenteditable=true]")) return false;
    return true;
  }
  function pickLocation() {
    const rect = main.getBoundingClientRect();
    const gapL = rect.left;
    const gapR = innerWidth - rect.right;
    const gutters = [];
    if (gapL >= 72) gutters.push("left");
    if (gapR >= 72) gutters.push("right");
    if (gutters.length) {
      const random = new Uint32Array(1);
      crypto.getRandomValues(random);
      const side = gutters[random[0] % gutters.length];
      return { position: side, parent: document.body };
    }
    // Mobile: only an in-flow element after a visible paragraph. No overlays.
    const paras = Array.from(main.querySelectorAll("p")).filter((node) => {
      const r = node.getBoundingClientRect();
      return r.top > 65 && r.bottom < innerHeight - 90 &&
        !node.closest("form,nav,header,footer,.chapter-navigation,.support-widget");
    });
    if (!paras.length) return null;
    const random = new Uint32Array(1);
    crypto.getRandomValues(random);
    return { position: "inline", parent: paras[random[0] % paras.length] };
  }
  function render(entry, spot, plan) {
    const node = document.createElement("aside");
    node.className = "tor-surprise tor-surprise--" + spot.position;
    node.setAttribute("aria-label", "Неожиданная находка");
    if (spot.position === "inline") spot.parent.insertAdjacentElement("afterend", node);
    else document.body.append(node);
    const trigger = document.createElement("button");
    trigger.type = "button";
    trigger.className = "tor-surprise__trigger";
    trigger.textContent = "🎁";
    trigger.setAttribute("aria-label", "Открыть случайную находку");
    node.append(trigger);
    const expiry = setTimeout(() => node.remove(), 15000);
    trigger.addEventListener("click", async () => {
      clearTimeout(expiry);
      trigger.disabled = true;
      const item = await POST("open", plan.csrf_token, { utc_day: plan.utc_day });
      if (!item) { node.remove(); return; }
      node.classList.add("tor-surprise--opened");
      node.textContent = "";
      const card = document.createElement("section");
      card.className = "tor-surprise__card";
      card.setAttribute("aria-label", "Содержимое находки");
      const heading = document.createElement("h3");
      heading.textContent = item.title;
      const body = document.createElement("p");
      body.textContent = item.body;
      card.append(heading, body);
      if (item.editorial) {
        const tag = document.createElement("p");
        tag.className = "tor-surprise__label";
        tag.textContent = "Выбор редакции";
        card.prepend(tag);
      }
      if (Number.isSafeInteger(item.book_id) && item.book_id > 0) {
        const link = document.createElement("a");
        link.textContent = "Открыть книгу";
        link.href = "/books/" + item.book_id;
        card.append(link);
      }
      const close = document.createElement("button");
      close.type = "button";
      close.textContent = "Закрыть";
      close.addEventListener("click", () => node.remove());
      const disable = document.createElement("button");
      disable.type = "button";
      disable.textContent = "Больше не показывать";
      disable.addEventListener("click", async () => {
        await POST("disable", plan.csrf_token, {});
        node.remove();
      });
      const controls = document.createElement("div");
      controls.className = "tor-surprise__controls";
      controls.append(close, disable);
      card.append(controls);
      node.append(card);
      close.focus();
    }, { once: true });
  }
  async function start() {
    let response;
    try {
      response = await fetch("/api/v1/me/surprises/plan", {
        credentials: "same-origin", cache: "no-store",
      });
      if (!response.ok) return;
      response = await response.json();
    } catch { return; }
    const plan = response;
    if (plan.state !== "planned" || !Number.isInteger(plan.active_after_seconds) || !Number.isInteger(plan.utc_day)) return;
    let activeSeconds = 0;
    let overdue = 0;
    let running = false;
    let lastAction = Date.now();
    ["keydown", "pointerdown", "scroll", "touchstart"].forEach((event) =>
      document.addEventListener(event, () => { lastAction = Date.now(); }, { passive: true })
    );
    const ticker = setInterval(async () => {
      if (!safeContext() || Date.now() - lastAction > 120000) return;
      activeSeconds++;
      if (activeSeconds < plan.active_after_seconds || running) return;
      const loc = pickLocation();
      if (!loc) {
        overdue++;
        if (overdue < 300) return;
        clearInterval(ticker);
        return;
      }
      running = true;
      clearInterval(ticker);
      let claimed;
      try { claimed = await POST("claim", plan.csrf_token, { utc_day: plan.utc_day, active_seconds: activeSeconds }); }
      catch { return; }
      if (claimed && claimed.accepted && safeContext()) render(null, loc, plan);
    }, 1000);
    addEventListener("pagehide", () => clearInterval(ticker), { once: true });
  }
  start();
})();
