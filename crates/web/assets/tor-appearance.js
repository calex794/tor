/* UI-01: presentation preferences only; never reads book, wallet, or session data. */
(function () {
  "use strict";
  const KEY = "torAppearance.v1";
  const OPTIONS = Object.freeze({
    style: ["showcase", "library", "minimal"],
    theme: ["system", "light", "dark"],
    density: ["compact", "normal", "spacious"],
    catalog: ["grid", "list", "covers"]
  });
  const DEFAULTS = Object.freeze({
    style: "showcase", theme: "system", density: "normal", catalog: "grid"
  });

  function normalize(input) {
    const raw = input && typeof input === "object" && !Array.isArray(input) ? input : {};
    const result = {};
    for (const setting of Object.keys(DEFAULTS)) {
      result[setting] = OPTIONS[setting].includes(raw[setting])
        ? raw[setting] : DEFAULTS[setting];
    }
    return result;
  }

  function load() {
    try {
      return normalize(JSON.parse(window.localStorage.getItem(KEY) || "null"));
    } catch (_) {
      return normalize(null);
    }
  }

  let settings = load();
  const root = document.documentElement;

  // UI-03A: the private library intentionally forbids all JavaScript.
  // Mirror this non-sensitive enum to a first-party cookie so its server
  // can choose the same color scheme without weakening the private CSP.
  function mirrorThemeCookie() {
    try {
      const secure = window.location?.protocol === "https:" ? "; Secure" : "";
      const hasThemeCookie = (document.cookie || "").split(";").some(
        item => item.trim().startsWith("tor-ui-theme-v1=")
      );
      if (settings.theme === "system") {
        // The OS theme needs no cookie. Remove a previously selected theme.
        if (hasThemeCookie) {
          document.cookie = "tor-ui-theme-v1=; Path=/; Max-Age=0; SameSite=Lax" + secure;
        }
      } else {
        document.cookie = "tor-ui-theme-v1=" + settings.theme +
          "; Path=/; Max-Age=31536000; SameSite=Lax" + secure;
      }
    } catch (_) {
      // Storage/cookies may be disabled; system theme remains the fallback.
    }
  }

  // Fixed, non-sensitive style enum for real book SSR under script-src 'none'.
  function mirrorStyleCookie() {
    try {
      const secure = window.location?.protocol === "https:" ? "; Secure" : "";
      document.cookie = "tor-ui-style-v1=" + settings.style +
        "; Path=/; Max-Age=31536000; SameSite=Lax" + secure;
    } catch (_) {
      // Cookie denial never blocks reading or navigation.
    }
  }

  function apply() {
    root.dataset.torStyle = settings.style;
    root.dataset.torTheme = settings.theme;
    root.dataset.torDensity = settings.density;
    root.dataset.torCatalog = settings.catalog;
    mirrorThemeCookie();
    mirrorStyleCookie();
  }

  // This script intentionally loads before CSS to avoid a light/dark flash.
  apply();

  function save() {
    try {
      window.localStorage.setItem(KEY, JSON.stringify(settings));
    } catch (_) {
      // Preferences must never prevent navigation or reading.
    }
  }

  function refreshButtons(panel) {
    for (const button of panel.querySelectorAll("button[data-tor-setting][data-tor-value]")) {
      const setting = button.dataset.torSetting;
      button.setAttribute("aria-pressed", String(settings[setting] === button.dataset.torValue));
    }
  }

  document.addEventListener("DOMContentLoaded", function () {
    const panel = document.querySelector("[data-tor-appearance]");
    if (!panel) return;
    panel.hidden = false;
    refreshButtons(panel);

    panel.addEventListener("click", function (event) {
      const button = event.target.closest && event.target.closest("button[data-tor-setting][data-tor-value]");
      if (!button || !panel.contains(button)) return;
      const setting = button.dataset.torSetting;
      const value = button.dataset.torValue;
      if (!Object.prototype.hasOwnProperty.call(OPTIONS, setting) ||
          !OPTIONS[setting].includes(value)) return;
      settings = { ...settings, [setting]: value };
      apply();
      refreshButtons(panel);
      save();
    });

    panel.addEventListener("keydown", function (event) {
      if (event.key === "Escape" && panel.open) {
        panel.open = false;
        panel.querySelector("summary")?.focus();
      }
    });

    window.addEventListener("storage", function (event) {
      if (event.key !== KEY) return;
      settings = load();
      apply();
      refreshButtons(panel);
    });
  });
})();