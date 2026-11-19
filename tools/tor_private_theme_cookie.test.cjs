// UI-03A: the client cookie is purely an allowlisted visual preference.
// The private /me/library page itself keeps script-src 'none'.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const source = fs.readFileSync(
  path.join(__dirname, "../crates/web/assets/tor-appearance.js"), "utf8"
);

function firstPaint(choice, protocol, initialCookie = "") {
  let cookie = initialCookie;
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
      if (value.includes("Max-Age=0")) {
        cookie = cookie.split(";").map(v => v.trim())
          .filter(v => !v.startsWith("tor-ui-theme-v1=")).join("; ");
      } else {
        const pair = value.split(";")[0];
        cookie = cookie ? cookie + "; " + pair : pair;
      }
    },
    get cookie() { return cookie; }
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
    get cookie() { return cookie; },
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

test("anonymous default system theme creates no cookie", () => {
  for (const protocol of ["http:", "https:"]) {
    const rendered = firstPaint(null, protocol);
    assert.equal(rendered.cookie, "");
    assert.equal(rendered.attrs.torTheme, "system");
    rendered.select("style", "minimal");
    assert.equal(rendered.cookie, "", "layout alone must not create a cookie");
  }
});

test("only explicit or persisted non-system theme reaches cookie", () => {
  for (const theme of ["light", "dark"]) {
    const rendered = firstPaint({ theme }, "https:");
    assert.match(rendered.cookie, new RegExp("^tor-ui-theme-v1=" + theme + "$"));
    assert.equal(rendered.attrs.torTheme, theme);
  }
  const first = firstPaint(null, "https:");
  first.select("theme", "dark");
  assert.match(first.cookie, /^tor-ui-theme-v1=dark$/);
  first.select("theme", "system");
  assert.equal(first.cookie, "", "switching to system clears stale cookie");

  const invalid = firstPaint({ theme: "<script>alert(1)</script>" }, "https:");
  assert.equal(invalid.cookie, "");
  assert.equal(invalid.attrs.torTheme, "system");
});

test("http and https both support preferences without leaking identity", () => {
  const light = firstPaint({ theme: "dark" }, "http:");
  assert.equal(light.cookie, "tor-ui-theme-v1=dark");
  assert.equal(light.attrs.torTheme, "dark");
  const stale = firstPaint(null, "https:", "tor-ui-theme-v1=dark");
  assert.equal(stale.cookie, "", "stale cookie without local preferences cleared");
});
