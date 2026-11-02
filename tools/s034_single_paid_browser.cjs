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
    assert.equal(balance(), 600 - price);
    assert.equal(purchases(), 1);
    const purchase = number("SELECT id FROM chapter_purchase_facts WHERE user_id=" + buyer.id);
    assert.equal(new URL(page.url()).searchParams.get("purchase_id"), String(purchase));
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

    // Simulate the canonical access-revocation fact used by D refunds. This
    // verifies C access revocation, not execution of the money-refund workflow.
    pg("INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason) VALUES(" +
      purchase + ",'ci:s034-single:" + buyer.id + "','refund')");
    const reader = await page.goto(SITE + "/read/paid/" + chapter.id + "?purchase_id=" + purchase);
    assert.equal(reader.status(), 403);
    assert(!(await page.content()).includes("Оплаченная V1: кириллица"));
    const csrf = (await context.cookies(SITE)).find((c) => c.name === "tor-dev-csrf").value;
    const grant = await context.request.post(SITE + "/api/v1/reader/chapters/" + chapter.id + "/delivery-grants",
      { data: {}, headers: { origin: SITE, "x-csrf-token": csrf } });
    assert.equal(grant.status(), 403);
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
