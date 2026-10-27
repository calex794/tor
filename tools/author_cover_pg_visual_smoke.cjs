// H-web-11: actual Chromium owner journey. No HTML fixture, bypassCSP or fake S3.
// Runs only with the disposable PostgreSQL and private loopback MinIO in CI.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');

const base = 'http://127.0.0.1:3098';
const output = process.env.TOR_COVER_BROWSER_SHOTS || 'cover-browser-shots';
fs.mkdirSync(output, { recursive: true });

function assert(ok, message) {
  if (!ok) throw new Error(message);
}

async function shot(page, viewport, scheme, state) {
  const frame = page.locator('.book-cover-picker__frame');
  const box = await frame.boundingBox();
  assert(box && Math.abs(box.width / box.height - 2 / 3) < 0.035,
    viewport + '/' + scheme + '/' + state + ': NEwBOO 2:3 cover frame missing');
  const metrics = await page.evaluate(() => ({
    viewport: document.documentElement.clientWidth,
    html: document.documentElement.scrollWidth,
    body: document.body.scrollWidth,
    border: getComputedStyle(document.querySelector('.book-cover-picker__frame')).borderTopStyle,
  }));
  assert(metrics.html <= metrics.viewport + 1 && metrics.body <= metrics.viewport + 1,
    viewport + '/' + scheme + ': horizontal overflow ' + JSON.stringify(metrics));
  assert(metrics.border === 'dashed', 'Original NEwBOO dashed cover frame missing');
  assert(await page.locator('.book-cover-picker').count() === 1,
    'TOR may show only its one functional slot, never three fake cover slots');
  assert(await page.getByRole('heading', { name: 'Обложки книги' }).isVisible(),
    'Original NEwBOO covers heading missing');
  await page.screenshot({
    path: path.join(output, ['cover-owner', viewport, scheme, state].join('-') + '.png'),
    fullPage: true,
  });
}

async function actualPng(page) {
  // Create a decodable 80x120 PNG, not a 1x1 browser-only placeholder.
  const data = await page.evaluate(() => {
    const canvas = document.createElement('canvas');
    canvas.width = 80;
    canvas.height = 120;
    const ctx = canvas.getContext('2d');
    ctx.fillStyle = '#3267a4';
    ctx.fillRect(0, 0, 80, 120);
    ctx.fillStyle = '#ffffff';
    ctx.fillRect(12, 20, 56, 14);
    return canvas.toDataURL('image/png');
  });
  assert(data.startsWith('data:image/png;base64,'), 'Browser did not create a valid PNG');
  return Buffer.from(data.split(',')[1], 'base64');
}

