// D-05/#1022: live Axum + PG18 + exact authenticated PACK frame + real
// Chromium. The PACK and sidecar exist only in disposable loopback MinIO.
// Never log signed URLs, keys, sessions, raw chapter contents or cookies.
"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");

const SITE = "http://127.0.0.1:3098";
const S3 = "http://127.0.0.1:9000";
const temp = process.env.RUNNER_TEMP || "";
const filename = process.env.TOR_D05_PACK_CHROMIUM_FIXTURE || "";
assert(temp && filename && path.resolve(filename).startsWith(path.resolve(temp) + path.sep),
  "PACK Chromium fixture must be inside disposable runner temp");
const fixture = JSON.parse(fs.readFileSync(filename, "utf8"));
assert(Number.isSafeInteger(fixture.chapter) && fixture.chapter > 0);
assert(Number.isSafeInteger(fixture.version) && fixture.version > 0);
assert(typeof fixture.buyer_session === "string" && fixture.buyer_session.length > 20);
assert(typeof fixture.outsider_session === "string" && fixture.outsider_session.length > 20);
assert(typeof fixture.expected_text === "string" && fixture.expected_text.length > 0);
const route = "/read/layer/free/" + fixture.chapter;
const single = "/api/v1/reader/layer/free/" + fixture.chapter + "/grant";
const pack = "/api/v1/reader/layer/free/" + fixture.chapter + "/pack";
const expectedBlocks = fixture.expected_text.replace(/\r\n?/g, "\n")
  .split(/\n\s*\n/).filter(block => block.trim());

async function authorized(browser, label, viewport) {
  const context = await browser.newContext({ viewport, serviceWorkers: "block" });
  try {
    await context.addCookies([{ url: SITE, name: "tor-dev-session",
      value: fixture.buyer_session, httpOnly: true, sameSite: "Lax" }]);
    const page = await context.newPage();
    const seen = { single404: 0, pack200: 0, externalS3: 0,
      publicFallback: 0, leakedPackUrl: 0 };
    page.on("request", request => {
      const target = new URL(request.url());
      if (target.origin === S3) seen.externalS3++;
      if (target.origin === SITE && (
        target.pathname.startsWith("/reader/") ||
        target.pathname.startsWith("/api/v1/chapters/"))) seen.publicFallback++;
      if (target.searchParams.has("X-Amz-Signature") ||
          target.pathname.includes("/private/free-packs/")) seen.leakedPackUrl++;
    });
    page.on("response", response => {
      const target = new URL(response.url());
      if (target.origin === SITE && target.pathname === single &&
          response.request().method() === "POST" && response.status() === 404)
        seen.single404++;
      if (target.origin === SITE && target.pathname === pack &&
          response.request().method() === "POST" && response.status() === 200)
        seen.pack200++;
    });
    const response = await page.goto(SITE + route, { waitUntil: "domcontentloaded" });
    assert.equal(response.status(), 200);
    const shell = await response.text();
    assert(shell.includes("data-layer-free-reader"));
    assert(!shell.includes(fixture.expected_text), "shell must exclude plaintext");
    assert(!shell.includes("X-Amz-Signature"));
    assert(!shell.includes("/private/free-packs/"));
    await page.locator("[data-layer-free-status]").filter({
      hasText: "Архивная редакция",
    }).waitFor({ timeout: 30000 });
    const blocks = await page.locator("[data-layer-free-content] p").allTextContents();
    assert.deepEqual(blocks, expectedBlocks,
      "browser must receive exactly the frozen PACK chapter, no adjacent frame");
    assert.equal(await page.locator("[data-layer-free-content] script, [data-layer-free-content] iframe").count(), 0);
    assert.deepEqual(seen, { single404: 1, pack200: 1, externalS3: 0,
      publicFallback: 0, leakedPackUrl: 0 });
    assert.equal(await page.evaluate(() => localStorage.length), 0);
    assert.equal(await page.evaluate(() => sessionStorage.length), 0);
    console.log("D05 LIVE PACK Chromium PASS: " + label);
  } finally {
    await context.close();
  }
}

async function rejected(browser, session, expectedStatus) {
  const context = await browser.newContext({ serviceWorkers: "block" });
  try {
    if (session) await context.addCookies([{ url: SITE, name: "tor-dev-session",
      value: session, httpOnly: true, sameSite: "Lax" }]);
    const page = await context.newPage();
    let forbiddenTransport = 0;
    page.on("request", request => {
      const target = new URL(request.url());
      if (target.origin === S3 || target.pathname.endsWith("/pack")) forbiddenTransport++;
    });
    const response = await page.goto(SITE + route, { waitUntil: "domcontentloaded" });
    assert.equal(response.status(), expectedStatus);
    assert.equal(await page.locator("[data-layer-free-content]").count(), 0);
    assert.equal(forbiddenTransport, 0);
  } finally {
    await context.close();
  }
}

async function main() {
  const browser = await chromium.launch({ headless: true });
  try {
    await authorized(browser, "desktop", { width: 1440, height: 900 });
    await authorized(browser, "mobile", { width: 390, height: 844 });
    await rejected(browser, fixture.outsider_session, 404);
    await rejected(browser, null, 401);
  } finally {
    await browser.close();
  }
}

main().catch(() => {
  // The failure is intentionally redacted: no session, S3 URL, or markdown logs.
  console.error("D05 LIVE PACK Chromium FAIL");
  process.exitCode = 1;
});
