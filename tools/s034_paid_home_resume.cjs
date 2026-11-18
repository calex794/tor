// RET-03e: real Home -> D-authorized purchased archive -> exact V1 reader.
// Navigation never mints an entitlement. Paid bytes come only from MinIO
// after the existing reader rechecks this buyer's pinned purchase.
"use strict";
const assert = require("node:assert/strict");

module.exports = async function verifyHomePaidResume({
  page, context, base, book, chapter, purchasedHref, progressUrl,
  savedV1, purchases,
}) {
  assert(Number.isSafeInteger(book) && book > 0);
  assert.equal(purchasedHref.startsWith("/read/paid/" + chapter.id + "?"), true);
  const homeResponse = await page.goto(base + "/me", { waitUntil: "domcontentloaded" });
  assert.equal(homeResponse.status(), 200);
  const home = page.locator('[data-ret-purchased-book="' + book + '"]');
  assert.equal(await home.count(), 1, "purchased-only Book absent from private Home");
  const archiveHref = "/me/library/" + book + "/saved";
  assert.equal(await home.getAttribute("href"), archiveHref);
  const homeHtml = await page.content();
  assert(!homeHtml.includes("purchase_id="), "Home must not expose a purchase ID");
  assert(!homeHtml.includes("X-Amz-Signature"));
  assert(!homeHtml.includes("Оплаченная V1: кириллица"));

  await home.click();
  await page.waitForURL(base + archiveHref);
  const archive = page.locator('a[href="' + purchasedHref + '"]');
  assert.equal(await archive.count(), 1, "the private archive lost the pinned V1 link");
  assert.match(await archive.textContent(), /Купленная редакция/);
  const archiveHtml = await page.content();
  assert(!archiveHtml.includes("Оплаченная V1: кириллица"));
  assert(!archiveHtml.includes("X-Amz-Signature"));

  await archive.click();
  await page.waitForURL(base + purchasedHref);
  await page.waitForFunction(() => Boolean(
    document.querySelector('[data-paid-resume] a[href="#p-1"]')
    && [...document.querySelectorAll("[data-paid-content] .reader-piece__text")]
      .some(node => (node.textContent || "").includes("Оплаченная V1: кириллица"))
  ), null, { timeout: 30000 });
  await page.locator('[data-paid-resume] a[href="#p-1"]').click();
  assert.equal(new URL(page.url()).hash, "#p-1");

  const savedResponse = await context.request.get(progressUrl);
  assert.equal(savedResponse.status(), 200);
  assert.equal(savedResponse.headers()["cache-control"], "no-store");
  const after = await savedResponse.json();
  assert.deepEqual(
    [after.chapter_id, after.version_id, after.block_id,
      after.char_offset, after.revision],
    [savedV1.chapter_id, savedV1.version_id, savedV1.block_id,
      savedV1.char_offset, savedV1.revision],
    "Home -> archive -> reader must preserve the exact paid V1 cursor"
  );
  assert.equal(purchases(), 1, "the archive shortcut cannot purchase twice");
};
