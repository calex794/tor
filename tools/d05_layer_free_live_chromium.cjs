// #1022: real Chromium -> live Rust private Layer FREE route -> signed loopback
// MinIO GET. Rust fixture is created exclusively on disposable PG18 + MinIO.
// Do not print grant URLs, session tokens, private object keys or body bytes.
"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");

const SITE = "http://127.0.0.1:3098";
const S3 = "http://127.0.0.1:9000";
const temp = process.env.RUNNER_TEMP || "";
const filename = process.env.TOR_D05_FREE_CHROMIUM_FIXTURE || "";
assert(temp && filename &&
  path.resolve(filename).startsWith(path.resolve(temp) + path.sep),
  "D05 real Chromium fixture must be inside disposable CI directory");
const fixture = JSON.parse(fs.readFileSync(filename, "utf8"));
assert(Number.isSafeInteger(fixture.chapter) && fixture.chapter > 0);
assert(Number.isSafeInteger(fixture.version) && fixture.version > 0);
assert(typeof fixture.buyer_session === "string" && fixture.buyer_session.length > 20);
assert(typeof fixture.outsider_session === "string" && fixture.outsider_session.length > 20);
assert(typeof fixture.expected_text === "string" && fixture.expected_text.length > 0);
const privatePage = SITE + "/read/layer/free/" + fixture.chapter;
const privateGrant = "/api/v1/reader/layer/free/" + fixture.chapter + "/grant";
const sourcePath = "/private/free-chapters/" + fixture.chapter +
  "/" + fixture.version + ".md";

async function verifyBuyer(browser, label, viewport) {
  const context = await browser.newContext({ viewport, serviceWorkers: "block" });
  const seen = { grant: 0, object: 0, publicPaths: 0, signedOnSite: 0 };
  try {
    await context.addCookies([{ url: SITE, name: "tor-dev-session",
      value: fixture.buyer_session, httpOnly: true, sameSite: "Lax" }]);
    const page = await context.newPage();
    page.on("request", request => {
      const target = new URL(request.url());
      if (target.origin === SITE &&
          (target.pathname.startsWith("/reader/") ||
           target.pathname.startsWith("/api/v1/chapters/"))) seen.publicPaths++;
      if (target.origin === SITE && target.searchParams.has("X-Amz-Signature"))
        seen.signedOnSite++;
    });
    page.on("response", response => {
      const target = new URL(response.url());
      if (target.origin === SITE && target.pathname === privateGrant &&
          response.request().method() === "POST" && response.status() === 200)
        seen.grant++;
      if (target.origin === S3 && target.pathname.endsWith(sourcePath) &&
          response.request().method() === "GET" && response.status() === 200 &&
          /^[a-f0-9]{64}$/.test(target.searchParams.get("X-Amz-Signature") || ""))
        seen.object++;
    });
    const reply = await page.goto(privatePage, { waitUntil: "domcontentloaded" });
    assert.equal(reply.status(), 200, "historical buyer must get private shell");
    const shell = await reply.text();
    assert(shell.includes("data-layer-free-reader"));
    assert(!shell.includes(fixture.expected_text), "shell must not carry plaintext");
    assert(!shell.includes("X-Amz-Signature"), "shell must not carry bearer URL");
    await page.locator("[data-layer-free-status]").filter({
      hasText: "Архивная редакция",
    }).waitFor({ timeout: 30000 });
    const article = page.locator("[data-layer-free-content]");
    assert.equal(await article.textContent(), fixture.expected_text,
      "Chromium must render the exact immutable archived FREE edition");
    assert.equal(await article.locator("script, iframe, img").count(), 0,
      "archived Markdown must be rendered as inert text");
    assert.equal(await page.evaluate(() => globalThis.__layerFreeXss), undefined);
    assert.deepEqual(seen, { grant: 1, object: 1, publicPaths: 0, signedOnSite: 0 },
      "browser must use D-checked grant then direct signed MinIO GET only");
    // No plaintext or signed S3 URL is put in browser-side storage.
    assert.equal(await page.evaluate(() => localStorage.length), 0);
    assert.equal(await page.evaluate(() => sessionStorage.length), 0);
    console.log("D05 LIVE FREE Chromium PASS: " + label);
  } finally {
    await context.close();
  }
}

async function verifyDenied(browser, session, expectedStatus) {
  const context = await browser.newContext({ serviceWorkers: "block" });
  try {
    if (session) await context.addCookies([{ url: SITE, name: "tor-dev-session",
      value: session, httpOnly: true, sameSite: "Lax" }]);
    const page = await context.newPage();
    let privateObjectRequests = 0;
    page.on("request", request => {
      if (new URL(request.url()).origin === S3) privateObjectRequests++;
    });
    const response = await page.goto(privatePage, { waitUntil: "domcontentloaded" });
    assert.equal(response.status(), expectedStatus);
    assert.equal(await page.locator("[data-layer-free-content]").count(), 0);
    assert.equal(privateObjectRequests, 0);
  } finally {
    await context.close();
  }
}

async function main() {
  const browser = await chromium.launch({ headless: true });
  try {
    await verifyBuyer(browser, "desktop", { width: 1440, height: 900 });
    await verifyBuyer(browser, "mobile", { width: 390, height: 844 });
    await verifyDenied(browser, fixture.outsider_session, 404);
    await verifyDenied(browser, null, 401);
  } finally {
    await browser.close();
  }
}

main().catch(() => {
  // Treat all browser errors as private. Never print signed URLs, text or cookies.
  console.error("D05 LIVE FREE Chromium FAIL");
  process.exitCode = 1;
});
