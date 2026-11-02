// S035: one real Chromium context per viewport, from native batch purchase
// through the owned library to BOTH decrypted private MinIO chapter editions.
// This runs ONLY with disposable local tor_ci, test sessions and loopback S3.
// Never log session cookies, scoped DEKs, signed S3 URLs or plaintext.
"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const { chromium } = require("playwright");

const SITE = "http://127.0.0.1:3098";
const S3 = "http://127.0.0.1:9000";
const fixturePath = process.env.TOR_S035_REAL_BROWSER_FIXTURE || "";
const temp = process.env.RUNNER_TEMP || "";
assert(temp && fixturePath &&
  path.resolve(fixturePath).startsWith(path.resolve(temp) + path.sep),
  "S035 fixture must stay in the disposable CI directory");
const fixture = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
assert.equal(fixture.chapters.length, 2);
assert.equal(fixture.buyers.length, 2);
assert.notEqual(fixture.chapters[0].id, fixture.chapters[1].id);
assert.notEqual(fixture.buyers[0].id, fixture.buyers[1].id);

const db = process.env.TEST_DATABASE_URL || "";
const dbUrl = new URL(db);
assert.equal(dbUrl.protocol, "postgresql:");
assert.equal(dbUrl.hostname, "127.0.0.1");
assert.equal(dbUrl.port, "5432");
assert.equal(dbUrl.pathname, "/tor_ci");
assert.equal(dbUrl.username, "tor_app");
function pg(sql) {
  return execFileSync("psql", [
    "-X", "-q", "-A", "-t", "-v", "ON_ERROR_STOP=1",
    "--dbname", db, "--command", sql,
  ], { encoding: "utf8", timeout: 20000 }).trim();
}
function count(query) {
  const value = pg(query);
  assert.match(value, /^[0-9]+$/);
  return Number(value);
}
const contentMarkers = [
  "Оплаченная V1: кириллица А😀Б",
  "Second paid edition content",
];
async function testViewport(browser, buyer, viewport) {
  const context = await browser.newContext({
    viewport: { width: viewport.width, height: viewport.height },
  });
  const s3Responses = [];
  const grantResponses = [];
  const page = await context.newPage();
  page.on("response", (response) => {
    const url = new URL(response.url());
    if (url.origin === S3 && url.pathname.endsWith(".enc")) {
      if (response.status() === 200) s3Responses.push(url.pathname);
    }
    if (url.origin === SITE && url.pathname.endsWith("/delivery-grants")) {
      grantResponses.push(response.status());
    }
  });
  try {
    assert.match(buyer.session, /^[a-zA-Z0-9_-]{43}$/);
    await context.addCookies([{
      url: SITE, name: "tor-dev-session", value: buyer.session,
      httpOnly: true, sameSite: "Lax",
    }]);
    const bookUrl = SITE + "/books/" + fixture.book_id;
    const opened = await page.goto(bookUrl);
    assert.equal(opened.status(), 200);
    assert(!(await opened.text()).includes(contentMarkers[0]));
    assert(!(await opened.text()).includes(contentMarkers[1]));
    await page.getByRole("link", { name: "Купить несколько глав" }).click();
    await page.waitForURL("**/buy-chapters");
    const cookies = await context.cookies(SITE);
    assert(cookies.some((c) => c.name === "tor-dev-csrf"),
      "real GET form must establish a CSRF cookie");
    for (const chapter of fixture.chapters) {
      await page.locator('input[name="chapter_' + chapter.id + '"]').check();
    }
    assert.equal(
      count("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=" + buyer.id),
      600,
      "opening native selection must not debit the wallet"
    );

    const select = page.waitForResponse((r) =>
      r.url().endsWith("/buy-chapters") && r.request().method() === "POST");
    await page.getByRole("button", { name: "Проверить выбор и цену" }).click();
    assert.equal((await select).status(), 200);
    assert(await page.getByRole("heading",
      { name: "Подтверждение покупки" }).isVisible());
    assert(await page.getByText("Итого: 3,50 ₽").isVisible());
    assert.equal(
      count("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=" + buyer.id),
      600,
      "showing the real quote must not debit the wallet"
    );

    const confirm = page.locator('form[action$="/buy-chapters"]');
    const form = await confirm.evaluate((element) =>
      Object.fromEntries(new FormData(element)));
    assert.equal(form.step, "commit");
    const buy = page.waitForResponse((r) =>
      r.url().endsWith("/buy-chapters") && r.request().method() === "POST");
    await page.getByRole("button",
      { name: "Подтвердить покупку на 3,50 ₽" }).click();
    assert.equal((await buy).status(), 200);
    assert(await page.getByRole("heading",
      { name: "Покупка завершена" }).isVisible());
    assert((await page.content()).includes("2 глав куплено"));
    assert(!(await page.content()).includes(contentMarkers[0]));
    assert(!(await page.content()).includes(contentMarkers[1]));

    const purchases = pg(
      "SELECT chapter_id,version_id,id FROM chapter_purchase_facts " +
      "WHERE user_id=" + buyer.id + " ORDER BY chapter_id"
    ).split("\n").map((line) => line.split("|"));
    const expected = [...fixture.chapters].sort((a, b) => a.id - b.id);
    assert.equal(purchases.length, 2, "atomic checkout must create two purchases");
    const purchaseIds = new Map();
    for (const [i, chapter] of expected.entries()) {
      assert.equal(purchases[i][0], String(chapter.id));
      assert.equal(purchases[i][1], String(chapter.version));
      purchaseIds.set(String(chapter.id), purchases[i][2]);
    }
    assert.notEqual(purchases[0][2], purchases[1][2]);
    assert.equal(
      count("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=" + buyer.id),
      250,
      "batch must debit 350 minor units exactly once"
    );
    assert.equal(
      count("SELECT count(*) FROM reader_wallet_entries " +
        "WHERE user_id=" + buyer.id + " AND kind='chapter_purchase'"),
      2,
      "both purchased editions must have independent immutable ledger rows"
    );

    // Same idempotency command, sent a second time by the same authenticated
    // browser context. A replay must not create a third journal entry.
    const replay = await context.request.post(
      SITE + "/me/books/" + fixture.book_id + "/buy-chapters",
      { form, headers: { origin: SITE } }
    );
    assert.equal(replay.status(), 200);
    assert((await replay.text()).includes("Повтор без нового списания"));
    assert.equal(
      count("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=" + buyer.id),
      250,
      "replay must not charge again"
    );

    // The payment must lead through the actual authenticated library, not
    // directly to a synthetic reader-page fixture or grant stub.
    await page.getByRole("link", { name: "Перейти в библиотеку" }).click();
    await page.waitForURL("**/me/library");
    assert.equal(new URL(page.url()).pathname, "/me/library");
    await page.locator('a[href="/me/purchases"]').first().click();
    await page.waitForURL("**/me/purchases");
    assert.equal(await page.locator("article.paid-library__item").count(), 2);
    for (let index = 0; index < fixture.chapters.length; index++) {
      const chapter = fixture.chapters[index];
      const expectedText = contentMarkers[index];
      const selector = 'article.paid-library__item a[href^="/read/paid/' +
        chapter.id + '?purchase_id="]';
      const link = page.locator(selector);
      assert.equal(await link.count(), 1, "missing owned library entry");
      const href = await link.getAttribute("href");
      const uri = new URL(href, SITE);
      assert.equal(uri.searchParams.get("purchase_id"),
        purchaseIds.get(String(chapter.id)),
        "library link must use the actual immutable purchase");
      const beforeS3 = s3Responses.length;
      const beforeGrants = grantResponses.length;
      const readerResponse = page.waitForResponse((r) =>
        r.url().includes("/read/paid/" + chapter.id + "?purchase_id=") &&
        r.request().method() === "GET");
      await link.click();
      const reader = await readerResponse;
      assert.equal(reader.status(), 200);
      const serverHtml = await reader.text();
      assert(!serverHtml.includes(expectedText),
        "server HTML must not contain the protected plaintext");
      assert.equal(await page.locator("[data-paid-chapter-reader]")
        .getAttribute("data-chapter-id"), String(chapter.id));
      await page.waitForFunction((marker) => {
        const pieces = document.querySelectorAll(
          "[data-paid-content] .reader-piece__text");
        return [...pieces].some((element) =>
          (element.textContent || "").includes(marker));
      }, expectedText, { timeout: 30000 });
      const blocks = await page.locator(
        "[data-paid-content] .reader-piece__text").allTextContents();
      assert(blocks.join("\n").includes(expectedText));
      assert(grantResponses.slice(beforeGrants).includes(200),
        "real reader must obtain an authenticated purchase grant");
      assert(s3Responses.length > beforeS3,
        "real reader must fetch its encrypted private MinIO object");
      await page.goto(SITE + "/me/purchases");
    }
    assert(s3Responses.length >= 2 && grantResponses.filter((s) => s === 200).length >= 2);
    const after = await page.goto(
      SITE + "/me/books/" + fixture.book_id + "/buy-chapters");
    assert.equal(after.status(), 200);
    assert((await page.textContent("body")).includes("нет двух доступных"),
      "purchased chapters must not be offered again");

    // C-06/S006: keep purchase, live PG progress and private S3 delivery in
    // one browser journey. Only the publication fixture is changed via SQL.
    const chapter = fixture.chapters[1];
    const purchase = purchaseIds.get(String(chapter.id));
    const readerPath = "/read/paid/" + chapter.id + "?purchase_id=" + purchase;
    const progressPath = "/api/v1/me/progress/purchased?chapter_id=" + chapter.id +
      "&purchase_id=" + purchase + "&version_id=" + chapter.version;
    await page.goto(SITE + readerPath);
    await page.getByRole("button", { name: "Сохранить после фрагмента 1" }).click();
    await page.getByText("Место сохранено.", { exact: true }).waitFor();
    const savedResponse = await context.request.get(SITE + progressPath);
    assert.equal(savedResponse.status(), 200);
    const saved = await savedResponse.json();
    assert.equal(saved.version_id, String(chapter.version));
    assert.equal(saved.block_id, 1);
    assert(saved.char_offset > 0 && saved.revision > 0);
    const email = pg("SELECT email_normalized FROM users WHERE id=" + buyer.id);
    const newText = "C06 new publication must not replace purchased text";
    pg("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) VALUES(" +
      chapter.id + ",2,'" + newText + "') ON CONFLICT (chapter_id,version_no) DO NOTHING");
    const v2 = count("SELECT id FROM chapter_versions WHERE chapter_id=" +
      chapter.id + " AND version_no=2");
    assert.notEqual(v2, chapter.version);
    try {
      pg("UPDATE chapter_publications SET version_id=" + v2 +
        " WHERE chapter_id=" + chapter.id);
      await page.goto(SITE + "/me");
      await page.getByRole("button", { name: "Выйти", exact: true }).click();
      await page.waitForURL("**/login");
      const revoked = await context.request.get(SITE + progressPath);
      assert.equal(revoked.status(), 401);
      await page.getByLabel("Электронная почта").fill(email);
      await page.getByLabel("Пароль", { exact: true }).fill("S034RealStorageE2EPassword2026");
      const login = page.waitForResponse((r) =>
        new URL(r.url()).pathname === "/login" && r.request().method() === "POST");
      await page.getByRole("button", { name: "Войти", exact: true }).click();
      assert.equal((await login).status(), 303, "real login must create a new session");
      await page.waitForURL("**/me");
      await page.goto(SITE + "/me/purchases");
      const grant = page.waitForResponse((r) =>
        r.url().endsWith("/chapters/" + chapter.id + "/delivery-grants"));
      await page.locator('a[href="' + readerPath + '"]').click();
      const resumedGrant = await grant;
      assert.equal(resumedGrant.status(), 200);
      assert.equal((await resumedGrant.json()).version_id, String(chapter.version));
      const resume = page.locator('[data-paid-resume] a[href="#p-1"]');
      await resume.waitFor();
      await resume.click();
      assert.equal(new URL(page.url()).hash, "#p-1");
      const text = await page.locator("[data-paid-content]").innerText();
      assert(text.includes(contentMarkers[1]));
      assert(!text.includes(newText));
      const restored = await context.request.get(SITE + progressPath);
      assert.equal(restored.status(), 200);
      assert.deepEqual(await restored.json(), saved,
        "relogin after V2 must preserve the exact V1 cursor and revision");
      assert.equal(count("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=" +
        buyer.id), 250, "resuming V1 must not charge again");
      console.log("C06 contiguous Chromium PASS: " + viewport.name +
        " purchased V1 cursor survives V2 publication and real logout/login");
    } finally {
      // Both viewport buyers must start with the same original offer.
      pg("UPDATE chapter_publications SET version_id=" + chapter.version +
        " WHERE chapter_id=" + chapter.id);
    }
    console.log("S035 contiguous Chromium PASS: " + viewport.name +
      " native purchase -> library -> two decrypted private chapters");
  } finally {
    await context.close();
  }
}

