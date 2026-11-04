// Real PostgreSQL/Axum/MinIO/Chromium: an administrator switches from
// a private plaintext draft to an earlier attached encrypted edition and PNG.
// Test-only data in disposable tor_ci; no purchases, wallet writes or production.
"use strict";
const assert = require("node:assert/strict");

module.exports = async function sealedHistoryAcceptance({
  browser, SITE, S3, admin_id, admin_session, outsider_session,
  chapter_id, version_id, asset_object_id, pg, count, contextFor, assertNoMoney,
}) {
  assert.equal(
    count("SELECT count(*) FROM chapter_working_drafts WHERE chapter_id=" + chapter_id),
    0, "fixture must start with the sealed published chapter, no draft"
  );
  const nextNo = count("SELECT COALESCE(MAX(version_no),0)+1 " +
    "FROM chapter_versions WHERE chapter_id=" + chapter_id);
  // Using a real, separate V2 tests that the historical ciphertext is not
  // accidentally substituted for the current unsealed working draft.
  const draft = count("INSERT INTO chapter_versions(chapter_id,version_no," +
    "body_markdown) VALUES(" + chapter_id + "," + nextNo +
    ",'S065-HIST-CURRENT-PRIVATE') RETURNING id");
  let draftAttached = false;
  let context = null;
  try {
    pg("INSERT INTO chapter_working_drafts(chapter_id,version_id) VALUES(" +
      chapter_id + "," + draft + ")");
    draftAttached = true;
    context = await contextFor(browser, admin_session);
    const page = await context.newPage();
    await page.setViewportSize({ width: 390, height: 844 });
    const seen = { chapter: 0, image: 0, s3Chapter: 0, s3Image: 0 };
    const base = "/api/v1/reader/chapters/" + chapter_id +
      "/admin-sealed-history";
    const edition = base + "/" + version_id + "/delivery-grants";
    const asset = base + "/" + version_id + "/assets/" +
      asset_object_id + "/delivery-grants";
    page.on("response", (response) => {
      const uri = new URL(response.url());
      if (response.status() !== 200) return;
      if (uri.origin === SITE && response.request().method() === "POST") {
        if (uri.pathname === edition) seen.chapter++;
        if (uri.pathname === asset) seen.image++;
      }
      if (uri.origin === S3 && response.request().method() === "GET") {
        if (uri.pathname.includes("/private/chapter-versions/")) seen.s3Chapter++;
        if (uri.pathname.includes("/private/chapter-assets/" +
          asset_object_id + ".enc")) seen.s3Image++;
      }
    });
    const pageResponse = await page.goto(SITE + "/read/admin/" + chapter_id, {
      waitUntil: "domcontentloaded",
    });
    assert.equal(pageResponse.status(), 200);
    const html = await pageResponse.text();
    assert(!html.includes("Оплаченная V1: кириллица") &&
      !html.includes("S065-HIST-CURRENT-PRIVATE") &&
      !html.includes("X-Amz-Signature"),
      "server HTML must never include private text or signed S3 URLs");
    await page.waitForFunction(() => document.querySelector("[data-admin-content]")
      ?.textContent.includes("S065-HIST-CURRENT-PRIVATE"));
    assert.equal(
      await page.locator("[data-admin-chapter-reader]").getAttribute("data-admin-unsealed"),
      "true", "current unsealed draft must take precedence"
    );
    const select = page.getByLabel("История зашифрованных редакций");
    await select.waitFor();
    assert.equal(await select.locator("option[value='" + version_id + "']").count(), 1);
    const csrf = await page.locator("[data-admin-chapter-reader]")
      .getAttribute("data-csrf-token");
    assert.match(csrf, /^[A-Za-z0-9_-]{43}$/);
    const headers = { origin: SITE, "x-csrf-token": csrf };
    const auditBefore = count("SELECT count(*) FROM reader_admin_chapter_access_audit " +
      "WHERE admin_user_id=" + admin_id + " AND chapter_id=" + chapter_id +
      " AND version_id=" + version_id + " AND action='sealed_chapter_grant'");
    // Only the explicitly selected historical version may trigger D->B->G->D.
    await select.selectOption(String(version_id));
    await page.locator("[data-admin-status]").filter({
      hasText: "Защищённый просмотр",
    }).waitFor({ timeout: 30000 });
    const article = page.locator("[data-admin-content]");
    assert((await article.textContent()).includes("Оплаченная V1: кириллица"));
    assert(!(await article.textContent()).includes("S065-HIST-CURRENT-PRIVATE"));
    assert.equal(await article.locator(".reader-save-button").count(), 0);
    const frame = article.locator(".reader-content__image-frame");
    assert.equal(await frame.count(), 1);
    await frame.scrollIntoViewIfNeeded();
    await page.waitForFunction(() => {
      const image = document.querySelector(
        "[data-admin-content] .reader-content__image-frame img");
      return image && image.complete && image.naturalWidth > 0;
    }, null, { timeout: 30000 });
    const image = await frame.locator("img").evaluate((node) => ({
      src: node.getAttribute("src"), referrer: node.referrerPolicy,
    }));
    assert(image.src.startsWith("blob:"));
    assert.equal(image.referrer, "no-referrer");
    assert(seen.chapter > 0 && seen.image > 0 &&
      seen.s3Chapter > 0 && seen.s3Image > 0,
      "historic encrypted chapter and PNG must load from real private MinIO");
    assert(count("SELECT count(*) FROM reader_admin_chapter_access_audit " +
      "WHERE admin_user_id=" + admin_id + " AND chapter_id=" + chapter_id +
      " AND version_id=" + version_id +
      " AND action='sealed_chapter_grant'") > auditBefore);

    // The two history selectors share one rendering generation. Hold a real
    // encrypted grant response, choose the current unsealed edition, then
    // release the old grant: it must not repaint the newer private selection.
    const unsealedSelect = page.getByLabel("История редакций");
    await unsealedSelect.waitFor();
    assert.equal(await unsealedSelect.locator(
      "option[value='" + draft + "']").count(), 1);
    const delayedPattern = "**/admin-sealed-history/" + version_id +
      "/delivery-grants";
    let releaseDelayed = () => {};
    const delay = new Promise(resolve => { releaseDelayed = resolve; });
    let markIntercepted = () => {};
    const intercepted = new Promise(resolve => { markIntercepted = resolve; });
    await page.route(delayedPattern, async (route) => {
      markIntercepted();
      await delay;
      await route.continue();
    });
    try {
      await select.dispatchEvent("change");
      await intercepted;
      await unsealedSelect.dispatchEvent("change");
      await page.locator("[data-admin-status]").filter({
        hasText: "Административный просмотр",
      }).waitFor({ timeout: 30000 });
      assert((await article.textContent()).includes("S065-HIST-CURRENT-PRIVATE"));
      const finished = page.waitForEvent("requestfinished", {
        predicate: (request) => request.url() === SITE + edition,
      });
      releaseDelayed();
      await finished;
      await page.evaluate(() => new Promise(resolve =>
        requestAnimationFrame(() => requestAnimationFrame(resolve))));
      assert((await article.textContent()).includes("S065-HIST-CURRENT-PRIVATE"),
        "late historical sealed response must not overwrite later unsealed choice");
      assert(!(await article.textContent()).includes("Оплаченная V1: кириллица"));
      assert.equal(await article.locator("img").count(), 0);
    } finally {
      releaseDelayed();
      await page.unroute(delayedPattern);
    }

    for (const [session, code] of [[null, 401], [outsider_session, 403]]) {
      const other = await contextFor(browser, session);
      try {
        for (const target of [base, edition, asset]) {
          const rejected = await other.request.post(SITE + target, { headers });
          assert.equal(rejected.status(), code);
          assert.equal((await rejected.body()).length, 0);
        }
      } finally { await other.close(); }
    }
    for (const target of [base, edition, asset]) {
      const rejected = await context.request.post(SITE + target, {
        headers: { ...headers, origin: "https://foreign.invalid" },
      });
      assert.equal(rejected.status(), 403);
      assert.equal((await rejected.body()).length, 0);
    }
    const fake = await context.request.post(SITE + base +
      "/999999999/delivery-grants", { headers });
    assert.notEqual(fake.status(), 200);
    assert.equal((await fake.body()).length, 0);
    // Revoke the role in the already open browser and ask for that SAME V1.
    // The old in-memory text must be cleared and no new bearer may be issued.
    pg("UPDATE account_roles SET is_active=FALSE WHERE user_id=" +
      admin_id + " AND role='administrator'");
    try {
      for (const target of [base, edition, asset]) {
        const revoked = await context.request.post(SITE + target, { headers });
        assert.equal(revoked.status(), 403);
        assert.equal((await revoked.body()).length, 0);
      }
      await select.dispatchEvent("change");
      await page.locator("[data-admin-status]").filter({
        hasText: "Не удалось открыть редакцию.",
      }).waitFor({ timeout: 30000 });
      assert.equal(await article.textContent(), "");
    } finally {
      pg("UPDATE account_roles SET is_active=TRUE WHERE user_id=" +
        admin_id + " AND role='administrator'");
    }
    await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
    assert.equal(await article.textContent(), "");
    assertNoMoney();
  } finally {
    if (context) await context.close();
    if (draftAttached) pg("DELETE FROM chapter_working_drafts WHERE chapter_id=" +
      chapter_id + " AND version_id=" + draft);
    // This orphaned *test-only* revision remains in the disposable database:
    // the immutable administrator audit may reference its version_id.
    // It has no publication or working-draft pointer and cannot affect buyers.
  }
};
