// H/UI-20: public desktop/mobile navigation to a published package.
// No login secrets or protected chapter contents are used by this test.
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
for (const key of ["book", "layer"]) {
  assert(Number.isSafeInteger(fixture[key]) && fixture[key] > 0);
}

async function verify(browser, viewport) {
  const context = await browser.newContext({ viewport });
  try {
    const page = await context.newPage();
    const book = await page.goto(SITE + "/books/" + fixture.book);
    assert.equal(book.status(), 200);
    assert(await page.getByRole("heading", { name: "Доступно через пакет" }).isVisible());
    assert.equal(await page.getByRole("link", { name: "Купить несколько глав" }).count(), 0);
    await page.getByRole("link", { name: /Посмотреть пакет/ }).click();
    await page.waitForURL("**/layers/" + fixture.layer);
    assert(await page.getByRole("heading", { name: /H20.*пакет/ }).isVisible());
    assert(await page.getByText("Оплата только из внутреннего кошелька.", { exact: false }).isVisible());
    assert(!(await page.content()).includes("H20 sealed fixture"));
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
