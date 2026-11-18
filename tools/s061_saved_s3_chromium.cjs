// Read an immutable saved edition through the live router and private bucket.
"use strict";
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { chromium } = require("playwright");

async function main() {
  const fixture = JSON.parse(readFileSync(0, "utf8"));
  const { origin, endpoint, objectPath, chapter, saved, current, token, markdown, sha256 } = fixture;
  const grant = `${origin}/api/v1/chapters/${chapter}/free-grant?mode=saved&version_id=`;
  const browser = await chromium.launch({ headless: true });
  try {
    const context = await browser.newContext();
    const guestGrant = await context.request.get(grant + saved);
    assert.equal(guestGrant.status(), 404, "guest must not receive a saved grant");
    const unsigned = await context.request.get(endpoint + objectPath, { maxRedirects: 0 });
    assert.equal(unsigned.status(), 403, "saved object must be private");

    await context.addCookies([{ name: "tor-dev-session", value: token, url: origin, httpOnly: true }]);
    const wrong = await context.request.get(grant + current);
    assert.equal(wrong.status(), 404, "saved grant must reject the unpinned current version");
    const issued = await context.request.get(grant + saved);
    assert.equal(issued.status(), 200, "the saved grant must be issued to its owner");
    const body = await issued.json();
    assert.equal(body.chapter_id, chapter);
    assert.equal(body.version_id, saved);
    assert.equal(body.sha256, sha256);
    assert.equal(body.size_bytes, Buffer.byteLength(markdown));
    const reader = `${origin}/me/library/saved/${chapter}`;
    const html = await context.request.get(reader);
    assert.equal(html.status(), 200, "saved reader page must open");
    assert.ok(!(await html.text()).includes(markdown), "TOR HTML contains the S3 chapter text");

    const page = await context.newPage();
    await Promise.all([
      page.waitForResponse(response => response.url() === grant + saved, { timeout: 16000 })
        .then(response => {
          assert.equal(response.status(), 200, "browser must receive the saved grant");
        }),
      page.waitForResponse(response => {
        const url = new URL(response.url());
        return url.origin === endpoint && url.pathname === objectPath &&
          url.searchParams.has("X-Amz-Signature") && response.request().method() === "GET";
      }, { timeout: 16000 }).then(async response => {
        assert.equal(response.status(), 200, "direct private S3 download must succeed");
        assert.equal(await response.request().headerValue("cookie"), null, "session cookie reached S3");
      }),
      page.goto(reader, { timeout: 12000 }),
    ]);
    // The production script renders the text only after verifying its SHA-256.
    await page.locator("#chapter-text").getByText(markdown, { exact: true }).waitFor({ timeout: 5000 });
  } finally {
    await browser.close();
  }
}

main().catch(error => {
  console.error(error.message);
  process.exitCode = 1;
});
