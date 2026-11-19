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
function firstPaint(choice, protocol) {
  let cookie = "";
  const document = {
    documentElement: { dataset: {} },
    addEventListener() {},
    set cookie(value) { cookie = value; },
    get cookie() { return cookie; }
  };
  const window = {
    localStorage: {
      getItem() { return JSON.stringify(choice); },
      setItem() {}
    },
    location: { protocol },
    addEventListener() {}
  };
  vm.runInNewContext(source, { window, document });
  return { cookie, attrs: document.documentElement.dataset };
}

test("only allowlisted theme reaches non-sensitive cookie", () => {
  for (const theme of ["light", "dark", "system"]) {
    const rendered = firstPaint({ theme }, "https:");
    assert.match(rendered.cookie, new RegExp("^tor-ui-theme-v1=" + theme + ";"));
    assert.match(rendered.cookie, /; Path=\/; Max-Age=31536000; SameSite=Lax; Secure$/);
    assert.equal(rendered.attrs.torTheme, theme);
  }
  const unknown = firstPaint({ theme: "<script>alert(1)</script>" }, "https:");
  assert.match(unknown.cookie, /^tor-ui-theme-v1=system;/);
  assert(!unknown.cookie.includes("<script"));
});
test("loopback fallback does not require Secure and private CSS remains usable", () => {
  const rendered = firstPaint({ theme: "dark" }, "http:");
  assert.match(rendered.cookie, /SameSite=Lax$/);
  assert(!rendered.cookie.includes("Secure"));
  assert.equal(rendered.attrs.torTheme, "dark");
});