async function readyCover(page, bookId, viewport, scheme) {
  for (let i = 0; i < 50; i++) {
    const response = await page.goto(base + '/me/books/' + bookId + '/settings/covers');
    assert(response && response.status() === 200,
      viewport + '/' + scheme + ': owner settings lost authorization while processing');
    if (await page.getByText('Обложка готова.').isVisible()) {
      assert(await page.locator('img[data-cover-picker-image]').getAttribute('src')
        === '/covers/' + bookId + '/main.webp',
      'Ready cover must use the B-owned published WebP');
      return;
    }
    assert(!(await page.getByText(/Ошибка обработки обложки/).isVisible()),
      viewport + '/' + scheme + ': worker rejected image');
    await page.waitForTimeout(1000);
  }
  throw new Error(viewport + '/' + scheme + ': S3/PG worker never marked cover done');
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [viewport, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      for (const scheme of ['light', 'dark']) {
        const context = await browser.newContext({
          viewport: { width, height }, colorScheme: scheme,
        });
        const page = await context.newPage();
        const title = 'Обложка из браузера ' + viewport + ' ' + scheme;
        await page.goto(base + '/register');
        await page.getByLabel('Отображаемое имя').fill('Автор обложки ' + viewport + scheme);
        await page.getByLabel('Электронная почта')
          .fill('cover.browser.' + viewport + '.' + scheme + '@example.test');
        await page.getByLabel('Пароль (12–128 символов)').fill('CoverBrowserStrongPassword2026');
        await page.getByRole('button', { name: 'Создать учётную запись' }).click();
        await page.waitForURL('**/me');

        await page.getByRole('link', { name: 'Мои книги' }).click();
        await page.getByRole('link', { name: 'Создать книгу' }).click();
        await page.getByLabel('Название книги').fill(title);
        await page.getByLabel('Язык').fill('ru');
        await page.getByRole('button', { name: 'Создать книгу' }).click();
        await page.waitForURL(/\/me\/books\/\d+$/);
        const bookId = page.url().split('/').pop();

        await page.getByRole('link', { name: 'Добавить главу' }).click();
        await page.getByLabel('Номер главы').fill('1');
        await page.getByLabel('Название главы').fill('Проверочная глава');
        await page.getByLabel('Текст главы').fill('Опубликованный текст с обложкой.');
        await page.getByRole('button', { name: 'Сохранить главу' }).click();
        await page.waitForURL(base + '/me/books/' + bookId);
        await page.getByRole('button', { name: 'Опубликовать книгу' }).click();
        await page.waitForURL(base + '/books/' + bookId);

        let response = await page.goto(base + '/me/books/' + bookId + '/settings/covers');
        assert(response && response.status() === 200,
          viewport + '/' + scheme + ': real owner cover page unavailable');
        const csp = response.headers()['content-security-policy'];
        assert(csp && csp.includes("script-src 'self'") && csp.includes("img-src 'self' blob:"),
          'Page-specific restrictive CSP lost');
        await shot(page, viewport, scheme, 'empty');

        const file = await actualPng(page);
        await page.locator('[data-cover-picker-input]').setInputFiles({
          name: 'ci-real-cover.png', mimeType: 'image/png', buffer: file,
        });
        await page.waitForFunction(() => {
          const image = document.querySelector('[data-cover-picker-image]');
          return image && !image.hidden && image.src.startsWith('blob:');
        });
        assert(await page.locator('[data-cover-picker-filename]').innerText()
          === 'ci-real-cover.png', 'NEwBOO local preview did not update');
        await shot(page, viewport, scheme, 'selected');

        await page.getByRole('button', { name: 'Загрузить обложку' }).click();
        await page.waitForURL(base + '/me/books/' + bookId + '/settings/covers');
        await readyCover(page, bookId, viewport, scheme);
        await shot(page, viewport, scheme, 'ready');

        const publicBook = await page.goto(base + '/books/' + bookId);
        assert(publicBook && publicBook.status() === 200, 'Published book lost');
        assert(await page.locator('img[src="/covers/' + bookId + '/main.webp"]').count() === 1,
          'Published book does not display its verified cover');
        const card = await context.request.get(base + '/covers/' + bookId + '/card.webp');
        const main = await context.request.get(base + '/covers/' + bookId + '/main.webp');
        for (const [kind, image] of [['card', card], ['main', main]]) {
          assert(image.status() === 200, kind + ': public WebP HTTP is not readable');
          const bytes = await image.body();
          assert(bytes.subarray(0, 4).toString() === 'RIFF' &&
            bytes.subarray(8, 12).toString() === 'WEBP',
          kind + ': invalid WebP bytes');
        }
        await page.goto(base + '/catalog?q=' + encodeURIComponent(title));
        assert(await page.locator('img[src="/covers/' + bookId + '/card.webp"]').count() >= 1,
          'Cover missing from real search results');
        await page.goto(base + '/');
        assert(await page.locator('img[src="/covers/' + bookId + '/card.webp"]').count() >= 1,
          'Cover missing from real home showcase');

        const anon = await browser.newContext();
        const denied = await anon.request.get(base + '/me/books/' + bookId + '/settings/covers', {
          maxRedirects: 0,
        });
        assert(denied.status() === 303,
          'Anonymous browser must not open owner cover settings');
        await anon.close();
        await context.close();
        console.log('H-web-11 PASS: ' + viewport + ' ' + scheme +
          ' real session -> published book -> S3 -> PostgreSQL worker -> public WebP');
      }
    }
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
