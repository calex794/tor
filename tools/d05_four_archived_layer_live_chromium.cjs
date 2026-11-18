// D-05/#1022: ONE historical Layer buyer reads 2 immutable S3 FREE and
// 2 encrypted S3 PAID chapters from the SAME deleted Book in actual Chromium.
// The fixture, S3 and Axum exist only on disposable CI loopback.
// Never print secrets, object keys, signed URLs, cookies or chapter bodies.
"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");

const SITE = "http://127.0.0.1:3098";
const S3 = "http://127.0.0.1:9000";
const filename = process.env.TOR_D05_FOUR_CHROMIUM_FIXTURE || "";
const temp = process.env.RUNNER_TEMP || "";
assert(filename && temp &&
  path.resolve(filename).startsWith(path.resolve(temp) + path.sep),
  "D05 four-chapter fixture must remain in disposable runner temp");
const fixture = JSON.parse(fs.readFileSync(filename, "utf8"));
assert(typeof fixture.old_session === "string" && fixture.old_session.length > 20);
assert(typeof fixture.new_session === "string" && fixture.new_session.length > 20);
assert(Array.isArray(fixture.chapters) && fixture.chapters.length === 4);
assert.deepEqual(fixture.chapters.map(c => c.kind), ["free", "free", "paid", "paid"]);
assert(new Set(fixture.chapters.map(c => c.id)).size === 4);
for (const item of fixture.chapters) {
  assert(Number.isSafeInteger(item.id) && item.id > 0);
  assert(typeof item.expected === "string" && item.expected.length > 10);
}
const route = item => "/read/layer/" +
  (item.kind === "free" ? "free/" : "") + item.id;

async function oldBuyer(browser) {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 }, serviceWorkers: "block",
  });
  try {
    await context.addCookies([{
      url: SITE, name: "tor-dev-session", value: fixture.old_session,
      httpOnly: true, sameSite: "Lax",
    }]);
    const page = await context.newPage();
    let minio200 = 0;
    let publicFallbacks = 0;
    let exposedSiteSignatures = 0;
    const grants = new Map();
    page.on("request", request => {
      const target = new URL(request.url());
      if (target.origin === SITE &&
          (target.pathname.startsWith("/reader/") ||
           target.pathname.startsWith("/api/v1/chapters/"))) publicFallbacks++;
      if (target.origin === SITE &&
          (target.searchParams.has("X-Amz-Signature") ||
           target.pathname.includes("/private/free-chapters/") ||
           target.pathname.includes("/private/paid/"))) exposedSiteSignatures++;
    });
    page.on("response", response => {
      const target = new URL(response.url());
      if (target.origin === S3 && response.request().method() === "GET" &&
          response.status() === 200 &&
          /^[a-f0-9]{64}$/.test(target.searchParams.get("X-Amz-Signature") || ""))
        minio200++;
      if (target.origin === SITE && response.request().method() === "POST" &&
          response.status() === 200 && (
            target.pathname.endsWith("/grant") ||
            target.pathname.endsWith("/pack") ||
            target.pathname.endsWith("/delivery-grants")
          )) grants.set(target.pathname, (grants.get(target.pathname) || 0) + 1);
    });
    for (const item of fixture.chapters) {
      const before = minio200;
      const response = await page.goto(SITE + route(item), {
        waitUntil: "domcontentloaded",
      });
      assert.equal(response.status(), 200, "historical private shell required");
      const html = await response.text();
      assert(!html.includes(item.expected), "private shell must exclude chapter plaintext");
      assert(!html.includes("X-Amz-Signature"), "shell cannot leak signed S3 URL");
      assert(!html.includes("dek_b64u"), "shell cannot leak decryption key");
      if (item.kind === "free") {
        assert(html.includes("data-layer-free-reader"));
        await page.locator("[data-layer-free-status]").filter({
          hasText: "Архивная редакция",
        }).waitFor({ timeout: 30000 });
        assert.equal(await page.locator("[data-layer-free-content]").textContent(),
          item.expected, "frozen FREE text must match verified S3 bytes");
        assert.equal(await page.locator("[data-layer-free-content] script, iframe").count(), 0);
      } else {
        assert(html.includes("data-layer-chapter-reader"));
        await page.locator("[data-layer-status]").filter({
          hasText: "Защищённая загрузка завершена",
        }).waitFor({ timeout: 30000 });
        assert((await page.locator("[data-layer-content]").textContent())
          .includes(item.expected), "encrypted PAID V1/V2 must decrypt only for old buyer");
        assert.equal(await page.locator("[data-purchase-id]").count(), 0,
          "Layer term must not mint a direct pinned purchase");
      }
      assert(minio200 > before,
        "each of four independently protected chapters requires its own real MinIO GET");
    }
    assert.equal(grants.size, 4,
      "two FREE grants and two PAID grants must use four chapter-scoped endpoints");
    assert.equal(publicFallbacks, 0, "deleted chapters must not use public reader fallback");
    assert.equal(exposedSiteSignatures, 0, "site requests cannot contain bearer S3 keys");
    console.log("D05 FOUR LIVE Chromium PASS");
  } finally {
    await context.close();
  }
}

async function newBuyer(browser) {
  const context = await browser.newContext({ serviceWorkers: "block" });
  try {
    await context.addCookies([{
      url: SITE, name: "tor-dev-session", value: fixture.new_session,
      httpOnly: true, sameSite: "Lax",
    }]);
    const page = await context.newPage();
    let s3Requests = 0;
    page.on("request", r => {
      if (new URL(r.url()).origin === S3) s3Requests++;
    });
    for (const item of fixture.chapters) {
      const response = await page.goto(SITE + route(item), {
        waitUntil: "domcontentloaded",
      });
      assert.equal(response.status(), 404,
        "new buyer must not receive deleted chapter metadata or private shell");
      assert.equal(await page.locator(
        "[data-layer-free-content], [data-layer-content]").count(), 0);
    }
    assert.equal(s3Requests, 0, "new buyer cannot reach private object delivery");
  } finally {
    await context.close();
  }
}

async function main() {
  const browser = await chromium.launch({ headless: true });
  try {
    await oldBuyer(browser);
    await newBuyer(browser);
  } finally {
    await browser.close();
  }
}
main().catch(() => {
  // Fail closed; logs must not expose session, grants or chapter bodies.
  console.error("D05 FOUR LIVE Chromium FAIL");
  process.exitCode = 1;
});
