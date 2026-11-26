"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");

const origin = process.env.TOR_FALLBACK_TEST_ORIGIN;
const configPath = process.env.TOR_FALLBACK_TEST_CONFIG;
const mode = process.env.TOR_FALLBACK_TEST_MODE;
const shots = process.env.TOR_FALLBACK_SHOTS;
assert.match(origin, /^http:\/\/127\.0\.0\.1:\d+$/);

async function main() {
  const browser = await chromium.launch({headless:true});
  try {
    const context = await browser.newContext({viewport:{width:1280,height:800}});
    const errors = [];
    const requests = [];
    const page = await context.newPage();
    page.on("pageerror", error => errors.push(error.message));
    page.on("console", msg => { if (msg.type() === "error" && /Content Security Policy/.test(msg.text())) errors.push(msg.text()); });
    page.on("request", request => requests.push(request.url()));
    if (mode === "recovery") {
      const response = await page.goto(origin + "/books/503");
      assert.equal(response.status(), 503);
      await page.locator("#check").click();
      await page.waitForFunction(() => document.getElementById("connection").textContent.includes("Каталог отвечает"));
      assert.equal(page.url(), origin + "/books/503");
    } else {
      // Place the first book one step to the right of the initial snake.
      await context.addInitScript(() => { Math.random = () => 168.5 / 321; });
      const response = await page.goto(origin + "/books/123");
      assert.equal(response.status(), 502);
      await page.waitForLoadState("networkidle");
      assert.equal(await page.locator("#telegram").isVisible(), false);
      assert.equal(await page.locator("#board").isVisible(), true);
      await page.locator("#start").click();
      await page.waitForFunction(() => Number(document.getElementById("score").textContent) >= 1);
      await page.locator("#board").press("ArrowUp");
      await page.locator("#pause").click();
      assert.equal(await page.locator("#pause").textContent(), "Продолжить");
      const best = await page.locator("#best").textContent();
      assert.ok(Number(best) >= 1);
      await page.reload();
      assert.equal(await page.locator("#best").textContent(), best);
      await page.locator("#check").click();
      await page.waitForFunction(() => document.getElementById("connection").textContent.includes("Каталог пока недоступен"));
      assert.equal(page.url(), origin + "/books/123");
      fs.writeFileSync(configPath, JSON.stringify({telegram_url:"javascript:alert(1)"}));
      await page.reload({waitUntil:"networkidle"});
      assert.equal(await page.locator("#telegram").isVisible(), false);
      const channel = "https://t.me/pandaolate_ci_fixture";
      fs.writeFileSync(configPath, JSON.stringify({telegram_url:channel}));
      await page.reload();
      await page.locator("#telegram").waitFor({state:"visible"});
      assert.equal(await page.locator("#telegram").getAttribute("href"), channel);
      if (shots) await page.screenshot({path:path.join(shots,"desktop.png"),fullPage:true});

      const mobile = await browser.newContext({viewport:{width:390,height:844},deviceScaleFactor:1,isMobile:true,hasTouch:true});
      const phone = await mobile.newPage();
      phone.on("pageerror", error => errors.push(error.message));
      phone.on("request", request => requests.push(request.url()));
      assert.equal((await phone.goto(origin + "/catalog")).status(), 502);
      assert.ok(await phone.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await phone.locator("#start").tap();
      await phone.locator('[data-dir="up"]').tap();
      await phone.locator("#pause").tap();
      assert.equal(await phone.locator("#pause").textContent(), "Продолжить");
      if (shots) await phone.screenshot({path:path.join(shots,"mobile.png"),fullPage:true});
      await mobile.close();
    }
    assert.deepEqual(errors, []);
    assert.ok(requests.every(url => url.startsWith(origin + "/")), "third-party request: " + requests.join(", "));
    await context.close();
    console.log("Chromium " + mode + " acceptance passed");
  } finally { await browser.close(); }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
