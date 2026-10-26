// Real desktop/mobile acceptance: register -> draft -> chapter -> release -> read.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');

const base = 'http://127.0.0.1:3098';
const out = process.env.TOR_AUTHOR_SHOTS || 'author-chapter-shots';
fs.mkdirSync(out, { recursive: true });

function assert(ok, reason) {
  if (!ok) throw new Error(reason);
}

async function inspect(page, viewport, name) {
  const sizes = await page.evaluate(() => ({
    doc: document.documentElement.scrollWidth,
    window: document.documentElement.clientWidth,
    body: document.body.scrollWidth,
  }));
  assert(
    sizes.doc <= sizes.window + 1 && sizes.body <= sizes.window + 1,
    viewport + '/' + name + ': horizontal overflow ' + JSON.stringify(sizes)
  );
  assert(await page.locator('h1').isVisible(), viewport + '/' + name + ': missing heading');
  await page.screenshot({
    path: path.join(out, 'author-' + viewport + '-' + name + '.png'),
    fullPage: true,
  });
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [viewport, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const context = await browser.newContext({ viewport: { width, height } });
      const page = await context.newPage();
      await page.goto(base + '/register');
      await page.getByLabel('Отображаемое имя').fill('Автор ' + viewport);
      await page.getByLabel('Электронная почта').fill('author.chapter.' + viewport + '@example.test');
      await page.getByLabel('Пароль (12–128 символов)').fill('TestAuthorPassword2026');
      await page.getByRole('button', { name: 'Создать учётную запись' }).click();
      await page.waitForURL('**/me');

      await page.getByRole('link', { name: 'Мои книги' }).click();
      await page.waitForURL('**/me/books');
      await page.getByRole('link', { name: 'Создать книгу' }).click();
      await page.waitForURL('**/me/books/new');
      await page.getByLabel('Название книги').fill('Новая книга ' + viewport);
      await page.getByLabel('Язык').fill('ru');
      await page.getByRole('button', { name: 'Создать книгу' }).click();
      await page.waitForURL(/\/me\/books\/\d+$/);

      const bookId = page.url().split('/').pop();
      assert(await page.getByRole('link', { name: 'Добавить главу' }).isVisible(),
        viewport + ': missing author chapter action');
      await inspect(page, viewport, 'draft');
      await page.getByRole('link', { name: 'Добавить главу' }).click();
      await page.waitForURL('**/chapters/new');
      await page.getByLabel('Номер главы').fill('1');
      await page.getByLabel('Название главы').fill('Глава для проверки');
      await page.getByLabel('Текст главы').fill('Проверочный текст: **важная строка**.');
      await inspect(page, viewport, 'editor');
      await page.getByRole('button', { name: 'Сохранить главу' }).click();
      await page.waitForURL(base + '/me/books/' + bookId);
      assert(await page.getByText('Глава для проверки').isVisible(),
        viewport + ': saved chapter absent from author list');
      assert(await page.getByText(/Черновик/).isVisible(),
        viewport + ': saving chapter must not publish the book');
      assert(await page.getByRole('button', { name: 'Опубликовать книгу' }).isVisible(),
        viewport + ': missing explicit publishing control');
      await inspect(page, viewport, 'chapter-saved');

      const hidden = await page.goto(base + '/books/' + bookId);
      assert(hidden && hidden.status() === 404,
        viewport + ': saved chapter exposed before explicit publication');
      await page.goto(base + '/me/books/' + bookId);
      await page.getByRole('button', { name: 'Опубликовать книгу' }).click();
      await page.waitForURL(base + '/books/' + bookId);
      assert(await page.getByRole('heading', { name: 'Новая книга ' + viewport }).isVisible(),
        viewport + ': public book missing after release');
      await inspect(page, viewport, 'published');

      await page.getByRole('link', { name: /Глава 1.*Глава для проверки/ }).click();
      await page.waitForURL(/\/reader\/\d+$/);
      assert(await page.getByText('Проверочный текст:').isVisible(),
        viewport + ': published chapter text missing');
      await inspect(page, viewport, 'reader');
      await context.close();
    }
    console.log('TOR author chapter browser PASS: 390/1440, real PG, draft isolation, deliberate publish');
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
