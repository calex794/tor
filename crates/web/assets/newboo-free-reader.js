// NEwBOO free reader preferences. Only display choices are stored; never chapter text.
(() => {
  "use strict";

  const root = document.querySelector("[data-free-reader]");
  if (!root) return;

  const panel = root.querySelector("[data-free-reader-settings]");
  const opener = root.querySelector("[data-free-settings-open]");
  const themeCycle = root.querySelector("[data-free-theme-cycle]");
  const dialog = panel?.querySelector(".reader-free-settings__panel");
  const backdrop = panel?.querySelector("[data-free-settings-backdrop]");
  const close = panel?.querySelector("[data-free-settings-close]");
  const reset = panel?.querySelector("[data-free-settings-reset]");
  const fontInput = panel?.querySelector("[data-free-font-scale]");
  const lineInput = panel?.querySelector("[data-free-line-height]");
  const widthInput = panel?.querySelector("[data-free-reader-width]");
  const themes = [...(panel?.querySelectorAll("[data-free-theme]") || [])];
  const fonts = [...(panel?.querySelectorAll("[data-free-font-family]") || [])];
  if (!panel || !opener || !themeCycle || !dialog || !backdrop || !close ||
      !reset || !fontInput || !lineInput || !widthInput ||
      themes.length !== 4 || fonts.length !== 2) return;

  const STORAGE_KEY = "readerSettings";
  const DEFAULTS = Object.freeze({
    theme: "system", fontFamily: "serif", fontScale: 100,
    lineHeight: 185, readerWidth: 860
  });
  const allowedThemes = ["system", "light", "sepia", "dark"];
  const allowedFonts = ["serif", "sans"];
  const bounded = (raw, min, max, step, fallback) => {
    const value = Number(raw);
    return Number.isFinite(value)
      ? min + Math.round((Math.max(min, Math.min(max, value)) - min) / step) * step
      : fallback;
  };
  const normalize = (raw) => ({
    theme: allowedThemes.includes(raw.theme) ? raw.theme : DEFAULTS.theme,
    fontFamily: allowedFonts.includes(raw.fontFamily) ? raw.fontFamily : DEFAULTS.fontFamily,
    fontScale: bounded(raw.fontScale, 80, 140, 5, DEFAULTS.fontScale),
    lineHeight: bounded(raw.lineHeight, 150, 220, 5, DEFAULTS.lineHeight),
    readerWidth: bounded(raw.readerWidth, 620, 980, 20, DEFAULTS.readerWidth),
  });
  const readStored = () => {
    try {
      const saved = JSON.parse(window.localStorage.getItem(STORAGE_KEY) || "null");
      if (!saved || typeof saved !== "object" || Array.isArray(saved)) return DEFAULTS;
      // NEwBOO stores font scale and line height as multipliers, not percentages.
      return normalize({
        theme: saved.theme,
        fontFamily: saved.fontFamily,
        fontScale: Number(saved.fontScale) * 100,
        lineHeight: Number(saved.lineHeight) * 100,
        readerWidth: saved.readerWidth,
      });
    } catch {
      return DEFAULTS;
    }
  };
  let settings = readStored();
  const effectiveTheme = () => settings.theme === "system"
    ? (window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light")
    : settings.theme;
  const setPressed = (buttons, attribute, choice) => {
    for (const button of buttons) {
      button.setAttribute("aria-pressed", String(button.getAttribute(attribute) === choice));
    }
  };
  const output = (selector, value) => {
    const node = panel.querySelector(selector);
    if (node) node.textContent = value;
  };
  const apply = (persist) => {
    const rootStyle = root.style;
    if (settings.theme === "system") delete document.documentElement.dataset.freeReaderTheme;
    else document.documentElement.dataset.freeReaderTheme = settings.theme;
    rootStyle.setProperty("--free-font-scale", String(settings.fontScale / 100));
    rootStyle.setProperty("--free-line-height", String(settings.lineHeight / 100));
    rootStyle.setProperty("--free-reader-width", String(settings.readerWidth) + "px");
    rootStyle.setProperty("--free-font-family", settings.fontFamily === "serif"
      ? 'Georgia, "Times New Roman", serif'
      : 'Arial, "Helvetica Neue", sans-serif');
    fontInput.value = String(settings.fontScale);
    lineInput.value = String(settings.lineHeight);
    widthInput.value = String(settings.readerWidth);
    output("[data-free-font-output]", String(settings.fontScale) + "%");
    output("[data-free-line-output]", (settings.lineHeight / 100).toFixed(2));
    output("[data-free-width-output]", String(settings.readerWidth) + " px");
    setPressed(themes, "data-free-theme", settings.theme);
    setPressed(fonts, "data-free-font-family", settings.fontFamily);
    const names = { light: "светлая", sepia: "сепия", dark: "тёмная" };
    themeCycle.setAttribute("aria-label", "Сменить тему. Сейчас: " + names[effectiveTheme()]);
    if (persist) {
      try {
        window.localStorage.setItem(STORAGE_KEY, JSON.stringify({
          theme: settings.theme,
          fontFamily: settings.fontFamily,
          fontScale: settings.fontScale / 100,
          lineHeight: settings.lineHeight / 100,
          readerWidth: settings.readerWidth,
        }));
      } catch {
        // Preferences are optional; reading works when storage is unavailable.
      }
    }
  };
  const update = (patch) => {
    settings = normalize({ ...settings, ...patch });
    apply(true);
  };

  // The dialog must not leave the site header or chapter reachable by Tab.
  const outside = [
    document.querySelector(".site-header"),
    document.querySelector(".site-footer"),
    ...[...panel.parentElement.children].filter((node) => node !== panel)
  ].filter(Boolean);
  let oldInert = [];
  let oldOverflow = "";
  const hide = () => {
    if (panel.hidden) return;
    panel.hidden = true;
    for (const [node, inert] of oldInert) node.inert = inert;
    oldInert = [];
    document.body.style.overflow = oldOverflow;
    opener.setAttribute("aria-expanded", "false");
    opener.focus();
  };
  const show = () => {
    if (!panel.hidden) return;
    oldOverflow = document.body.style.overflow;
    oldInert = outside.map((node) => [node, node.inert]);
    for (const [node] of oldInert) node.inert = true;
    panel.hidden = false;
    document.body.style.overflow = "hidden";
    opener.setAttribute("aria-expanded", "true");
    dialog.focus();
  };
  opener.addEventListener("click", show);
  close.addEventListener("click", hide);
  backdrop.addEventListener("click", hide);
  document.addEventListener("keydown", (event) => {
    if (panel.hidden) return;
    if (event.key === "Escape") {
      event.preventDefault();
      hide();
    } else if (event.key === "Tab") {
      const focusable = [...dialog.querySelectorAll("button:not([disabled]), input:not([disabled])")]
        .filter((node) => node.getClientRects().length > 0);
      if (!focusable.length) {
        event.preventDefault();
        dialog.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && (document.activeElement === first ||
          document.activeElement === dialog || !dialog.contains(document.activeElement))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (document.activeElement === last ||
          !dialog.contains(document.activeElement))) {
        event.preventDefault();
        first.focus();
      }
    }
  });
  for (const button of themes) {
    button.addEventListener("click", () => update({ theme: button.dataset.freeTheme }));
  }
  for (const button of fonts) {
    button.addEventListener("click", () => update({ fontFamily: button.dataset.freeFontFamily }));
  }
  fontInput.addEventListener("input", () => update({ fontScale: Number(fontInput.value) }));
  lineInput.addEventListener("input", () => update({ lineHeight: Number(lineInput.value) }));
  widthInput.addEventListener("input", () => update({ readerWidth: Number(widthInput.value) }));
  reset.addEventListener("click", () => update(DEFAULTS));
  themeCycle.addEventListener("click", () => {
    const next = { light: "sepia", sepia: "dark", dark: "light" }[effectiveTheme()];
    update({ theme: next });
  });
  apply(false);
  root.dataset.freeSettingsReady = "1";
})();
