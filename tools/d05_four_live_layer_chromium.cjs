// D-05/#1022: one deleted Book, one historic Layer session, four real
// chapter bytes (2 frozen FREE + 2 sealed PAID) via Axum/PG18/MinIO/Chromium.
// This file must be invoked by the real Rust fixture, never a mock HTTP server.
// No private sessions, bearer URLs, storage object keys or text in CI output.
"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");

const SITE = "http://127.0.0.1:3098";
const S3 = "http://127.0.0.1:9000";
const temp = process.env.RUNNER_TEMP || "";
const name = process.env.TOR_D05_FOUR_LIVE_FIXTURE || "";
assert(temp && name && path.resolve(name).startsWith(path.resolve(temp) + path.sep),
  "D05 four-chapter fixture must stay inside isolated runner temp");
assert.equal(fs.statSync(name).mode & 0o077, 0, "private session fixture must be 0600");
const fixture = JSON.parse(fs.readFileSync(name, "utf8"));
assert(typeof fixture.historical_session === "string" && fixture.historical_session.length > 20);
assert(typeof fixture.outsider_session === "string" && fixture.outsider_session.length > 20);
assert(Array.isArray(fixture.chapters) && fixture.chapters.length === 4);
assert.deepEqual(fixture.chapters.map(x => x.kind), ["free", "free", "paid", "paid"]);
const ids = new Set();
for (const row of fixture.chapters) {
  assert(Number.isSafeInteger(row.id) && row.id > 0);
  assert(Number.isSafeInteger(row.version) && row.version > 0);
  assert(typeof row.expected_text === "string" && row.expected_text.length);
  assert(!ids.has(row.id), "one real Book must contain four unique chapters");
  ids.add(row.id);
}
const blocks = text => text.replace(/\r\n?/g, "\n").split(/\n\s*\n/).filter(s => s.trim());

async function readOldBuyer(browser, viewport, label) {
  const context = await browser.newContext({ viewport, serviceWorkers: "block" });
  try {
    await context.addCookies([{ url: SITE, name: "tor-dev-session",
      value: fixture.historical_session, httpOnly: true, sameSite: "Lax" }]);
    const page = await context.newPage();
    for (const entry of fixture.chapters) {
      const route = entry.kind === "free" ? "/read/layer/free/" : "/read/layer/";
      const url = SITE + route + entry.id;
      const traffic = { grant: 0, signedS3: 0, publicReader: 0, leakedS3: 0 };
      const onRequest = request => {
        const target = new URL(request.url());
        if (target.origin === SITE && (target.pathname.startsWith("/reader/") ||
            target.pathname.startsWith("/api/v1/chapters/"))) traffic.publicReader++;
        if (target.origin === SITE && (target.searchParams.has("X-Amz-Signature") ||
            target.pathname.startsWith("/private/"))) traffic.leakedS3++;
      };
      const onResponse = response => {
        const target = new URL(response.url());
        const grant = entry.kind === "free"
          ? "/api/v1/reader/layer/free/" + entry.id + "/grant"
          : "/api/v1/reader/chapters/" + entry.id + "/delivery-grants";
        if (target.origin === SITE && target.pathname === grant &&
            response.request().method() === "POST" && response.status() === 200)
          traffic.grant++;
        if (target.origin === S3 && response.request().method() === "GET" &&
            response.status() === 200 && target.searchParams.has("X-Amz-Signature"))
          traffic.signedS3++;
      };
      page.on("request", onRequest);
      page.on("response", onResponse);
      const response = await page.goto(url, { waitUntil: "domcontentloaded" });
      assert.equal(response.status(), 200, "historic Layer buyer must read all four chapters");
      const html = await response.text();
      assert(!html.includes(entry.expected_text), "shell must never embed private chapter bytes");
      assert(!html.includes("X-Amz-Signature"), "shell must not embed signed S3 bearers");
      const selector = entry.kind === "free" ? "[data-layer-free-" : "[data-layer-";
      const status = page.locator(selector + "status]");
      const success = entry.kind === "free" ? "Архивная редакция" : "Защищённая загрузка завершена";
      await status.filter({ hasText: success }).waitFor({ timeout: 30000 });
      const content = page.locator(selector + "content] p");
      const received = await content.allTextContents();
      assert.deepEqual(received, blocks(entry.expected_text),
        "exact frozen edition required; neither neighbor nor current replacement");
      assert.equal(await page.locator(selector + "content] script, " +
        selector + "content] iframe").count(), 0, "private text is never executable markup");
      assert.equal(traffic.publicReader, 0, "no public fallback for deleted Book");
      assert.equal(traffic.leakedS3, 0, "storage path must never leak on same origin");
      assert.equal(traffic.grant, 1, "one real actor-authorized grant per chapter");
      assert(traffic.signedS3 >= 1, "real protected object must be opened in loopback MinIO");
      page.off("request", onRequest);
      page.off("response", onResponse);
    }
    assert.equal(await page.evaluate(() => localStorage.length), 0);
    assert.equal(await page.evaluate(() => sessionStorage.length), 0);
    console.log("D05 FOUR LIVE PASS: " + label);
  } finally {
    await context.close();
  }
}

async function denied(browser, session) {
  const context = await browser.newContext({ serviceWorkers: "block" });
  try {
    if (session) await context.addCookies([{ url: SITE, name: "tor-dev-session",
      value: session, httpOnly: true, sameSite: "Lax" }]);
    const page = await context.newPage();
    let s3 = 0;
    page.on("request", request => { if (new URL(request.url()).origin === S3) s3++; });
    for (const entry of fixture.chapters) {
      const route = entry.kind === "free" ? "/read/layer/free/" : "/read/layer/";
      const response = await page.goto(SITE + route + entry.id, { waitUntil: "domcontentloaded" });
      assert(session ? [403, 404].includes(response.status()) : response.status() === 401);
      assert.equal(await page.locator("[data-layer-content], [data-layer-free-content]").count(), 0);
    }
    assert.equal(s3, 0, "outsiders must never fetch private bytes");
  } finally {
    await context.close();
  }
}

async function main() {
  const browser = await chromium.launch({ headless: true });
  try {
    await readOldBuyer(browser, { width: 1440, height: 900 }, "desktop");
    await readOldBuyer(browser, { width: 390, height: 844 }, "mobile");
    await denied(browser, fixture.outsider_session);
    await denied(browser, null);
  } finally {
    await browser.close();
  }
}
main().catch(() => {
  // Errors are redacted: never log signed S3 URL, session, or chapter body.
  console.error("D05 FOUR LIVE FAIL");
  process.exitCode = 1;
});
