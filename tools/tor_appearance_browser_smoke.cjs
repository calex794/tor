// UI-01: exercise real server-rendered pages and appearance controls in Chromium.
// Read-only presentation tests: never write a user account or open paid content.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const base = "http://127.0.0.1:3099";
const out = process.env.TOR_SCREENSHOT_DIR;
if (out) fs.mkdirSync(out, { recursive: true });

async function noOverflow(page, label) {
  const box = await page.evaluate(() => ({
    root: document.documentElement.scrollWidth,
    body: document.body.scrollWidth,
    viewport: document.documentElement.clientWidth
  }));
  assert(box.root <= box.viewport + 1 && box.body <= box.viewport + 1,
    label + ": horizontal overflow " + JSON.stringify(box));
}

async function open(page, pathname) {
  const response = await page.goto(base + pathname, { waitUntil: "networkidle" });
  assert(response && response.status() === 200, pathname + ": HTTP 200 required");
}

async function select(page, setting, value) {
  const picker = page.locator("[data-tor-appearance]");
  if ((await picker.getAttribute("open")) === null) {
    await picker.locator("summary").click();
  }
  const button = picker.locator('button[data-tor-setting="' + setting +
    '"][data-tor-value="' + value + '"]');
  await button.click();
  assert.equal(await button.getAttribute("aria-pressed"), "true");
  assert.equal(await page.locator("html").getAttribute("data-tor-" + setting), value);
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const width of [320, 390, 768, 1440]) {
      const page = await browser.newPage({
        viewport: { width, height: 900 }, deviceScaleFactor: 1, colorScheme: "dark"
      });
      await open(page, "/");
      const picker = page.locator("[data-tor-appearance]");
      assert(await picker.isVisible(), width + ": switcher missing");
      assert.equal(await page.locator("html").getAttribute("data-tor-theme"), "system");
      assert.equal((await page.context().cookies(base))
        .filter(c => c.name === "tor-ui-theme-v1").length, 0,
      width + ": anonymous system theme must not create a cookie");
      assert((await page.locator("a[href^='/books/']").count()) > 0,
        width + ": no real demo book links");
      const before = await page.locator("a[href^='/books/']")
        .evaluateAll(nodes => nodes.map(a => a.getAttribute("href")));
      await select(page, "theme", "light");
      assert.deepEqual((await page.context().cookies(base))
        .filter(c => c.name === "tor-ui-theme-v1").map(c => c.value),
      ["light"], width + ": explicit light theme must persist in validated cookie");
      await select(page, "style", "minimal");
      await select(page, "density", "compact");
      assert.equal(await page.locator("html").evaluate(el =>
        getComputedStyle(el).getPropertyValue("--bg-main").trim()), "#f7f7f8",
      width + ": explicit light must override OS dark");
      const after = await page.locator("a[href^='/books/']")
        .evaluateAll(nodes => nodes.map(a => a.getAttribute("href")));
      assert.deepEqual(after, before, width + ": changing presentation changed book links");
      await noOverflow(page, width + ": home minimal");
      if (out && (width === 390 || width === 1440)) {
        await page.screenshot({ path: path.join(out, "tor-ui-minimal-" + width + ".png"),
          fullPage: true });
      }
      await page.reload({ waitUntil: "networkidle" });
      assert.equal(await page.locator("html").getAttribute("data-tor-style"), "minimal",
        width + ": appearance not restored after reload");
      await open(page, "/catalog");
      const catalogBefore = await page.locator(".catalog-grid a[href^='/books/']")
        .evaluateAll(nodes => nodes.map(a => a.getAttribute("href")));
      assert(catalogBefore.length > 0, width + ": catalog missing books");
      await select(page, "catalog", "list");
      assert.equal(await page.locator(".catalog-grid .book-mini-card__link").first()
        .evaluate(el => getComputedStyle(el).display), "grid",
      width + ": list layout not applied");
      assert.deepEqual(await page.locator(".catalog-grid a[href^='/books/']")
        .evaluateAll(nodes => nodes.map(a => a.getAttribute("href"))),
      catalogBefore, width + ": catalog layout changed book destinations");
      await noOverflow(page, width + ": catalog list");
      await select(page, "theme", "dark");
      assert.deepEqual((await page.context().cookies(base))
        .filter(c => c.name === "tor-ui-theme-v1").map(c => c.value),
      ["dark"], width + ": explicit dark theme must replace the cookie");
      assert.equal(await page.locator("html").evaluate(el =>
        getComputedStyle(el).getPropertyValue("--bg-main").trim()), "#0b1120",
      width + ": explicit dark did not apply");
      await page.keyboard.press("Escape");
      assert((await page.locator("[data-tor-appearance]").getAttribute("open")) === null,
        width + ": Escape must close picker");
      if (out && (width === 390 || width === 1440)) {
        await page.screenshot({ path: path.join(out, "tor-ui-catalog-" + width + ".png"),
          fullPage: true });
      }
      await select(page, "theme", "system");
      assert.equal((await page.context().cookies(base))
        .filter(c => c.name === "tor-ui-theme-v1").length, 0,
      width + ": restoring system theme must clear appearance cookie");
      await page.close();
    }

    const context = await browser.newContext({ viewport: { width: 768, height: 900 } });
    const first = await context.newPage();
    const second = await context.newPage();
    await open(first, "/");
    await open(second, "/catalog");
    await select(first, "theme", "dark");
    await second.waitForFunction(() => document.documentElement.dataset.torTheme === "dark");
    await context.close();

    const noJs = await browser.newContext({ javaScriptEnabled: false,
      viewport: { width: 390, height: 844 } });
    const page = await noJs.newPage();
    await open(page, "/catalog");
    assert(!(await page.locator("[data-tor-appearance]").isVisible()),
      "No-JavaScript fallback must hide nonfunctional picker");
    assert((await page.locator(".catalog-grid a[href^='/books/']").count()) > 0,
      "No-JavaScript fallback must keep the real catalog");
    await noOverflow(page, "No-JavaScript catalog");
    await noJs.close();
    console.log("UI-01 Chromium PASS: 320/390/768/1440, preferences, OS override, links, no JS");
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
