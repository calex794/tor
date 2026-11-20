// Public appearance settings may mirror ONLY safe, fixed CSS enums to cookies.
// Script-free PostgreSQL book/library pages retain their original CSP.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const source = fs.readFileSync(
  path.join(__dirname, "../crates/web/assets/tor-appearance.js"), "utf8"
);

function cookieValues(raw) {
  return Object.fromEntries(raw.split(";").map(part => part.trim()).filter(Boolean)
    .map(part => {
      const i = part.indexOf("=");
      return [part.slice(0, i), part.slice(i + 1)];
    }));
}

function firstPaint(choice, protocol, initialCookie = "") {
  // A real browser replaces cookies by name; a concatenating string mock
  // falsely retains old values after appearance switches.
  const jar = new Map(Object.entries(cookieValues(initialCookie)));
  const writes = [];
  const handlers = {};
  const panelHandlers = {};
  const panel = {
    hidden: true,
    querySelectorAll() { return []; },
    addEventListener(type, fn) { panelHandlers[type] = fn; },
    contains() { return true; }
  };
  const document = {
    documentElement: { dataset: {} },
    addEventListener(type, fn) { handlers[type] = fn; },
    querySelector() { return panel; },
    set cookie(value) {
      writes.push(value);
      const pair = value.split(";")[0];
      const i = pair.indexOf("=");
      const name = pair.slice(0, i);
      const cookieValue = pair.slice(i + 1);
      if (value.includes("Max-Age=0")) jar.delete(name);
      else jar.set(name, cookieValue);
    },
    get cookie() {
      return [...jar.entries()].map(([key, value]) => key + "=" + value).join("; ");
    }
  };
  const window = {
    localStorage: {
      getItem() { return choice === null ? null : JSON.stringify(choice); },
      setItem() {}
    },
    location: { protocol },
    addEventListener(type, fn) { handlers["window:" + type] = fn; }
  };
  vm.runInNewContext(source, { window, document });
  return {
    get cookie() { return cookieValues(document.cookie); },
    get writes() { return [...writes]; },
    attrs: document.documentElement.dataset,
    select(setting, value) {
      handlers.DOMContentLoaded();
      panelHandlers.click({
        target: { closest() { return { dataset: {
          torSetting: setting, torValue: value
        } }; } }
      });
    }
  };
}

test("anonymous system theme creates no theme cookie; style remains a safe enum", () => {
  for (const protocol of ["http:", "https:"]) {
    const rendered = firstPaint(null, protocol);
    assert.deepEqual(rendered.cookie, { "tor-ui-style-v1": "showcase" });
    assert.equal(rendered.attrs.torTheme, "system");
    rendered.select("style", "minimal");
    assert.deepEqual(rendered.cookie, { "tor-ui-style-v1": "minimal" });
  }
});

test("explicit theme and style are independent, and system clears only theme", () => {
  for (const theme of ["light", "dark"]) {
    const rendered = firstPaint({ theme }, "https:");
    assert.deepEqual(rendered.cookie, {
      "tor-ui-theme-v1": theme, "tor-ui-style-v1": "showcase"
    });
    assert.equal(rendered.attrs.torTheme, theme);
  }
  const first = firstPaint(null, "https:");
  first.select("theme", "dark");
  assert.equal(first.cookie["tor-ui-theme-v1"], "dark");
  first.select("style", "library");
  assert.equal(first.cookie["tor-ui-style-v1"], "library");
  first.select("theme", "system");
  assert.deepEqual(first.cookie, { "tor-ui-style-v1": "library" });

  const invalid = firstPaint({
    theme: "<script>alert(1)</script>", style: "library\" onclick=\"alert(1)"
  }, "https:");
  assert.deepEqual(invalid.cookie, { "tor-ui-style-v1": "showcase" });
  assert.equal(invalid.attrs.torTheme, "system");
  assert.equal(invalid.attrs.torStyle, "showcase");
});

test("http and https only carry presentation enums and honor Secure", () => {
  const http = firstPaint({ theme: "dark", style: "minimal" }, "http:");
  assert.deepEqual(http.cookie, {
    "tor-ui-theme-v1": "dark", "tor-ui-style-v1": "minimal"
  });
  assert(!http.writes.some(write => write.includes("; Secure")));
  const https = firstPaint({ theme: "light", style: "library" }, "https:");
  assert.deepEqual(https.cookie, {
    "tor-ui-theme-v1": "light", "tor-ui-style-v1": "library"
  });
  assert(https.writes.every(write => write.includes("; Secure")));
  const stale = firstPaint(null, "https:", "tor-ui-theme-v1=dark");
  assert.deepEqual(stale.cookie, { "tor-ui-style-v1": "showcase" });
});
