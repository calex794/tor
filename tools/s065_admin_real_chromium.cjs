// S065: REAL PostgreSQL/Axum/MinIO -> Chromium WebCrypto administrator reader.
// No mock routes, purchase, stored plaintext, bearer URL, DEK or cookie in logs.
"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");
const { execFileSync } = require("node:child_process");
const { chromium } = require("playwright");

const SITE = "http://127.0.0.1:3098";
const S3 = "http://127.0.0.1:9000";
const temp = process.env.RUNNER_TEMP || "";
const fixtureFile = process.env.TOR_S065_ADMIN_BROWSER_FIXTURE || "";
assert(temp && fixtureFile && path.resolve(fixtureFile).startsWith(
  path.resolve(temp) + path.sep), "only disposable runner fixtures allowed");
const fixture = JSON.parse(fs.readFileSync(fixtureFile, "utf8"));
const { admin_id, admin_session, outsider_session, chapter_id, version_id,
  historical_version_id, asset_object_id } = fixture;
for (const id of [admin_id, chapter_id, version_id, historical_version_id]) {
  assert(Number.isSafeInteger(id) && id > 0, "valid fixture ID");
}
assert.notEqual(historical_version_id, version_id, "historical V1 and current V2 must differ");
assert.match(admin_session, /^[A-Za-z0-9_-]{43}$/);
assert.match(outsider_session, /^[A-Za-z0-9_-]{43}$/);
assert.match(asset_object_id, /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);

const database = process.env.TEST_DATABASE_URL || "";
const parsed = new URL(database);
assert.equal(parsed.protocol, "postgresql:");
assert.equal(parsed.hostname, "127.0.0.1");
assert.equal(parsed.port, "5432");
assert.equal(parsed.pathname, "/tor_ci");
assert.equal(parsed.username, "tor_app");
function pg(sql) {
  return execFileSync("psql", [
    "-X", "-q", "-A", "-t", "-v", "ON_ERROR_STOP=1",
    "--dbname", database, "--command", sql,
  ], { encoding: "utf8", timeout: 20000 }).trim();
}
function count(sql) {
  const output = pg(sql);
  assert.match(output, /^[0-9]+$/);
  return Number(output);
}
function assertNoMoney() {
  assert.equal(count("SELECT count(*) FROM chapter_purchase_facts WHERE user_id=" +
    admin_id), 0, "administrator must not obtain a paid purchase");
  assert.equal(count("SELECT count(*) FROM reader_wallet_entries WHERE user_id=" +
    admin_id), 0, "administrator must not create a wallet journal entry");
}

const readerPath = "/read/admin/" + chapter_id;
const chapterGrant = "/api/v1/reader/chapters/" + chapter_id + "/delivery-grants";
const assetGrant = "/api/v1/reader/chapters/" + chapter_id +
  "/assets/" + asset_object_id + "/delivery-grants";
const historyList = "/api/v1/reader/chapters/" + chapter_id + "/admin-sealed-history";
const historyChapter = historyList + "/" + historical_version_id + "/delivery-grants";
const historyImage = historyList + "/" + historical_version_id +
  "/assets/" + asset_object_id + "/delivery-grants";
