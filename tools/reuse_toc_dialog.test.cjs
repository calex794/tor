// Exercise the actual mode-specific assets with the same local bundle
// boundary as reader_toc_assets::append. No fetches, browser packages or mocks
// of the shared implementation.
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const asset = name => readFileSync(
  path.join(__dirname, "../crates/web/assets", name), "utf8"
);
const shared = asset("newboo-toc-dialog.js");
const runtime = mode =>
  '(() => {\n"use strict";\n' + shared + "\n" +
  asset("newboo-" + mode + "-toc.js") + "\n})();\n";

function fixture(mode) {
  const events = [];
  const requests = [];
  const document = {
    activeElement: null, body: { style: { overflow: "" } },
    addEventListener(name, fn) { events.push({ name, fn }); },
    querySelector() { return mode === "paid" ? reader : root; }
  };
  function element(name, visible = true) {
    const handlers = new Map();
    return {
      name, dataset: {}, children: [], open: false,
      addEventListener(type, fn) { handlers.set(type, fn); },
      click() { handlers.get("click")(); },
      focus() { document.activeElement = this; },
      getClientRects() { return visible ? [{}] : []; },
      querySelectorAll() { return []; },
      querySelector() { return null; }
    };
  }
  const root = element("root");
  const trigger = element("summary");
  const close = element("close");
  const backdrop = element("backdrop");
  const modal = element("modal");
  const scroll = element("scroll");
  const current = element("current");
  const list = element("list");
  const status = element("status");
  const total = element("total");
  const firstLink = element("first link");
  const hiddenLink = element("hidden link", false);
  const lastLink = element("last link");
  const outside = element("outside");
  modal.querySelectorAll = selector => {
    assert.equal(selector, "a[href]");
    return [firstLink, hiddenLink, lastLink];
  };
  modal.contains = node => [close, scroll, firstLink, hiddenLink, lastLink].includes(node);
  Object.assign(root.dataset, {
    freeTocBook: "1", freeTocChapter: "2",
    archiveTocChapter: "2", archivePaidAvailable: "1"
  });
  root.querySelector = selector => ({
    summary: trigger,
    "[data-free-toc-list]": list,
    "[data-free-toc-scroll]": scroll,
    "[data-free-toc-status]": status,
    "[data-free-toc-total]": total,
    "[data-free-toc-close]": close,
    "[data-free-toc-backdrop]": backdrop,
    "[data-free-toc-modal]": modal,
    "[data-paid-toc-modal]": modal,
    "[data-paid-toc-scroll]": scroll,
    ".reader-paid__toc-list": list,
    "[data-paid-toc-close]": close,
    "[data-paid-toc-backdrop]": backdrop,
    '[data-toc-state="current"]': current,
    "[data-archive-toc-modal]": modal,
    "[data-archive-toc-scroll]": scroll,
    ".archive-toc__list": list,
    ".archive-toc__row--current": current,
    "[data-archive-toc-close]": close,
    "[data-archive-toc-backdrop]": backdrop,
    "[data-archive-toc-status]": status
  })[selector] || null;
  const reader = element("paid reader");
  Object.assign(reader.dataset, { chapterId: "2", purchaseId: "3" });
  reader.querySelectorAll = () => [root];
  const sandbox = { document, fetch: (...args) => { requests.push(args); } };
  vm.runInNewContext(runtime(mode), sandbox, { timeout: 1000 });
  assert.equal(events.filter(event => event.name === "keydown").length, 1);
  assert.equal(sandbox.bindTocDialog, undefined, "no global helper");
  function key(key, shiftKey = false) {
    let prevented = false;
    for (const event of events.filter(e => e.name === "keydown")) {
      event.fn({ key, shiftKey, preventDefault() { prevented = true; } });
    }
    return prevented;
  }
  return {
    root, trigger, close, backdrop, scroll, firstLink, lastLink, outside,
    document, requests, key
  };
}

for (const mode of ["free", "paid", "archive"]) {
  test(mode + ": closed dialog ignores keyboard; Escape and clicks restore focus", () => {
    const f = fixture(mode);
    f.outside.focus();
    assert.equal(f.key("Escape"), false);
    assert.equal(f.document.activeElement, f.outside);
    for (const dismiss of [() => f.key("Escape"), () => f.close.click(),
      () => f.backdrop.click()]) {
      f.root.open = true;
      f.lastLink.focus();
      dismiss();
      assert.equal(f.root.open, false);
      assert.equal(f.document.activeElement, f.trigger);
    }
    assert.deepEqual(f.requests, []);
  });

  test(mode + ": Tab wraps only at the edges or when focus escapes", () => {
    const f = fixture(mode);
    f.root.open = true;
    f.close.focus();
    assert.equal(f.key("Tab", true), true);
    assert.equal(f.document.activeElement, f.lastLink);
    assert.equal(f.key("Tab"), true);
    assert.equal(f.document.activeElement, f.close);
    f.outside.focus();
    assert.equal(f.key("Tab"), true);
    assert.equal(f.document.activeElement, f.close);
    f.outside.focus();
    assert.equal(f.key("Tab", true), true);
    assert.equal(f.document.activeElement, f.lastLink);
    f.firstLink.focus();
    assert.equal(f.key("Tab"), false);
    assert.equal(f.key("Tab", true), false);
    assert.equal(f.key("Enter"), false);
    assert.equal(f.document.activeElement, f.firstLink);
    assert.deepEqual(f.requests, []);
  });
}

test("no visible focus targets: Tab does not trap focus", () => {
  const document = { activeElement: null, addEventListener(_, fn) { this.key = fn; } };
  const invisible = { getClientRects: () => [], addEventListener() {} };
  const modal = { querySelectorAll: () => [], contains: () => false };
  const root = { open: true };
  vm.runInNewContext(shared +
    "\nbindTocDialog(root, trigger, modal, invisible, invisible, []);",
    { document, root, trigger: { focus() {} }, modal, invisible });
  document.key({ key: "Tab", shiftKey: false, preventDefault() {
    assert.fail("no visible focus targets");
  } });
});
