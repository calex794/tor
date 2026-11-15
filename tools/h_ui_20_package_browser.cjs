// H/UI-20: real desktop/mobile navigation to the same public package.
// Only a disposable tor_ci account and loopback server; never log cookies.
"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");
const SITE = "http://127.0.0.1:3098";
const temp = process.env.RUNNER_TEMP || "";
const filename = process.env.TOR_H20_REAL_BROWSER_FIXTURE || "";
assert(temp && filename && path.resolve(filename).startsWith(path.resolve(temp) + path.sep));
const fixture = JSON.parse(fs.readFileSync(filename, "utf8"));
for (const key of ["book", "layer", "buyer"]) {
  assert(Number.isSafeInteger(fixture[key]) && fixture[key] > 0);
}
assert.match(fixture.token, /^[A-Za-z0-9_-]{43}$/);

async function verify(browser, viewport) {
  const context = await browser.newContext({ viewport });
  try {
    await context.addCookies([{
      url: SITE, name: "tor-dev-session", value: fixture.token,
      httpOnly: true, sameSite: "Lax",
    }]);
    const page = await context.newPage();
    const book = await page.goto(SITE + "/books/" + fixture.book);
    assert.equal(book.status(), 200);
    assert(await page.getByRole("heading", { name: "Доступно через пакет" }).isVisible());
    assert.equal(await page.getByRole("link", { name: "Купить несколько глав" }).count(), 0);
    await page.getByRole("link", { name: /Посмотреть пакет/ }).click();
    await page.waitForURL("**/layers/" + fixture.layer);
    assert(await page.getByRole("heading", { name: /H20.*пакет/ }).isVisible());
    const batch = await page.goto(SITE + "/me/books/" + fixture.book + "/buy-chapters");
    assert.equal(batch.status(), 200);
    assert(await page.getByRole("heading", { name: "Доступно через пакет" }).isVisible());
    assert.equal(await page.locator('input[name^="chapter_"]').count(), 0);
    await page.getByRole("link", { name: "Посмотреть пакет" }).click();
    await page.waitForURL("**/layers/" + fixture.layer);
    assert(await page.getByText("Оплата только из внутреннего кошелька.", { exact: false }).isVisible());
    const horizontalOverflow = await page.evaluate(() =>
      document.documentElement.scrollWidth > document.documentElement.clientWidth + 1);
    assert(!horizontalOverflow, "package storefront must fit the viewport");
  } finally {
    await context.close();
  }
}
(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    await verify(browser, { width: 1365, height: 850 });
    await verify(browser, { width: 390, height: 844 });
    console.log("H/UI-20 Chromium PASS: desktop and mobile");
  } finally {
    await browser.close();
  }
})().catch(err => { console.error(err.message); process.exitCode = 1; });
