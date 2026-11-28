"use strict";
// Real PG+MinIO book, real Axum response, only public numeric IDs.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const { createHash } = require("node:crypto");
const path = require("node:path");
const base = "http://127.0.0.1:3119";
const integrity = "sha256-" + createHash("sha256").update(
  fs.readFileSync(path.join(__dirname, "../crates/web/assets/tor-auth-dialog.js"))
).digest("base64");
function id(v) { assert.match(v || "", /^[1-9]\d*$/); return v; }
function pair(v) {
  const items = (v || "").split(",");
  assert.equal(items.length, 2); return items.map(id);
}
const book = id(process.env.TOR_UI10B_BOOK_ID);
const paid = pair(process.env.TOR_UI10B_PAID_IDS);
const free = pair(process.env.TOR_UI10B_FREE_IDS);
const out = process.env.TOR_PAID_READER_SHOTS ||
  path.join(process.env.RUNNER_TEMP || "/tmp", "tor-paid-reader-visual");
fs.mkdirSync(out, { recursive: true });

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const width of [390, 1440]) {
      let canonical = null;
      const shapes = [];
      for (const style of ["showcase", "library", "minimal"]) {
        for (const theme of ["light", "dark"]) {
          const ctx = await browser.newContext({
            viewport: { width, height: 900 }, deviceScaleFactor: 1,
            javaScriptEnabled: false, colorScheme: theme
          });
          try {
            // Cookies are explicit, validated presentation enums, NOT identity.
            await ctx.addCookies([
              { name: "tor-ui-style-v1", value: style, url: base, sameSite: "Lax" },
              { name: "tor-ui-theme-v1", value: theme, url: base, sameSite: "Lax" },
            ]);
            const page = await ctx.newPage();
            const response = await page.goto(base + "/books/" + book, { waitUntil: "load" });
            assert(response && response.status() === 200);
            const csp = (await response.allHeaders())["content-security-policy"] || "";
            assert(csp.includes("script-src '" + integrity + "';") &&
              csp.includes("connect-src 'self';") &&
              !csp.includes("script-src 'self'") && !csp.includes("unsafe-inline"));
            assert.equal(await page.locator('script[src^="/assets/tor-auth-dialog.js"]').getAttribute('integrity'), integrity);
            assert.equal(await page.locator("html").getAttribute("data-tor-style"), style);
            assert.equal(await page.locator("html").getAttribute("data-tor-theme"), theme);
            assert.equal(await page.locator("h1#book-title").count(), 1);
            assert.equal(await page.locator(".book-hero-card__cover-col").count(), 1);
            const paidRows = page.locator(".chapter-toc__item--paid");
            const freeRows = page.locator(".chapter-toc__item:not(.chapter-toc__item--paid)");
            assert(await paidRows.count() >= 2 && await freeRows.count() >= 2);
            for (const ch of paid)
              assert.equal(await paidRows.locator('a[href="/reader/' + ch + '"]').count(), 1);
            for (const ch of free)
              assert.equal(await freeRows.locator('a[href="/reader/' + ch + '"]').count(), 1);
            const read = page.locator('.book-hero-actions a[href^="/reader/"]').first();
            assert.equal(await read.count(), 1, "free read action lost");
            const titleBox = await page.locator("#book-title").boundingBox();
            const ctaBox = await read.boundingBox();
            assert(titleBox && titleBox.y >= 0 && titleBox.y < 900);
            assert(ctaBox && ctaBox.y >= 0 && ctaBox.y + ctaBox.height <= 900);
            const size = await page.evaluate(() => ({
              root: document.documentElement.scrollWidth,
              body: document.body.scrollWidth,
              client: document.documentElement.clientWidth
            }));
            assert(size.root <= size.client + 1 && size.body <= size.client + 1,
              "horizontal overflow");
            const links = await page.locator(".book-detail-page a[href]")
              .evaluateAll(nodes => nodes.map(n => n.getAttribute("href")));
            if (!canonical) canonical = links;
            assert.deepEqual(links, canonical, "layout changed real chapter links");
            const html = await page.content();
            for (const secret of [
              "Second paid edition content", "New V2, not purchased",
              "dek_b64u", "X-Amz-Signature", "data-paid-chapter-reader",
              "<img src=x onerror="
            ]) assert(!html.includes(secret), "public HTML leaked paid data");
            const shape = await page.locator(".book-hero-card").evaluate(el => {
              const c = getComputedStyle(el);
              return c.gridTemplateAreas + "|" + c.gridTemplateColumns + "|" + c.borderTopWidth;
            });
            if (theme === "light") shapes.push(shape);
            await page.screenshot({
              path: path.join(out, "ui10b-mixed-" + width + "-" + style + "-" + theme + ".png"),
              fullPage: true
            });
          } finally { await ctx.close(); }
        }
      }
      assert.equal(new Set(shapes).size, 3, "all layouts must differ");
    }
    console.log("UI-10B MIXED PUBLIC PASS");
  } finally { await browser.close(); }
})().catch(() => {
  console.error("UI-10B MIXED PUBLIC FAIL (private diagnostics withheld)");
  process.exitCode = 1;
});
