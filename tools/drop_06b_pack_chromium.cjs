// DROP-06b: real Chromium execution of the production FREE S3/PACK script.
// Network is deterministic and intercepted; ACL and MinIO bytes are proved
// separately by the PG18/MinIO integration test in the same CI suite.
"use strict";
const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const { chromium } = require("playwright");

const ORIGIN = "http://127.0.0.1:3098";
const S3 = "https://s3.example.invalid";
const script = readFileSync(join(__dirname, "../crates/web/assets/newboo-free-s3-reader.js"), "utf8");
const markdown = "# Глава PACK 😀\n\nПервый абзац 中文\n\nВторой абзац для прогресса.";
const bytes = Buffer.from(markdown, "utf8");
const hash = createHash("sha256").update(bytes).digest("hex");
const signedUrl = S3 + "/bucket/private/free-chapters/101/201.md" +
  "?X-Amz-Expires=90&X-Amz-Signature=" + "a".repeat(64);

const doc = saved => `<!doctype html>
<html lang="ru"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>DROP-06b reader</title><style>
body{margin:0;padding:16px;font-family:system-ui,sans-serif}
article{max-width:860px;margin:auto;overflow-wrap:anywhere}
</style></head><body><section data-free-reader>
<article id="chapter-text" data-free-s3-chapter="101" data-free-s3-version="201"
 data-free-s3-csrf="${"x".repeat(43)}" data-free-pack-reader="1"
 ${saved ? 'data-free-s3-saved="1" data-free-s3-archive="1"' : ""}
 ><p role="status">Загружаем текст главы…</p></article>
</section><script defer src="/assets/newboo-free-s3-reader.js"></script></body></html>`;

async function scenario(browser, width, { saved = false, transport = "gateway", packStatus = 200,
  corrupted = false, deniedPlan = false } = {}) {
  const context = await browser.newContext({ viewport: { width, height: 800 }, deviceScaleFactor: 1 });
  const page = await context.newPage();
  const requests = [];
  try {
    await page.route("**/*", async route => {
      const url = new URL(route.request().url());
      requests.push(url.pathname + url.search);
      if (url.origin === ORIGIN && url.pathname === "/") {
        return route.fulfill({ status: 200, contentType: "text/html", body: doc(saved) });
      }
      if (url.origin === ORIGIN && url.pathname === "/assets/newboo-free-s3-reader.js") {
        return route.fulfill({ status: 200, contentType: "application/javascript", body: script });
      }
      if (url.origin === ORIGIN && url.pathname.startsWith("/assets/newboo-") &&
          url.pathname.endsWith(".js")) {
        return route.fulfill({ status: 200, contentType: "application/javascript", body: "" });
      }
      if (url.origin === ORIGIN && url.pathname.endsWith("/free-source-plan")) {
        assert.equal(url.search, saved ? "?mode=saved&version_id=201" : "");
        return route.fulfill({
          status: deniedPlan ? 404 : 200, contentType: "application/json",
          body: JSON.stringify({
            schema: "tor.free-source-plan.v1", chapter_id: "101", version_id: "201",
            transport, sha256: hash, size_bytes: bytes.length
          })
        });
      }
      if (url.origin === ORIGIN && url.pathname.endsWith("/free-pack-source")) {
        assert.equal(transport, "gateway");
        assert.equal(url.search, saved ? "?mode=saved&version_id=201" : "");
        return route.fulfill({
          status: packStatus,
          headers: {
            "content-type": "text/plain; charset=utf-8",
            "x-tor-free-version-id": "201",
            "x-tor-free-sha256": hash,
            "x-tor-free-size": String(bytes.length)
          },
          body: corrupted ? Buffer.from("X".repeat(bytes.length), "utf8") : bytes
        });
      }
      if (url.origin === ORIGIN && url.pathname.endsWith("/free-grant")) {
        assert.equal(transport, "signed-single");
        return route.fulfill({
          status: 200, contentType: "application/json",
          body: JSON.stringify({
            schema: "tor.free-s3-grant.v1", chapter_id: "101", version_id: "201",
            url: signedUrl, sha256: hash, size_bytes: bytes.length
          })
        });
      }
      if (url.origin === S3 && url.pathname.endsWith("/101/201.md")) {
        return route.fulfill({
          status: 200, headers: { "content-type": "text/plain", "access-control-allow-origin": "*" },
          body: bytes
        });
      }
      return route.fulfill({ status: 404, body: "no synthetic route" });
    });
    await page.goto(ORIGIN + "/", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => {
      const article = document.querySelector("#chapter-text");
      return article?.textContent?.includes("Глава PACK") ||
        !!article?.querySelector('[role="alert"]');
    }, { timeout: 15000 });
    const state = await page.evaluate(() => {
      const article = document.querySelector("#chapter-text");
      return {
        text: article.textContent,
        error: !!article.querySelector('[role="alert"]'),
        version: article.dataset.freeProgressVersion,
        manifest: article.dataset.freeProgressBlocks,
        horizontalOverflow: document.documentElement.scrollWidth > window.innerWidth + 1
      };
    });
    const expectedFail = packStatus !== 200 || corrupted || deniedPlan;
    assert.equal(state.error, expectedFail, "unexpected render state at viewport " + width);
    assert.equal(state.horizontalOverflow, false, "horizontal overflow at " + width);
    if (!expectedFail) {
      assert.match(state.text, /Глава PACK/);
      assert.match(state.text, /Второй абзац/);
      assert.equal(state.version, "201");
      assert.ok(state.manifest.includes("0:"));
    } else {
      assert.equal(state.version, undefined, "failed source must not initialize progress");
    }
    assert.ok(requests.some(path => path.endsWith("/free-source-plan" + (saved ? "?mode=saved&version_id=201" : ""))));
    if (transport === "gateway" || deniedPlan) {
      assert.ok(!requests.some(path => path.includes("/free-grant")), "PACK must never fall back to SINGLE");
      assert.ok(!requests.some(path => path.endsWith("/101/201.md" + new URL(signedUrl).search)));
    }
    if (deniedPlan) assert.ok(!requests.some(path => path.includes("free-pack-source")));
    console.log("DROP06B_CHROMIUM_PASS", JSON.stringify({
      width, saved, transport, deniedPlan, corrupted, packStatus, requestCount: requests.length
    }));
  } finally {
    await context.close();
  }
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    await scenario(browser, 1440);
    await scenario(browser, 390);
    await scenario(browser, 390, { saved: true });
    await scenario(browser, 1440, { transport: "signed-single" });
    await scenario(browser, 390, { packStatus: 403 });
    await scenario(browser, 1440, { corrupted: true });
    await scenario(browser, 390, { saved: true, deniedPlan: true });
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
