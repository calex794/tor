// S037: real SSR forms against the Rust test server and disposable PostgreSQL.
const assert = require('node:assert/strict');
const { chromium } = require('playwright');

(async () => {
  const origin = 'http://127.0.0.1:3098';
  const width = Number(process.env.SWAP_TEST_WIDTH);
  assert.ok([390, 1440].includes(width));
  const browser = await chromium.launch({ headless: true });
  try {
    const context = await browser.newContext({ viewport: { width, height: 900 } });
    await context.addCookies([
      { name: 'tor-dev-session', value: process.env.SWAP_TEST_SESSION, url: origin },
      { name: 'tor-dev-csrf', value: process.env.SWAP_TEST_CSRF, url: origin },
    ]);
    const page = await context.newPage();
    page.setDefaultTimeout(15000);
    assert.equal((await page.goto(`${origin}/me`)).status(), 200);
    await page.getByRole('link', { name: 'Обмен глав', exact: true }).click();
    await page.waitForURL(`${origin}/me/refunds/swap`);
    await page.getByLabel('Вариант обмена').selectOption(process.env.SWAP_TEST_CHOICE);
    const preview = page.waitForResponse(r => r.url().endsWith('/me/refunds/swap') && r.request().method() === 'POST');
    await page.getByRole('button', { name: 'Проверить обмен и сумму' }).click();
    assert.equal((await preview).status(), 200);
    await page.getByRole('button', { name: 'Подтвердить обмен' }).waitFor();
    assert.match(await page.getByRole('status').innerText(), /Доплата 1,50/);
    const fields = await page.locator('form[action="/me/refunds/swap"]').evaluate(form =>
      Array.from(new FormData(form).entries()));
    const confirmation = new URLSearchParams(fields).toString();
    assert.equal(new URLSearchParams(confirmation).get('step'), 'commit');
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), 'no horizontal overflow');
    const commit = page.waitForResponse(r => r.url().endsWith('/me/refunds/swap') && r.request().method() === 'POST');
    await page.getByRole('button', { name: 'Подтвердить обмен' }).click();
    const response = await commit;
    assert.equal(response.status(), 200);
    assert.equal(response.headers()['cache-control'], 'no-store');
    await page.getByText(/Глава обменена/).waitFor();
    assert.match(await page.getByRole('status').innerText(), /5,50/);
    assert.ok(!(await page.content()).includes('Закрытая глава'), 'no paid text in receipt');
    // stdout is consumed by Rust for the exact replay; never log session cookies.
    process.stdout.write(confirmation);
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
