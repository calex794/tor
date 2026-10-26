// Real desktop/mobile acceptance: register -> draft -> chapter -> release -> read.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');

const base = process.env.TOR_BASE_URL || 'http://127.0.0.1:3098';
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
  const browser = await chromium.launch({
    headless: true,
    executablePath: process.env.TOR_CHROMIUM_EXECUTABLE || undefined,
  });
  const runId = Date.now().toString(36);
  try {
    for (const [viewport, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const context = await browser.newContext({ viewport: { width, height } });
      const page = await context.newPage();
      await page.goto(base + '/register');
      await page.getByLabel('Отображаемое имя').fill('Автор ' + viewport);
      await page.getByLabel('Электронная почта').fill('author.chapter.' + viewport + '.' + runId + '@example.test');
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
      await page.getByRole('link', { name: 'Редактировать' }).click();
      await page.waitForURL(/\/chapters\/\d+\/edit$/);
      assert(await page.getByRole('heading', { name: 'Редактировать главу' }).isVisible(),
        viewport + ': missing draft editor');
      assert(await page.getByText('Редакция №1.').isVisible(),
        viewport + ': initial chapter revision missing');
      await page.getByLabel('Текст главы').fill('Обновлённая версия: **важная строка**.');
      await inspect(page, viewport, 'edit-draft');
      await page.getByRole('button', { name: 'Сохранить изменения' }).click();
      await page.waitForURL(base + '/me/books/' + bookId);
      assert(await page.getByText(/Черновик/).isVisible(),
        viewport + ': draft edit must not publish book');

      // Empty 404 navigations can surface as net::ERR_HTTP_RESPONSE_CODE_FAILURE
      // in Chromium. The context request shares the browser's authenticated cookies.
      const hidden = await context.request.get(base + '/books/' + bookId);
      assert(hidden.status() === 404,
        viewport + ': saved chapter exposed before explicit publication');
      assert(!(await hidden.text()).includes('Обновлённая версия:'),
        viewport + ': private chapter text leaked in the denial response');
      await page.goto(base + '/me/books/' + bookId);
      await page.getByRole('button', { name: 'Опубликовать книгу' }).click();
      await page.waitForURL(base + '/books/' + bookId);
      assert(await page.getByRole('heading', { name: 'Новая книга ' + viewport }).isVisible(),
        viewport + ': public book missing after release');
      await inspect(page, viewport, 'published');

      await page.getByRole('link', { name: /Глава 1.*Глава для проверки/ }).click();
      await page.waitForURL(/\/reader\/\d+$/);
      assert(await page.getByText('Обновлённая версия:').isVisible(),
        viewport + ': published chapter text missing');
      await inspect(page, viewport, 'reader');

      // B-06: a second chapter on this ALREADY PUBLIC book remains hidden.
      await page.goto(base + '/me/books/' + bookId);
      await page.getByRole('link', { name: 'Добавить главу' }).click();
      await page.waitForURL('**/chapters/new');
      await page.getByLabel('Номер главы').fill('2');
      await page.getByLabel('Название главы').fill('Вторая глава закрыта');
      await page.getByLabel('Текст главы').fill('PRIVATE B06 old');
      await page.getByRole('button', { name: 'Сохранить главу' }).click();
      await page.waitForURL(base + '/me/books/' + bookId);
      assert(await page.getByText(/Черновик \(не виден читателям\)/).isVisible(),
        viewport + ': chapter draft visibility marker missing');
      await inspect(page, viewport, 'second-draft');
      const publicDraftBook = await page.goto(base + '/books/' + bookId);
      assert(publicDraftBook && publicDraftBook.status() === 200,
        viewport + ': public book lost when second chapter saved');
      assert(!(await page.content()).includes('Вторая глава закрыта'),
        viewport + ': second chapter leaked into public table of contents');

      await page.goto(base + '/me/books/' + bookId);
      await page.getByRole('link', { name: 'Редактировать' }).click();
      await page.waitForURL(/\/chapters\/\d+\/edit$/);
      await page.getByLabel('Текст главы').fill('PUBLIC B06 new');
      await page.getByRole('button', { name: 'Сохранить изменения' }).click();
      await page.waitForURL(base + '/me/books/' + bookId);
      await page.getByRole('link', { name: 'Выпустить главу' }).click();
      await page.waitForURL(/\/chapters\/\d+\/release$/);
      assert(await page.getByText('PUBLIC B06 new').isVisible(),
        viewport + ': confirmation must preview the latest draft');
      await inspect(page, viewport, 'release-confirmation');
      await page.getByRole('button', { name: 'Опубликовать эту главу' }).click();
      await page.waitForURL(base + '/books/' + bookId);
      assert(await page.getByRole('link', { name: /Глава 2.*Вторая глава закрыта/ }).isVisible(),
        viewport + ': second chapter not public after owner release');
      await inspect(page, viewport, 'second-released');
      await page.getByRole('link', { name: /Глава 2.*Вторая глава закрыта/ }).click();
      await page.waitForURL(/\/reader\/\d+$/);
      assert(await page.getByText('PUBLIC B06 new').isVisible(),
        viewport + ': reader does not show approved second version');
      await inspect(page, viewport, 'second-reader');
      await context.close();
    }
    console.log('TOR B-06 browser PASS: 390/1440 public book, hidden second chapter, edit, confirm, release, reader');
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
