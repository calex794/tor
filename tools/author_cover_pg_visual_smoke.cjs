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
        // S019: exercise the header action at both viewport sizes.
        if (viewport === 'desktop') {
          const create = page.getByRole('navigation', { name: 'Основная навигация' })
            .getByRole('link', { name: 'Создать книгу' });
          assert(await create.isVisible(), 'Desktop header must offer book registration');
          await create.click();
        } else {
          await page.locator('.site-mobile-menu__trigger').click();
          const create = page.getByRole('navigation', { name: 'Мобильная навигация' })
            .getByRole('link', { name: 'Создать книгу' });
          assert(await create.isVisible(), 'Mobile menu must offer book registration');
          await create.click();
        }
        await page.getByLabel('Название книги').fill(title);
        await page.getByLabel('Язык', { exact: true }).fill('ru');
        await page.getByRole('button', { name: 'Создать книгу' }).click();
        await page.waitForURL(/\/me\/books\/\d+$/);
        const bookId = page.url().split('/').pop();

        // A second form with the same title must show a useful 409, not a blank error.
        await page.goto(base + '/me/books/new');
        await page.getByLabel('Название книги').fill(title);
        await page.getByLabel('Язык', { exact: true }).fill('ru');
        const [duplicateResponse] = await Promise.all([
          page.waitForResponse(r => r.url() === base + '/me/books' &&
            r.request().method() === 'POST'),
          page.getByRole('button', { name: 'Создать книгу' }).click(),
        ]);
        assert(duplicateResponse.status() === 409,
          'S019: duplicate title must return 409');
        assert(await page.getByRole('heading', { name: 'Книга не создана' }).isVisible(),
          'S019: duplicate title must explain the failure in Russian');
        assert(await page.getByRole('link', { name: 'Проверить каталог' }).isVisible(),
          'S019: duplicate response must offer a useful next action');
        // S020: edited Chinese and English aliases are claimed and protected.
        await page.goto(base + '/me/books/' + bookId + '/settings/titles');
        const chineseAlias = '修仙之路 ' + viewport + ' ' + scheme;
        const englishAlias = 'Immortal Path ' + viewport + ' ' + scheme;
        await page.getByLabel('Оригинальное название').fill(chineseAlias);
        await page.getByLabel('Альтернативное название 1 (например, английское)')
          .fill(englishAlias);
        await page.getByRole('button', { name: 'Сохранить названия' }).click();
        await page.waitForURL(base + '/me/books/' + bookId);
        await page.goto(base + '/me/books/' + bookId + '/settings/titles');
        assert(await page.getByLabel('Оригинальное название').inputValue() === chineseAlias,
          'S020: edited Chinese original title must persist');
        assert(await page.getByLabel('Альтернативное название 1 (например, английское)')
          .inputValue() === englishAlias, 'S020: edited English alias must persist');
        await page.goto(base + '/me/books/new');
        await page.getByLabel('Название книги').fill('Другая книга ' + viewport + scheme);
        await page.getByLabel('Оригинальное название').fill(chineseAlias);
        const [aliasConflict] = await Promise.all([
          page.waitForResponse(r => r.url() === base + '/me/books' &&
            r.request().method() === 'POST'),
          page.getByRole('button', { name: 'Создать книгу' }).click(),
        ]);
        assert(aliasConflict.status() === 409,
          'S020: edited Chinese title blocks new duplicate registration');
        // S020: change origin language/year/category independently of publication.
        await page.goto(base + '/me/books/' + bookId + '/settings/origin');
        await page.getByLabel('Язык оригинала (например, zh, en)').fill('zh');
        await page.getByLabel('Год выпуска').fill('2024');
        await page.getByLabel('Раздел каталога').selectOption('novels/chinese');
        await page.getByRole('button', { name: 'Сохранить сведения' }).click();
        await page.waitForURL(base + '/me/books/' + bookId);
        await page.goto(base + '/me/books/' + bookId + '/settings/origin');
        assert(await page.getByLabel('Язык оригинала (например, zh, en)').inputValue() === 'zh',
          'S020: original language persists');
        assert(await page.getByLabel('Год выпуска').inputValue() === '2024',
          'S020: release year persists');
        assert(await page.getByLabel('Раздел каталога').inputValue() === 'novels/chinese',
          'S020: catalog section persists');
        // S019: the same normalized URL from any site must not create a duplicate.
        // Keep query and path semantics; no external URL fetch.
        const providerId = (viewport === 'desktop' ? 910100 : 910200)
          + (scheme === 'light' ? 1 : 2);
        await page.goto(base + '/me/books/new');
        await page.getByLabel('Название книги').fill('Исходная книга ' + providerId);
        await page.getByLabel('Ссылка на первоисточник')
          .fill('http://www.example.org/work/' + providerId + '/');
        await page.getByRole('button', { name: 'Создать книгу' }).click();
        await page.waitForURL(/\/me\/books\/\d+$/);
        await page.goto(base + '/me/books/new');
        await page.getByLabel('Название книги').fill('Совсем другое название ' + providerId);
        await page.getByLabel('Ссылка на первоисточник')
          .fill('https://example.org/work/' + providerId);
        const [providerDuplicate] = await Promise.all([
          page.waitForResponse(r => r.url() === base + '/me/books' &&
            r.request().method() === 'POST'),
          page.getByRole('button', { name: 'Создать книгу' }).click(),
        ]);
        assert(providerDuplicate.status() === 409,
          'S019: identical normalized URL under changed title must be rejected');
        assert(await page.getByRole('heading', { name: 'Книга не создана' }).isVisible(),
          'S019: generic URL conflict must preserve actionable error page');
        // S019: an actual original and a mirror may be from arbitrary hosts.
        // Registration must claim both in one transaction without fetching them.
        const workNum = providerId + 500000;
        const chineseOriginal = 'https://fiction.example.cn/novel/' + workNum;
        const mirrorLink = 'https://mirror.example.org/work/' + workNum;
        await page.goto(base + '/me/books/new');
        await page.getByLabel('Название книги')
          .fill('Перевод китайского оригинала ' + workNum);
        await page.getByLabel('Ссылка на первоисточник').fill(chineseOriginal);
        await page.getByLabel('Дополнительная ссылка на это произведение').fill(mirrorLink);
        await page.getByRole('button', { name: 'Создать книгу' }).click();
        await page.waitForURL(/\/me\/books\/\d+$/);
        const sourceBookId = page.url().split('/').pop();
        await page.getByRole('link', { name: 'Ссылки на произведение' }).click();
        assert(await page.getByLabel('Ссылка на оригинал').inputValue() === chineseOriginal,
          'S019: source settings must show the existing original');
        assert(await page.getByLabel('Дополнительная ссылка на произведение')
          .inputValue() === mirrorLink, 'S019: existing mirror must be shown');
        const editedMirror = 'https://mirror.example.org/updated/' + workNum;
        await page.getByLabel('Дополнительная ссылка на произведение').fill(editedMirror);
        const [linksSaved] = await Promise.all([
          page.waitForResponse(r => r.url() === base + '/me/books/' + sourceBookId +
            '/settings/sources' && r.request().method() === 'POST'),
          page.getByRole('button', { name: 'Сохранить ссылки' }).click(),
        ]);
        assert(linksSaved.status() === 303, 'S019: owner must be able to save URLs');
        await page.waitForURL(base + '/me/books/' + sourceBookId);
        await page.goto(base + '/me/books/' + sourceBookId + '/settings/sources');
        assert(await page.getByLabel('Дополнительная ссылка на произведение')
          .inputValue() === editedMirror, 'S019: saved mirror must survive reload');

        await page.goto(base + '/me/books/new');
        await page.getByLabel('Название книги').fill('Другое название зеркала ' + workNum);
        await page.getByLabel('Ссылка на первоисточник').fill(editedMirror);
        const [mirrorDuplicate] = await Promise.all([
          page.waitForResponse(r => r.url() === base + '/me/books' &&
            r.request().method() === 'POST'),
          page.getByRole('button', { name: 'Создать книгу' }).click(),
        ]);
        assert(mirrorDuplicate.status() === 409,
          'S019: a supplied mirror URL must prevent duplicate registration');
        assert(await page.getByRole('heading', { name: 'Книга не создана' }).isVisible(),
          'S019: conflict must not reveal private book details');
        await page.goto(base + '/me/books/' + bookId);

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

        // S019: published book appears as a hint before a duplicate is submitted.
        await page.goto(base + '/me/books/new');
        await page.getByLabel('Название книги').fill(title);
        await page.waitForFunction(id =>
          document.querySelector('#book-title-suggestions a[href="/books/' + id + '"]') !== null,
          bookId, { timeout: 10000 });
        const hint = page.locator('#book-title-suggestions');
        assert(await hint.getByRole('link', { name: title }).isVisible(),
          'S019: published title hint must point to the real book');
        await page.goto(base + '/books/' + bookId);
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
