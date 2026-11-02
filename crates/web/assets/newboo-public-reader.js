// Free chapters only: persist harmless presentation preferences, never chapter content.
(() => {
  "use strict";
  const root = document.querySelector("[data-public-reader]");
  if (!root) return;
  const controls = {
    theme: root.querySelector("[data-public-theme]"),
    font: root.querySelector("[data-public-font]"),
    scale: root.querySelector("[data-public-scale]"),
    spacing: root.querySelector("[data-public-spacing]"),
    width: root.querySelector("[data-public-width]"),
    reset: root.querySelector("[data-public-reset]"),
    scaleOut: root.querySelector("[data-public-scale-out]"),
    spacingOut: root.querySelector("[data-public-spacing-out]"),
    widthOut: root.querySelector("[data-public-width-out]"),
  };
  if (Object.values(controls).some((control) => !control)) return;

  const storageKey = "tor.free_reader_settings.v1";
  const defaults = { theme: "system", font: "serif", scale: 100, spacing: 185, width: 860 };
  const choices = { theme: ["system", "light", "sepia", "dark"], font: ["serif", "sans"] };
  const ranges = { scale: [80, 140, 5], spacing: [150, 220, 5], width: [620, 980, 20] };
  const validNumber = (value, [min, max, step]) =>
    Number.isInteger(value) && value >= min && value <= max && (value - min) % step === 0;
  const readSettings = (record) => {
    const settings = { ...defaults };
    if (!record || typeof record !== "object" || Array.isArray(record)) return settings;
    for (const key of ["theme", "font"]) {
      if (choices[key].includes(record[key])) settings[key] = record[key];
    }
    for (const key of ["scale", "spacing", "width"]) {
      if (validNumber(record[key], ranges[key])) settings[key] = record[key];
    }
    return settings;
  };
  let settings = { ...defaults };
  try {
    settings = readSettings(JSON.parse(window.localStorage.getItem(storageKey)));
  } catch {
    // Private browsing or restricted storage must not break reading.
  }
  const save = () => {
    try {
      window.localStorage.setItem(storageKey, JSON.stringify(settings));
    } catch {
      // Preferences remain usable in the current page even when storage is denied.
    }
  };
  const apply = () => {
    for (const key of ["theme", "font", "scale", "spacing", "width"]) {
      controls[key].value = String(settings[key]);
    }
    if (settings.theme === "system") delete root.dataset.publicTheme;
    else root.dataset.publicTheme = settings.theme;
    if (settings.font === "serif") root.style.removeProperty("--public-font-family");
    else root.style.setProperty("--public-font-family", "Arial, Helvetica, sans-serif");
    if (settings.scale === 100) root.style.removeProperty("--public-font-size");
    else root.style.setProperty("--public-font-size", String(settings.scale / 5) + "px");
    if (settings.spacing === 185) root.style.removeProperty("--public-line-height");
    else root.style.setProperty("--public-line-height", String(settings.spacing / 100));
    if (settings.width === 860) root.style.removeProperty("--public-reader-width");
    else root.style.setProperty("--public-reader-width", String(settings.width) + "px");
    controls.scaleOut.textContent = String(settings.scale) + "%";
    controls.spacingOut.textContent = (settings.spacing / 100).toFixed(2);
    controls.widthOut.textContent = String(settings.width) + " px";
  };
  for (const key of ["theme", "font", "scale", "spacing", "width"]) {
    const eventName = ranges[key] ? "input" : "change";
    controls[key].addEventListener(eventName, () => {
      const raw = controls[key].value;
      const value = ranges[key] ? Number(raw) : raw;
      if (ranges[key] ? !validNumber(value, ranges[key]) : !choices[key].includes(value)) return;
      settings[key] = value;
      apply();
      save();
    });
  }
  controls.reset.addEventListener("click", () => {
    settings = { ...defaults };
    apply();
    save();
  });
  root.querySelector("[data-public-settings]")?.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      const details = root.querySelector("[data-public-settings]");
      details.open = false;
      details.querySelector("summary")?.focus();
    }
  });
  apply();
  root.setAttribute("data-public-ready", "");
})();
