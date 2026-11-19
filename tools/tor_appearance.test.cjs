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