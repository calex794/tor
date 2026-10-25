const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');

const base = 'http://127.0.0.1:3098';
const out = process.env.TOR_READER_SHOTS || 'reader-shots';
fs.mkdirSync(out, { recursive: true });

function assert(ok, why) {
  if (!ok) throw new Error(why);
}

async function inspect(page, name, where) {
  const metrics = await page.evaluate(() => ({
    content: document.documentElement.scrollWidth,
    viewport: document.documentElement.clientWidth,
    body: document.body.scrollWidth
  }));
  assert(metrics.content <= metrics.viewport + 1 && metrics.body <= metrics.viewport + 1,
    name + ': ' + where + ' horizontal overflow ' + JSON.stringify(metrics));
  assert(await page.locator('h1').isVisible(), name + ': missing h1');
  await page.screenshot({ path: path.join(out, name + '-' + where + '.png'), fullPage: true });
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [name, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 1 });
      const book = await page.goto(base + '/books/1', { waitUntil: 'networkidle' });
      assert(book && book.status() === 200, name + ': public book missing');
      assert(await page.getByText('Глава 1. Начало истории').isVisible(), name + ': TOC invisible');
      assert(await page.getByText('Платная · пока недоступна').isVisible(), name + ': paywall label missing');
      assert(!(await page.content()).includes('PRIVATE-PAID-MARKER'), name + ': paid text in TOC');
      await inspect(page, name, 'toc');
      await page.getByRole('link', { name: /Глава 1\. Начало истории/ }).click();
      await page.waitForURL('**/reader/101');
      assert(await page.getByText('Первая строка').count() > 0, name + ': no actual text');
      const html = await page.content();
      assert(!html.includes('<script>alert(1)</script>'), name + ': unsafe author HTML');
      assert(html.includes('&lt;script&gt;'), name + ': escaped author HTML not displayed');
      assert(!html.includes('PRIVATE-PAID-MARKER'), name + ': paid body leaked');
      await inspect(page, name, 'reader');
      for (const denied of ['/reader/102', '/reader/103', '/books/2']) {
        const response = await page.goto(base + denied);
        assert(response && response.status() === 404, name + ': denied URL ' + denied);
      }
      await page.close();
    }
    // Exercise real PostgreSQL account forms on both target viewport sizes.
    for (const [name, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 1 });
      await page.goto(base + '/register');
      assert(await page.getByRole('heading', { name: 'Регистрация' }).isVisible(), name + ': registration form hidden');
      await inspect(page, name, 'register');
      await page.getByLabel('Отображаемое имя').fill('Читатель ' + name);
      await page.getByLabel('Электронная почта').fill('visual.' + name + '@example.test');
      await page.getByLabel('Пароль (12–128 символов)').fill('TestPassphrase2026');
      const beforeCookies = await page.context().cookies(base);
      const beforeCsrf = beforeCookies.find(c => c.name === 'tor-dev-csrf');
      const beforeToken = await page.locator('input[name="csrf_token"]').inputValue();
      // Never print CSRF/session values. Metadata is sufficient to debug cookies.
      const before = {
        csrfCookie: Boolean(beforeCsrf),
        hasMatchingToken: beforeCsrf ? beforeCsrf.value === beforeToken : false,
        tokenLength: beforeToken.length,
        cookieNames: beforeCookies.map(c => c.name),
      };
      const submitted = page.waitForResponse(response =>
        response.url().endsWith('/register') && response.request().method() === 'POST'
      );
      await page.getByRole('button', { name: 'Создать учётную запись' }).click();
      const registerResponse = await submitted;
      if (registerResponse.status() !== 303) {
        const headers = await registerResponse.request().allHeaders();
        const requestCookieNames = (headers.cookie || '').split(';')
          .map(x => x.trim().split('=')[0]).filter(Boolean);
        throw new Error(name + ': register HTTP ' + registerResponse.status() +
          ', diagnostics ' + JSON.stringify({
            ...before,
            origin: headers.origin || null,
            fetchSite: headers['sec-fetch-site'] || null,
            cookieNamesInRequest: requestCookieNames,
            postsCsrf: (registerResponse.request().postData() || '').includes('csrf_token=')
          }));
      }
      await page.waitForURL('**/me', { timeout: 10000 });
      assert(await page.getByText('Читатель ' + name).isVisible(), name + ': profile missing');
      await inspect(page, name, 'me');
      await page.getByRole('button', { name: 'Выйти' }).click();
      await page.waitForURL('**/login');
      await page.getByLabel('Электронная почта').fill('visual.' + name + '@example.test');
      await page.getByLabel('Пароль').fill('TestPassphrase2026');
      await inspect(page, name, 'login');
      await page.getByRole('button', { name: 'Войти', exact: true }).click();
      await page.waitForURL('**/me');
      assert(await page.getByText('Читатель ' + name).isVisible(), name + ': login did not restore session');
      await page.close();
    }
    console.log('TOR PG reader+auth browser PASS: 2 viewports, 10 screenshots, no overflow and real DB');
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