// S005/S065: complete native one-chapter purchase in a real browser. The
// separate 500-minor buyer keeps S035's two-chapter buyers untouched.
async function testSingleChapter(browser) {
  assert(Number.isSafeInteger(fixture.single_buyer_id));
  assert.match(fixture.outsider_session, /^[a-zA-Z0-9_-]{43}$/);
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 },
  });
  const page = await context.newPage();
  const chapter = fixture.chapters[0];
  const buyPath = "/me/chapters/" + chapter.id + "/buy";
  const marker = contentMarkers[0];
  const paidGrants = [];
  const encryptedGets = [];
  page.on("response", (res) => {
    const uri = new URL(res.url());
    if (uri.origin === SITE && uri.pathname.endsWith("/delivery-grants")) {
      paidGrants.push(res.status());
    }
    if (uri.origin === S3 && uri.pathname.endsWith(".enc") && res.status() === 200) {
      encryptedGets.push(uri.pathname);
    }
  });
  try {
    assert.equal(count("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=" +
      fixture.single_buyer_id), 500);
    assert.equal(count("SELECT count(*) FROM chapter_purchase_facts WHERE user_id=" +
      fixture.single_buyer_id), 0);
    const paywall = await page.goto(SITE + "/reader/" + chapter.id);
    assert.equal(paywall.status(), 200);
    assert.equal(paywall.headers()["cache-control"], "no-store");
    assert.equal(paywall.headers()["x-robots-tag"], "noindex, nofollow");
    assert(!(await paywall.text()).includes(marker));
    const buyLink = page.locator('a[href="' + buyPath + '"]');
    assert.equal(await buyLink.count(), 1);
    await buyLink.click();
    await page.waitForURL("**/login");
    await page.getByLabel("Электронная почта")
      .fill("s005-real-browser@tor-ci.example");
    await page.getByLabel("Пароль", { exact: true })
      .fill("S034RealStorageE2EPassword2026");
    const login = page.waitForResponse((res) =>
      new URL(res.url()).pathname === "/login" &&
      res.request().method() === "POST");
    await page.getByRole("button", { name: "Войти", exact: true }).click();
    assert.equal((await login).status(), 303);
    await page.waitForURL("**/me");

    await page.goto(SITE + "/reader/" + chapter.id);
    await page.locator('a[href="' + buyPath + '"]').click();
    await page.waitForURL((url) => url.pathname === buyPath);
    assert(await page.getByRole("heading", { name: "Подтвердить покупку" })
      .isVisible());
    assert((await page.content()).includes("1,75 ₽"));
    assert(!(await page.content()).includes(marker));
    assert(!(await page.content()).includes("<img src=x onerror="));
    const form = await page.locator('form[action="' + buyPath + '"]')
      .evaluate((element) => Object.fromEntries(new FormData(element)));
    assert.equal(form.expected_version_id, String(chapter.version));
    assert.equal(form.expected_price_minor, "175");
    assert.match(form.command_key, /^[a-zA-Z0-9_-]{43}$/);
    assert.match(form.csrf_token, /^[a-zA-Z0-9_-]{43}$/);
    assert((await context.cookies(SITE)).some((cookie) =>
      cookie.name === "tor-dev-csrf" && cookie.value === form.csrf_token));
    const post = (fields, origin = SITE) => context.request.post(SITE + buyPath, {
      form: fields, headers: { Origin: origin }, maxRedirects: 0,
    });
    assert.equal((await post({ ...form, csrf_token: "forged" })).status(), 403);
    assert.equal((await post(form, "https://evil.invalid")).status(), 403);
    assert.equal((await post({
      ...form, expected_version_id: String(chapter.version + 1),
    })).status(), 409);
    assert.equal((await post({ ...form, expected_price_minor: "176" })).status(), 409);
    assert.equal(count("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=" +
      fixture.single_buyer_id), 500);

    const buy = page.waitForResponse((res) =>
      new URL(res.url()).pathname === buyPath && res.request().method() === "POST");
    const reader = page.waitForResponse((res) =>
      new URL(res.url()).pathname === "/read/paid/" + chapter.id &&
      res.request().method() === "GET");
    await page.getByRole("button",
      { name: "Подтвердить покупку за 1,75 ₽" }).click();
    assert.equal((await buy).status(), 303);
    const shell = await reader;
    assert.equal(shell.status(), 200);
    assert(!(await shell.text()).includes(marker),
      "Rust must not return protected plaintext in reader HTML");
    await page.waitForFunction((text) => [...document.querySelectorAll(
      "[data-paid-content] .reader-piece__text")].some((piece) =>
      (piece.textContent || "").includes(text)), marker, { timeout: 30000 });
    const purchase = count("SELECT id FROM chapter_purchase_facts WHERE user_id=" +
      fixture.single_buyer_id);
    assert.equal(new URL(page.url()).searchParams.get("purchase_id"), String(purchase));
    assert(paidGrants.includes(200), "private grant must be authorized");
    assert(encryptedGets.length > 0, "reader must fetch encrypted bytes from MinIO");
    assert.equal(count("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=" +
      fixture.single_buyer_id), 325);
    assert.equal(count("SELECT count(*) FROM reader_wallet_entries WHERE user_id=" +
      fixture.single_buyer_id + " AND kind='chapter_purchase'"), 1);
    const replay = await post(form);
    assert.equal(replay.status(), 303);
    assert.equal(new URL(replay.headers().location, SITE).searchParams.get("purchase_id"),
      String(purchase));
    assert.equal((await post({ ...form, command_key: "A".repeat(42) + "8" }))
      .status(), 409);
    assert.equal(count("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=" +
      fixture.single_buyer_id), 325);
    assert.equal(count("SELECT count(*) FROM chapter_purchase_facts WHERE user_id=" +
      fixture.single_buyer_id), 1);
    const repurchase = await context.request.get(SITE + buyPath, { maxRedirects: 0 });
    assert.equal(repurchase.status(), 303);
    assert.equal(new URL(repurchase.headers().location, SITE).pathname, "/me/purchases");

    const outsider = await browser.newContext();
    try {
      await outsider.addCookies([{
        url: SITE, name: "tor-dev-session",
        value: fixture.outsider_session, httpOnly: true, sameSite: "Strict",
      }]);
      const denied = await outsider.request.get(SITE + "/read/paid/" +
        chapter.id + "?purchase_id=" + purchase);
      assert.equal(denied.status(), 403);
      assert(!(await denied.text()).includes(marker));
      const progress = "/api/v1/me/progress/purchased?purchase_id=" + purchase +
        "&chapter_id=" + chapter.id + "&version_id=" + chapter.version;
      assert.equal((await context.request.get(SITE + progress)).status(), 200);
      assert.equal((await outsider.request.get(SITE + progress)).status(), 404);
      for (const alternate of [
        "/reader/" + chapter.id + "/history?version_id=" + chapter.version,
        "/reader/" + chapter.id + "/preview?version_id=" + chapter.version,
        "/api/v1/chapters/" + chapter.id + "/history",
        "/api/v1/chapters/" + chapter.id + "/versions/" + chapter.version,
        "/cdn/chapters/" + chapter.id + "/" + chapter.version + ".html",
      ]) {
        const response = await outsider.request.get(SITE + alternate);
        assert.equal(response.status(), 404, "unregistered paid alternate path");
        assert(!(await response.text()).includes(marker));
      }
    } finally {
      await outsider.close();
    }
    console.log("S005 single-chapter Chromium PASS");
  } finally {
    await context.close();
  }
}

async function run() {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [index, viewport] of [
      { name: "desktop", width: 1440, height: 900 },
      { name: "mobile", width: 390, height: 844 },
    ].entries()) {
      await testViewport(browser, fixture.buyers[index], viewport);
    }
    await testSingleChapter(browser);
  } finally {
    await browser.close();
  }
}
run().catch((error) => {
  // Script never outputs cookies, grants, signed URLs or plaintext.
  console.error("S035 contiguous Chromium failed:", error.message);
  process.exitCode = 1;
});
