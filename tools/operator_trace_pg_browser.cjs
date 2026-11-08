'use strict';
const assert = require('node:assert/strict');
const path = require('node:path');
const { chromium } = require('playwright');

(async () => {
  const origin = process.env.TOR_S059_ORIGIN;
  const id = process.env.TOR_S059_TRACE_ID;
  const password = process.env.TOR_TEST_OPERATOR_PASSWORD;
  assert.equal(origin, 'http://127.0.0.1:3094');
  assert.match(id, /^[0-9a-f]{32}$/);
  assert.ok(password && password.length >= 32);
  const browser = await chromium.launch({ headless: true });
  try {
    const stranger = await browser.newContext();
    const denied = await stranger.request.get(`${origin}/ops/traces?id=${id}`);
    assert.equal(denied.status(), 401);
    assert.equal(denied.headers()['cache-control'], 'no-store');
    await stranger.close();
    const context = await browser.newContext({ httpCredentials: { username: 'operator', password } });
    const page = await context.newPage();
    for (const [label, width] of [['desktop', 1440], ['mobile', 390]]) {
      await page.setViewportSize({ width, height: 900 });
      const response = await page.goto(`${origin}/ops/traces`);
      assert.equal(response.status(), 200);
      await page.locator('input[name="id"]').fill(id);
      await page.locator('form button').click();
      await page.waitForURL(`**/ops/traces?id=${id}`);
      assert.ok(await page.getByRole('heading', { name: `Трасса ${id}`, exact: true }).isVisible());
      const text = await page.locator('main').innerText();
      assert.ok(text.includes('PostgreSQL (SQLx)'));
      assert.ok(text.includes('Возможное узкое место:'));
      assert.ok(text.includes('HTTP-запрос длился более одной секунды'));
      assert.ok(!text.includes('SQLx не измерялась'));
      for (const secret of [password, 'SELECT ', 'S059 dataset', 'CLIENT-SUPPLIED-TRACE', 'PRIVATE-PAID-MARKER'])
        assert.ok(!text.includes(secret), 'diagnostics leaked content');
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), false);
      await page.screenshot({ path: path.join(process.env.TOR_S059_OUTPUT, `${label}.png`), fullPage: true });
    }
    await context.close();
    console.log('S059 real PostgreSQL operator browser PASS (desktop/mobile)');
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
