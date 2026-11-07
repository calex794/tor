// Real loopback Rust/PG/MinIO: paywall -> explicit single purchase -> private
// reader -> canonical revocation -> denial. Never print credentials or text.
"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const { chromium } = require("playwright");
const SITE = "http://127.0.0.1:3098";
const temp = process.env.RUNNER_TEMP || "";
const file = process.env.TOR_S034_REAL_BROWSER_FIXTURE || "";
assert(temp && file && path.resolve(file).startsWith(path.resolve(temp) + path.sep));
const fixture = JSON.parse(fs.readFileSync(file, "utf8"));
const db = process.env.TEST_DATABASE_URL || "";
const target = new URL(db);
assert.equal(target.protocol, "postgresql:");
assert.equal(target.hostname, "127.0.0.1");
assert.equal(target.port, "5432");
assert.equal(target.pathname, "/tor_ci");
assert.equal(target.username, "tor_app");
function pg(sql) {
  return execFileSync("psql", ["-X", "-q", "-A", "-t", "-v", "ON_ERROR_STOP=1",
    "--dbname", db, "--command", sql], { encoding: "utf8", timeout: 20000 }).trim();
}
function number(sql) {
  const value = pg(sql);
  assert.match(value, /^[0-9]+$/);
  return Number(value);
}
async function verify(browser, buyer, viewport) {
  const chapter = fixture.chapters[0];
  for (const id of [buyer.id, chapter.id, chapter.version]) assert(Number.isSafeInteger(id) && id > 0);
  const context = await browser.newContext({ viewport });
  const page = await context.newPage();
  const assetId = fixture.asset_object_id;
  assert.match(assetId, /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
  const assetGrantPath = "/api/v1/reader/chapters/" + chapter.id +
    "/assets/" + assetId + "/delivery-grants";
  let assetGrants = 0, assetCiphertexts = 0;
  page.on("response", (reply) => {
    if (reply.url().startsWith(SITE + assetGrantPath) &&
        reply.request().method() === "POST" && reply.status() === 200) assetGrants++;
    if (reply.url().startsWith("http://127.0.0.1:9000/") &&
        reply.url().includes("/private/chapter-assets/" + assetId + ".enc") &&
        reply.request().method() === "GET" && reply.status() === 200) assetCiphertexts++;
  });
  try {
    // A guest sees metadata only, and cannot submit another person's purchase.
    const guest = await context.request.get(SITE + "/me/chapters/" + chapter.id + "/buy",
      { maxRedirects: 0 });
    assert.equal(guest.status(), 303);
    assert.equal(guest.headers().location, "/login");
    await context.addCookies([{ url: SITE, name: "tor-dev-session", value: buyer.session,
      httpOnly: true, sameSite: "Lax" }]);
    const paywall = await page.goto(SITE + "/reader/" + chapter.id);
    assert.equal(paywall.status(), 200);
    assert(!(await paywall.text()).includes("Оплаченная V1: кириллица"));
    await page.getByRole("link", { name: "Купить главу", exact: true }).click();
    await page.waitForURL("**/buy");
    const form = await page.locator('form[action$="/buy"]').evaluate((element) =>
      Object.fromEntries(new FormData(element)));
    assert.equal(form.expected_version_id, String(chapter.version));
    const price = Number(form.expected_price_minor);
    assert(price > 0 && price < 600);
    const route = SITE + "/me/chapters/" + chapter.id + "/buy";
    const post = (data, origin = SITE) => context.request.post(route,
      { form: data, headers: { origin }, maxRedirects: 0 });
    const balance = () => number("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=" + buyer.id);
    const purchases = () => number("SELECT count(*) FROM chapter_purchase_facts WHERE user_id=" + buyer.id);
    assert.equal(balance(), 600);
    assert.equal(purchases(), 0);
    assert.equal((await post({ ...form, csrf_token: "invalid" })).status(), 403);
    assert.equal((await post(form, "https://foreign.example")).status(), 403);
    assert.equal((await post({ ...form, actor_id: String(buyer.id) })).status(), 422);
    assert.equal((await post({ ...form, expected_price_minor: price + 1 })).status(), 409);
    assert.equal((await post({ ...form, expected_version_id: String(chapter.version + 1) })).status(), 409);
    assert.equal(purchases(), 0);
    // Fixture funding only; no public endpoint pretends to accept real money.
    pg("UPDATE reader_wallet_accounts SET balance_minor=0 WHERE user_id=" + buyer.id);
    assert.equal((await post(form)).status(), 409);
    assert.equal(balance(), 0);
    assert.equal(purchases(), 0);
    pg("UPDATE reader_wallet_accounts SET balance_minor=600 WHERE user_id=" + buyer.id);

    const submitted = page.waitForResponse((r) => r.url() === route && r.request().method() === "POST");
    // Race the identical command against an actual click in the native form.
    const duplicate = post(form);
    await page.getByRole("button", { name: "Подтвердить покупку за", exact: false }).click();
    assert.equal((await submitted).status(), 303);
    assert.equal((await duplicate).status(), 303);
    await page.waitForURL("**/read/paid/**");
    await page.waitForFunction(() => [...document.querySelectorAll("[data-paid-content] .reader-piece__text")]
      .some((node) => (node.textContent || "").includes("Оплаченная V1: кириллица")), null, { timeout: 30000 });
    // Full-stack S065: Rust B/D/C authorizes a real V1 image, MinIO returns
    // its sealed ciphertext, and Chromium WebCrypto renders an in-memory PNG.
    const frame = page.locator(".reader-paid__image-block .reader-content__image-frame");
    assert.equal(await frame.count(), 1, "exactly one B-attached V1 figure");
    await frame.scrollIntoViewIfNeeded();
    try {
      await page.waitForFunction(() => {
        const frame = document.querySelector(".reader-paid__image-block .reader-content__image-frame");
        const img = frame?.querySelector("img");
        return frame?.classList.contains("is-error") ||
          (img && img.complete && img.naturalWidth === 1 && img.naturalHeight === 1);
      }, null, { timeout: 30000 });
    } catch (error) {
      // Never log ciphertext URLs, purchase/session IDs, DEKs or paid content.
      const media = await frame.evaluate((node) => ({
        failed: node.classList.contains("is-error"),
        displayed: Boolean(node.querySelector("img")),
        status: (node.textContent || "").slice(0, 120),
      }));
      throw new Error("S065 image did not settle: " + JSON.stringify({
        media, assetGrants, assetCiphertexts,
      }) + "; " + error.message);
    }
    const imageFailed = await frame.evaluate((node) => node.classList.contains("is-error"));
    assert(!imageFailed, "S065 private image loader rejected: " + JSON.stringify({
      status: (await frame.textContent()).slice(0, 120),
      assetGrants, assetCiphertexts,
    }));
    const image = await frame.locator("img").evaluate((img) => ({
      src: img.getAttribute("src"), alt: img.alt, policy: img.referrerPolicy,
    }));
    assert(image.src.startsWith("blob:"), "private image must not expose a signed S3 URL");
    assert.equal(image.alt, "Рисунок платной главы");
    assert.equal(image.policy, "no-referrer");
    assert.equal(assetGrants, 1, "one real authenticated image grant");
    assert.equal(assetCiphertexts, 1, "one real encrypted MinIO image GET");
    const privateHtml = await page.locator("[data-paid-content]").innerHTML();
    assert(!privateHtml.includes("X-Amz-Signature"));
    assert(!privateHtml.includes("asset:"), "raw private media marker must be replaced");
    assert.equal(balance(), 600 - price);
    assert.equal(purchases(), 1);
    const purchase = number("SELECT id FROM chapter_purchase_facts WHERE user_id=" + buyer.id);
    assert.equal(new URL(page.url()).searchParams.get("purchase_id"), String(purchase));
    // The owner must see the exact pinned paid edition in the real library.
    const purchasedHref = "/read/paid/" + chapter.id + "?purchase_id=" + purchase;
    const ownLibrary = await context.request.get(SITE + "/me/purchases");
    assert.equal(ownLibrary.status(), 200);
    assert.equal(ownLibrary.headers()["cache-control"], "no-store");
    assert((await ownLibrary.text()).includes('href="' + purchasedHref + '"'));
    assert.equal(number("SELECT count(*) FROM reader_wallet_entries WHERE user_id=" + buyer.id +
      " AND kind='chapter_purchase'"), 1);

    // E-12/S047: a REAL reader selection from decrypted private S3 content.
    const issues = page.locator("[data-paid-issues] form");
    await issues.waitFor({ state: "visible" });
    const selectedQuote = await page
      .locator("[data-paid-content] .reader-piece__text")
      .filter({ hasText: "Оплаченная V1: кириллица" }).first()
      .evaluate((paragraph) => {
        const walker = document.createTreeWalker(paragraph, NodeFilter.SHOW_TEXT);
        let node;
        while ((node = walker.nextNode())) {
          const offset = (node.textContent || "").indexOf("Оплаченная");
          if (offset < 0) continue;
          const range = document.createRange();
          range.setStart(node, offset);
          range.setEnd(node, offset + "Оплаченная".length);
          const selection = window.getSelection();
          selection.removeAllRanges();
          selection.addRange(range);
          document.dispatchEvent(new Event("selectionchange"));
          return "Оплаченная";
        }
        throw new Error("No selectable paid text node");
      });
    assert.equal(await issues.locator('textarea[name="quote"]').inputValue(), selectedQuote);
    const suggestion = "Проверьте написание слова в купленной редакции.";
    await issues.locator('textarea[name="suggested_text"]').fill(suggestion);
    const issueButton = issues.locator('button[type="submit"]');
    const issueCount = () => number("SELECT count(*) FROM text_issues WHERE reporter_user_id=" +
      buyer.id + " AND purchase_id=" + purchase + " AND chapter_id=" + chapter.id);
    await issueButton.click();
    await page.waitForFunction(() => document.querySelector('[data-paid-issues] [role="status"]')
      ?.textContent === "Сообщение отправлено владельцу книги.");
    assert.equal(issueCount(), 1);
    await issueButton.click();
    await page.waitForFunction(() => document.querySelector('[data-paid-issues] [role="status"]')
      ?.textContent === "Такое сообщение уже отправлялось.");
    assert.equal(issueCount(), 1, "identical correction replay must not duplicate issue");

    // S011: this is the live Rust + MinIO paid reader, not a mocked
    // discussion view. The edition is decrypted before comment UI mounts.
    const comments = page.locator("#paid-chapter-comments");
    await comments.waitFor({state:"visible"});
    const commentCount = () => number("SELECT count(*) FROM discussion_comments WHERE " +
      "author_user_id=" + buyer.id + " AND chapter_id=" + chapter.id +
      " AND version_id=" + chapter.version);
    assert.equal(commentCount(), 0);
    await comments.getByLabel("Текст комментария").fill("Вопрос по купленной редакции");
    await comments.getByRole("button", {name:"Отправить"}).click();
    await page.waitForFunction(() => document.querySelectorAll(
      "#paid-chapter-comments .comment-card").length === 1);
    assert.equal(commentCount(), 1);
    assert.equal(number("SELECT count(*) FROM discussion_anchors a JOIN discussion_comments c " +
      "ON c.id=a.comment_id WHERE c.author_user_id=" + buyer.id +
      " AND c.version_id=" + chapter.version), 1,
      "selected paid quote must be anchored to the exact purchased version");
    await comments.getByRole("button", {name:"Ответить"}).first().click();
    await comments.getByLabel("Текст комментария").fill("Ответ к той же редакции");
    await comments.getByRole("button", {name:"Отправить"}).click();
    await page.waitForFunction(() => document.querySelectorAll(
      "#paid-chapter-comments .comment-card").length === 2);
    assert.equal(commentCount(), 2);
    assert.equal(number("SELECT count(*) FROM discussion_comments WHERE " +
      "author_user_id=" + buyer.id + " AND chapter_id=" + chapter.id +
      " AND parent_id IS NOT NULL"), 1);

    assert.equal((await post(form)).status(), 303);
    assert.equal(balance(), 600 - price);
    assert.equal(purchases(), 1);

    // S065: a different authenticated account cannot read the purchased
    // edition, fetch its delivery grant, or inspect its progress.
    const outsider = await browser.newContext();
    try {
      assert.match(fixture.outsider_session, /^[a-zA-Z0-9_-]{43}$/);
      await outsider.addCookies([{ url: SITE, name: "tor-dev-session",
        value: fixture.outsider_session, httpOnly: true, sameSite: "Strict" }]);
      const secret = "Оплаченная V1: кириллица";
      const privateUrl = SITE + "/read/paid/" + chapter.id + "?purchase_id=" + purchase;
      const denied = await outsider.request.get(privateUrl);
      assert.equal(denied.status(), 403);
      assert(!(await denied.text()).includes(secret));
      // Test the mounted grant API with valid Origin and CSRF, so any denial
      // comes from purchase ownership rather than a missing route or CSRF.
      const csrf = require("node:crypto").randomBytes(32).toString("base64url");
      assert.equal(csrf.length, 43);
      await outsider.addCookies([{ url: SITE, name: "tor-dev-csrf",
        value: csrf, httpOnly: true, sameSite: "Strict" }]);
      const deniedGrant = await outsider.request.post(
        SITE + "/api/v1/reader/chapters/" + chapter.id + "/delivery-grants",
        { data: { purchase_id: String(purchase) },
          headers: { origin: SITE, "x-csrf-token": csrf } });
      assert.equal(deniedGrant.status(), 403);
      assert.equal(deniedGrant.headers()["cache-control"], "no-store");
      assert.equal((await deniedGrant.body()).length, 0);
      const deniedAsset = await outsider.request.post(SITE + assetGrantPath, {
        data: { purchase_id: String(purchase) },
        headers: { origin: SITE, "x-csrf-token": csrf },
      });
      assert.equal(deniedAsset.status(), 403);
      assert.equal(deniedAsset.headers()["cache-control"], "no-store");
      assert.equal((await deniedAsset.body()).length, 0);
      // /me/purchases is a real entitlement-backed route. Neither an outsider
      // nor a guessed keyset cursor may reveal the buyer's pinned edition.
      for (const path of ["/me/purchases", "/me/purchases?after=" + purchase]) {
        const foreignLibrary = await outsider.request.get(SITE + path);
        assert.equal(foreignLibrary.status(), 200);
        assert.equal(foreignLibrary.headers()["cache-control"], "no-store");
        const html = await foreignLibrary.text();
        assert(html.includes("Доступных покупок пока нет"));
        assert(!html.includes('href="' + purchasedHref + '"'));
      }
      const progress = SITE + "/api/v1/me/progress/purchased?purchase_id=" +
        purchase + "&chapter_id=" + chapter.id + "&version_id=" + chapter.version;
      assert.equal((await context.request.get(progress)).status(), 200);
      assert.equal((await outsider.request.get(progress)).status(), 404);
      for (const alternate of [
        "/reader/" + chapter.id + "/history?version_id=" + chapter.version,
        "/reader/" + chapter.id + "/preview?version_id=" + chapter.version,
        "/api/v1/chapters/" + chapter.id + "/history",
        "/api/v1/chapters/" + chapter.id + "/versions/" + chapter.version,
        "/cdn/chapters/" + chapter.id + "/" + chapter.version + ".html",
      ]) {
        const response = await outsider.request.get(SITE + alternate);
        assert.equal(response.status(), 404);
        assert(!(await response.text()).includes(secret));
      }
    } finally {
      await outsider.close();
    }
    const existing = await context.request.get(route, { maxRedirects: 0 });
    assert.equal(existing.status(), 303);
    assert.equal(existing.headers().location, "/me/purchases");

    // S065 policy matrix: a purchased V1 survives an ordinary draft/withdrawal,
    // but moderation and hard deletion override even a historical purchase.
    // Probe all three real HTTP surfaces with the buyer's valid session/CSRF.
    const policyCsrf = (await context.cookies(SITE))
      .find((cookie) => cookie.name === "tor-dev-csrf")?.value;
    assert.match(policyCsrf || "", /^[A-Za-z0-9_-]{43}$/);
    const paidUrl = SITE + purchasedHref;
    const assertPaidState = async (expectedStatus) => {
      const paidPage = await context.request.get(paidUrl);
      assert.equal(paidPage.status(), expectedStatus);
      assert((paidPage.headers()["cache-control"] || "").includes("no-store"));
      assert(!(await paidPage.text()).includes("Оплаченная V1: кириллица"),
        "HTTP shell/denial must never include the paid plaintext");
      const library = await context.request.get(SITE + "/me/purchases");
      assert.equal(library.status(), 200);
      assert.equal(library.headers()["cache-control"], "no-store");
      const html = await library.text();
      assert.equal(html.includes('href="' + purchasedHref + '"'), expectedStatus === 200);
      const signed = await context.request.post(
        SITE + "/api/v1/reader/chapters/" + chapter.id + "/delivery-grants",
        { data: { purchase_id: String(purchase) },
          headers: { origin: SITE, "x-csrf-token": policyCsrf } });
      assert.equal(signed.status(), expectedStatus);
      assert((signed.headers()["cache-control"] || "").includes("no-store"));
      if (expectedStatus !== 200) {
        assert.equal((await signed.body()).length, 0,
          "blocked/delete states must never issue a signed bearer URL");
      }
    };
    // S065: while moderation or hard deletion blocks a purchased V1, legacy
    // history, preview, asset and progress URLs must never bypass D/B/G.
    // These requests use the actual buyer session, not an anonymous denial.
    const assertAlternateSurfacesClosed = async () => {
      const secret = "Оплаченная V1: кириллица";
      const oldRoutes = [
        "/read/" + chapter.id + "?preview=1&version_id=" + chapter.version +
          "&purchase_id=" + purchase,
        "/api/v1/chapters/" + chapter.id + "/read?preview=1&purchase_id=" + purchase,
        "/reader/" + chapter.id + "/toc-fragment?limit=100",
        "/reader/" + chapter.id + "/history?version_id=" + chapter.version,
        "/reader/" + chapter.id + "/preview?version_id=" + chapter.version,
        "/api/v1/chapters/" + chapter.id + "/history",
        "/api/v1/chapters/" + chapter.id + "/versions/" + chapter.version,
        "/api/v1/chapters/" + chapter.id + "/preview",
        "/cdn/chapters/" + chapter.id + "/" + chapter.version + ".html",
        "/media/chapters/" + chapter.id + "/" + chapter.version,
      ];
      for (const route of oldRoutes) {
        const reply = await context.request.get(SITE + route, { maxRedirects: 0 });
        assert.equal(reply.status(), 404, "disabled alternate route " + route);
        const body = await reply.text();
        assert(!body.includes(secret), "paid text in alternate route " + route);
        assert(!body.includes("X-Amz-Signature"),
          "S3 bearer data in alternate route " + route);
        assert(!body.includes("ciphertext_url"),
          "private object metadata in alternate route " + route);
      }
      // A forged public preview may show a paywall, but never an owned edition.
      const publicPreview = await context.request.get(SITE + "/reader/" + chapter.id +
        "?preview=1&version_id=" + chapter.version + "&purchase_id=" + purchase,
        { maxRedirects: 0 });
      assert([200, 403, 404].includes(publicPreview.status()));
      assert.equal(publicPreview.headers()["cache-control"], "no-store");
      const paywall = await publicPreview.text();
      assert(!paywall.includes(secret));
      assert(!paywall.includes("X-Amz-Signature"));
      assert(!paywall.includes("purchase_id=" + purchase),
        "forged purchase token was reflected in public reader");
      // Stored progress may remain visible after access is blocked, but it must
      // never contain a paid body or a reusable signed object URL.
      const progress = await context.request.get(SITE +
        "/api/v1/me/progress/purchased?purchase_id=" + purchase +
        "&chapter_id=" + chapter.id + "&version_id=" + chapter.version);
      assert([200, 403, 404].includes(progress.status()));
      assert.equal(progress.headers()["cache-control"], "no-store");
      const position = await progress.text();
      assert(!position.includes(secret));
      assert(!position.includes("X-Amz-Signature"));
      assert(!position.includes("ciphertext_url"));
    };
    assert(Number.isSafeInteger(fixture.book_id) && fixture.book_id > 0);
    for (const [table, id] of [["books", fixture.book_id], ["chapters", chapter.id]]) {
      for (const [column, changed, original, expected] of [
        ["visibility", "'draft'", "'public'", 200],
        ["admin_hold", "true", "false", 403],
        ["is_deleted", "true", "false", 404],
      ]) {
        // These are fixed test-owned columns, on a disposable PG fixture.
        pg("UPDATE " + table + " SET " + column + "=" + changed + " WHERE id=" + id);
        try {
          await assertPaidState(expected);
          if (expected !== 200) await assertAlternateSurfacesClosed();
        } finally {
          pg("UPDATE " + table + " SET " + column + "=" + original + " WHERE id=" + id);
        }
        const restored = await context.request.get(paidUrl);
        assert.equal(restored.status(), 200, "restoring a temporary policy block must restore paid access");
      }
    }

    // Simulate the canonical access-revocation fact used by D refunds. This
    // verifies C access revocation, not execution of the money-refund workflow.
    pg("INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason) VALUES(" +
      purchase + ",'ci:s034-single:" + buyer.id + "','refund')");
    const reader = await page.goto(SITE + "/read/paid/" + chapter.id + "?purchase_id=" + purchase);
    assert.equal(reader.status(), 403);
    const commentsDenied = await context.request.get(
      SITE + "/api/v1/reader/chapters/" + chapter.id + "/paid-comments?" +
      new URLSearchParams({purchase_id:String(purchase),version_id:String(chapter.version)})
    );
    assert.equal(commentsDenied.status(), 403, "refund must also revoke private discussion read");
    assert.equal(commentCount(), 2, "revocation retains immutable discussion facts");
    assert(!(await page.content()).includes("Оплаченная V1: кириллица"));
    const csrf = (await context.cookies(SITE)).find((c) => c.name === "tor-dev-csrf").value;
    const grant = await context.request.post(SITE + "/api/v1/reader/chapters/" + chapter.id + "/delivery-grants",
      { data: {}, headers: { origin: SITE, "x-csrf-token": csrf } });
    assert.equal(grant.status(), 403);
    const revokedAsset = await context.request.post(SITE + assetGrantPath, {
      data: { purchase_id: String(purchase) },
      headers: { origin: SITE, "x-csrf-token": csrf },
    });
    assert.equal(revokedAsset.status(), 403);
    assert.equal(revokedAsset.headers()["cache-control"], "no-store");
    assert.equal((await revokedAsset.body()).length, 0);
    await assertAlternateSurfacesClosed();
    const rejectedIssue = await context.request.post(
      SITE + "/api/v1/reader/chapters/" + chapter.id + "/text-issues", {
        data: {
          purchase_id: String(purchase), version_id: String(chapter.version),
          block_index: 1, selection_start: 0, selection_end: selectedQuote.length,
          quote: selectedQuote, suggested_text: "Повтор после отзыва права",
          issue_type: "typo",
        },
        headers: { origin: SITE, "x-csrf-token": csrf },
      });
    assert.equal(rejectedIssue.status(), 403);
    assert.equal(issueCount(), 1, "revoked purchase must not create a new text issue");
    // A revoked entitlement must leave the purchased library immediately;
    // keeping a historical purchase fact must not create a navigable link.
    const revokedLibrary = await context.request.get(SITE + "/me/purchases");
    assert.equal(revokedLibrary.status(), 200);
    assert.equal(revokedLibrary.headers()["cache-control"], "no-store");
    const revokedHtml = await revokedLibrary.text();
    assert(revokedHtml.includes("Доступных покупок пока нет"));
    assert(!revokedHtml.includes('href="' + purchasedHref + '"'));
    assert.equal(balance(), 600 - price, "revocation alone must never pretend to refund money");
    assert.equal(purchases(), 1);
    const width = viewport.width === 1440 ? "desktop" : "mobile";
    console.log("S034 single Chromium PASS: " + width + " purchase, replay, denial after revocation");
  } finally { await context.close(); }
}
async function run() {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [i, viewport] of [{ width: 1440, height: 900 }, { width: 390, height: 844 }].entries()) {
      await verify(browser, fixture.buyers[i], viewport);
    }
  } finally { await browser.close(); }
}
run().catch((error) => { console.error("S034 single Chromium failed:", error.message); process.exitCode = 1; });
