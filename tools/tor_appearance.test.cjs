/* UI-01: test first-paint preferences without external DOM packages. */
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const source = fs.readFileSync(
  path.join(__dirname, "../crates/web/assets/tor-appearance.js"), "utf8"
);

function boot(value, storageBroken = false) {
  const attributes = {};
  const handlers = {};
  const events = {};
  let stored = value;
  const panel = {
    hidden: true,
    querySelectorAll() { return []; },
    addEventListener(kind, handler) { events[kind] = handler; },
    contains() { return true; },
    querySelector() { return null; }
  };
  const document = {
    documentElement: { dataset: attributes },
    addEventListener(kind, handler) { handlers[kind] = handler; },
    querySelector() { return panel; }
  };
  const window = {
    localStorage: {
      getItem() { if (storageBroken) throw Error("blocked"); return stored; },
      setItem(_, next) { if (storageBroken) throw Error("blocked"); stored = next; }
    },
    addEventListener(kind, handler) { handlers["window:" + kind] = handler; }
  };
  vm.runInNewContext(source, { window, document });
  return { attributes, handlers, events, panel, getStored: () => stored };
}

test("defaults preserve the real homepage and system theme", () => {
  const run = boot(null);
  assert.deepEqual({ ...run.attributes }, {
    torStyle: "showcase", torTheme: "system",
    torDensity: "normal", torCatalog: "grid"
  });
  run.handlers.DOMContentLoaded();
  assert.equal(run.panel.hidden, false);
});

test("persisted choices are applied before DOMContentLoaded", () => {
  const run = boot(JSON.stringify({
    style: "minimal", theme: "dark", density: "compact", catalog: "list"
  }));
  assert.equal(run.attributes.torStyle, "minimal");
  assert.equal(run.attributes.torTheme, "dark");
  assert.equal(run.attributes.torCatalog, "list");
});

test("unknown, malformed and hostile values are ignored", () => {
  for (const input of [
    "{", JSON.stringify({ style: "<img>", theme: "evil", density: 999 }),
    JSON.stringify(["minimal", "dark"])
  ]) {
    const run = boot(input);
    assert.equal(run.attributes.torStyle, "showcase");
    assert.equal(run.attributes.torTheme, "system");
  }
});

test("blocked localStorage cannot break page rendering", () => {
  const run = boot(null, true);
  assert.equal(run.attributes.torStyle, "showcase");
  run.handlers.DOMContentLoaded();
  assert.equal(run.panel.hidden, false);
});

test("a valid choice updates presentation, not the page content", () => {
  const run = boot(null);
  run.handlers.DOMContentLoaded();
  const button = { dataset: { torSetting: "theme", torValue: "light" } };
  run.events.click({ target: { closest() { return button; } } });
  assert.equal(run.attributes.torTheme, "light");
  assert.equal(JSON.parse(run.getStored()).theme, "light");
  const bad = { dataset: { torSetting: "theme", torValue: "<script>" } };
  run.events.click({ target: { closest() { return bad; } } });
  assert.equal(run.attributes.torTheme, "light");
});

test("library uses common color tokens across system and manual palettes", () => {
  const css = fs.readFileSync(
    path.join(__dirname, "../crates/web/assets/newboo-library.css"), "utf8"
  );
  const base = fs.readFileSync(
    path.join(__dirname, "../crates/web/assets/newboo-port.css"), "utf8"
  );
  const manual = fs.readFileSync(
    path.join(__dirname, "../crates/web/assets/tor-appearance.css"), "utf8"
  );
  const inherited = ["primary-color", "text-muted", "border-color",
    "text-main", "bg-surface", "bg-secondary"];
  for (const name of inherited) {
    assert(base.includes("--" + name + ":"), name + " requires a default");
    assert(!css.includes("var(--" + name + ","),
      name + " must not fall back to legacy green styling");
  }
  assert(css.includes("color: var(--tor-danger-ink)"));
  assert(!css.includes("color: #9d4039"));
  assert(base.includes("--tor-danger-ink: #9d4039;"));
  assert(base.includes("--tor-danger-ink: #fca5a5;"));
  const light = manual.split(':root[data-tor-theme="light"] {')[1].split("}")[0];
  const dark = manual.split(':root[data-tor-theme="dark"] {')[1].split("}")[0];
  assert(light.includes("--tor-danger-ink: #9d4039"));
  assert(dark.includes("--tor-danger-ink: #fca5a5"));
  // Illustrated book covers remain intentionally independent of UI tokens.
  assert(css.includes("linear-gradient(140deg, #315b4c"));
});