async function contextFor(browser, session = null) {
  const ctx = await browser.newContext();
  if (session) {
    await ctx.addCookies([{ url: SITE, name: "tor-dev-session",
      value: session, httpOnly: true, sameSite: "Strict" }]);
  }
  return ctx;
}
async function postAdminGrant(ctx, endpoint, csrf) {
  return ctx.request.post(SITE + endpoint, {
    data: { source: "administrator" },
    headers: { origin: SITE, "x-csrf-token": csrf },
  });
}
async function denied(ctx, status) {
  const csrf = crypto.randomBytes(32).toString("base64url");
  await ctx.addCookies([{ url: SITE, name: "tor-dev-csrf",
    value: csrf, sameSite: "Strict" }]);
  const page = await ctx.request.get(SITE + readerPath);
  assert.equal(page.status(), status, "admin shell requires active role");
  assert((page.headers()["cache-control"] || "").includes("no-store"));
  assert.equal((await page.body()).length, 0, "denied shell must be empty");
  for (const endpoint of [chapterGrant, assetGrant,
    historyList, historyChapter, historyImage]) {
    const response = await postAdminGrant(ctx, endpoint, csrf);
    assert.equal(response.status(), status, "role-based grant denial");
    assert((response.headers()["cache-control"] || "").includes("no-store"));
    assert.equal((await response.body()).length, 0,
      "denied request cannot contain a signed S3 bearer");
  }
}
async function positive(browser, viewport, device) {
  const ctx = await contextFor(browser, admin_session);
  // Put the viewport on the same context that owns the real auth session.
  const page = await ctx.newPage();
  await page.setViewportSize(viewport);
  const observed = { textGrant: 0, imageGrant: 0, chapterS3: 0, imageS3: 0,
    historicalTextGrant: 0, historicalImageGrant: 0, progressWrites: 0 };
  page.on("request", (request) => {
    if (request.url().startsWith(SITE + "/api/v1/me/progress/purchased") &&
        request.method() !== "GET") observed.progressWrites++;
  });
  page.on("response", (response) => {
    const uri = new URL(response.url());
    if (uri.origin === SITE && response.status() === 200 &&
        response.request().method() === "POST") {
      if (uri.pathname === chapterGrant) observed.textGrant++;
      if (uri.pathname === assetGrant) observed.imageGrant++;
      if (uri.pathname === historyChapter) observed.historicalTextGrant++;
      if (uri.pathname === historyImage) observed.historicalImageGrant++;
    }
    if (uri.origin === S3 && response.status() === 200 &&
        response.request().method() === "GET") {
      if (uri.pathname.includes("/private/chapter-versions/")) observed.chapterS3++;
      if (uri.pathname.includes("/private/chapter-assets/" + asset_object_id + ".enc")) {
        observed.imageS3++;
      }
    }
  });
  try {
    const response = await page.goto(SITE + readerPath, {
      waitUntil: "domcontentloaded",
    });
    assert.equal(response.status(), 200, "real admin SSR");
    assert((response.headers()["cache-control"] || "").includes("no-store"));
    assert.equal(response.headers()["referrer-policy"], "no-referrer");
    assert.equal(response.headers()["x-robots-tag"], "noindex");
    const shell = await response.text();
    for (const secret of ["Оплаченная V1: кириллица", "ciphertext_url",
      "dek_b64u", "X-Amz-Signature", "data-purchase-id"]) {
      assert(!shell.includes(secret), "admin SSR must never expose private material");
    }

    try {
      await page.locator("[data-admin-status]").filter({
        hasText: "Защищённая загрузка завершена",
      }).waitFor({ timeout: 30000 });
    } catch {
      const safeStatus = (await page.locator("[data-admin-status]").textContent() || "");
      const safeOutcome = {
        status: safeStatus.startsWith("Административный доступ недоступен.")
          ? "private loader denied" : "protected loader incomplete",
        textGrant: observed.textGrant,
        imageGrant: observed.imageGrant,
        chapterS3: observed.chapterS3,
        imageS3: observed.imageS3,
        progressWrites: observed.progressWrites,
      };
      throw new Error("S065 admin reader did not reach protected state: " +
        JSON.stringify(safeOutcome));
    }
    assert((await page.locator("[data-admin-status]").textContent())
      .includes("Редакция " + version_id));
    const article = page.locator("[data-admin-content]");
    assert((await article.textContent()).includes("New V2, not purchased"),
      "canonical administrator reader must open CURRENT sealed V2");
    assert.equal(await article.locator(".reader-content__image-frame").count(), 0,
      "V2 has no image; historical V1 image must not leak into current edition");
    const history = page.locator('select[aria-label="История зашифрованных редакций"]');
    await history.waitFor({ timeout: 30000 });
    assert.equal(await history.locator("option").count(), 2,
      "only V1 and V2 sealed editions are attached");
    await history.selectOption(String(historical_version_id));
    await page.locator("[data-admin-status]").filter({
      hasText: "Редакция " + historical_version_id + " · Защищённый просмотр",
    }).waitFor({ timeout: 30000 });
    assert((await article.textContent()).includes("Оплаченная V1: кириллица"),
      "historical V1 must decrypt through its own scoped S3 grant");
    assert(!(await article.textContent()).includes("New V2, not purchased"),
      "switching edition must clear previous decrypted chapter");
    assert.equal(await article.locator(".reader-save-button").count(), 0,
      "admin inspection is not personal reading progress");
    const frame = article.locator(".reader-content__image-frame");
    assert.equal(await frame.count(), 1, "one B-attached private PNG");
    await frame.scrollIntoViewIfNeeded();
    await page.waitForFunction(() => {
      const image = document.querySelector(
        "[data-admin-content] .reader-content__image-frame img");
      return image && image.complete && image.naturalWidth > 0 && image.naturalHeight > 0;
    }, null, { timeout: 30000 });
    const image = await frame.locator("img").evaluate((node) => ({
      src: node.getAttribute("src"), alt: node.alt, referrer: node.referrerPolicy,
    }));
    assert(image.src.startsWith("blob:"), "decrypted PNG must stay in memory");
    assert.equal(image.alt, "Рисунок платной главы");
    assert.equal(image.referrer, "no-referrer");
    const html = await article.innerHTML();
    assert(!html.includes("asset:") && !html.includes("X-Amz-Signature") &&
      !html.includes("ciphertext_url"), "rendered HTML cannot contain bearer metadata");
    assert.deepEqual(observed, {
      textGrant: 1, imageGrant: 0, chapterS3: 2, imageS3: 1,
      historicalTextGrant: 1, historicalImageGrant: 1, progressWrites: 0,
    }, "current V2 and historical V1/PNG require their own real signed grants, no progress writes");
    assertNoMoney();

    await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
    assert.equal(await article.textContent(), "",
      "pagehide must erase the decrypted text and detach private blob URLs");
    await ctx.close();
    return device;
  } finally {
    // An earlier assertion must still destroy the authenticated context.
    if (page && !page.isClosed()) await ctx.close();
  }
}
let stage = "bootstrap";
async function main() {
  const browser = await chromium.launch({ headless: true });
  try {
    stage = "guest and outsider denial";
    const guest = await contextFor(browser);
    try { await denied(guest, 401); } finally { await guest.close(); }
    const outsider = await contextFor(browser, outsider_session);
    try { await denied(outsider, 403); } finally { await outsider.close(); }

    stage = "desktop real admin WebCrypto";
    await positive(browser, { width: 1440, height: 900 }, "desktop");
    // C-12: an active site administrator can inspect a B/G-attached
    // edition even when both the book is hidden and moderation-held.
    // This is NOT a bypass for ordinary buyers or unsealed drafts.
    stage = "mobile hidden and held admin WebCrypto";
    const bookId = count("SELECT book_id FROM chapters WHERE id=" + chapter_id);
    assert(Number.isSafeInteger(bookId) && bookId > 0);
    pg("UPDATE books SET visibility='draft',admin_hold=TRUE WHERE id=" + bookId);
    try {
      await positive(browser, { width: 390, height: 844 }, "mobile");
    } finally {
      // This is a disposable, test-owned book and must not pollute the
      // remaining S034 buyer and Layer policy tests on this same tor_ci.
      pg("UPDATE books SET visibility='public',admin_hold=FALSE WHERE id=" + bookId);
    }

    // The REAL active admin still has to present a valid same-origin CSRF
    // command. A purchase ID cannot be smuggled through the admin source.
    stage = "administrator CSRF, Origin and purchase spoofing denial";
    const privileged = await contextFor(browser, admin_session);
    try {
      const csrf = crypto.randomBytes(32).toString("base64url");
      await privileged.addCookies([{ url: SITE, name: "tor-dev-csrf",
        value: csrf, httpOnly: true, sameSite: "Strict" }]);
      const attempt = (data, origin, token) => privileged.request.post(
        SITE + chapterGrant, {
          data,
          headers: { origin, "x-csrf-token": token },
        });
      for (const endpoint of [chapterGrant, assetGrant,
    historyList, historyChapter, historyImage]) {
        const forbiddenOrigin = await privileged.request.post(SITE + endpoint, {
          data: { source: "administrator" },
          headers: { origin: "https://foreign.invalid", "x-csrf-token": csrf },
        });
        assert.equal(forbiddenOrigin.status(), 403);
        assert.equal((await forbiddenOrigin.body()).length, 0);
        const invalidCsrf = await privileged.request.post(SITE + endpoint, {
          data: { source: "administrator" },
          headers: { origin: SITE, "x-csrf-token": "invalid" },
        });
        assert.equal(invalidCsrf.status(), 403);
        assert.equal((await invalidCsrf.body()).length, 0);
      }
      const forgedPurchase = await attempt({
        source: "administrator", purchase_id: "1",
      }, SITE, csrf);
      assert.equal(forgedPurchase.status(), 422);
      assert.equal((await forgedPurchase.body()).length, 0);
    } finally {
      await privileged.close();
    }

    stage = "unsealed history desktop and mobile";
    await require("./s065_admin_history_chromium.cjs")({
      browser, SITE, admin_id, admin_session, outsider_session,
      chapter_id, pg, count, contextFor, assertNoMoney,
    });

    // Separate mixed-current regression: an unsealed working V3 cannot
    // authorize historical sealed V1 except via the administrator-only path.
    stage = "sealed V1 history from current unsealed draft on real MinIO";
    await require("./s065_admin_sealed_history_chromium.cjs")({
      browser, SITE, S3, admin_id, admin_session, outsider_session,
      chapter_id, version_id: historical_version_id, asset_object_id,
      pg, count, contextFor, assertNoMoney,
    });

    stage = "revoked admin denial";
    pg("UPDATE account_roles SET is_active=FALSE WHERE user_id=" +
      admin_id + " AND role='administrator'");
    const revoked = await contextFor(browser, admin_session);
    try { await denied(revoked, 403); } finally { await revoked.close(); }
    assertNoMoney();
    console.log("S065 real admin Chromium PASS: desktop, mobile, revoke");
  } finally {
    await browser.close();
    // The Rust Drop guard repeats cleanup if Node exits before revocation.
  }
}
main().catch((error) => {
  // Do not print raw URLs, browser snapshots, private bodies or bearer details.
  console.error("S065 real admin Chromium failed at " + stage +
    " (" + error.name + ")");
  process.exitCode = 1;
});
