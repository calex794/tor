'use strict';
// Loop 1: real browser and disposable PostgreSQL; no external email/push.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');

const base = 'http://127.0.0.1:3098';
const shots = process.env.TOR_READER_SHOTS || 'reader-shots';
fs.mkdirSync(shots, { recursive: true });

function assert(condition, why) {
  if (!condition) throw new Error('loop-01: ' + why);
}

async function register(page, name, address) {
  await page.goto(base + '/register');
  await page.getByLabel('Отображаемое имя').fill(name);
  await page.getByLabel('Электронная почта').fill(address);
  await page.getByLabel('Пароль (12–128 символов)').fill('LoopOneBrowser2026Pass');
  await page.getByRole('button', { name: 'Создать учётную запись' }).click();
  await page.waitForURL(base + '/me');
}

async function screenshot(page, screen, step) {
  const width = await page.evaluate(() => ({
    html: document.documentElement.scrollWidth,
    viewport: document.documentElement.clientWidth,
    body: document.body.scrollWidth,
  }));
  assert(width.html <= width.viewport + 1 && width.body <= width.viewport + 1,
    screen + '/' + step + ': horizontal overflow ' + JSON.stringify(width));
  await page.screenshot({
    path: path.join(shots, 'loop-01-' + screen + '-' + step + '.png'),
    fullPage: true,
  });
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [screen, width, height] of [
      ['desktop', 1440, 900], ['mobile', 390, 844],
    ]) {
      const ownerContext = await browser.newContext({ viewport: { width, height } });
      const readerContext = await browser.newContext({ viewport: { width, height } });
      try {
        const owner = await ownerContext.newPage();
        const reader = await readerContext.newPage();
        const nonce = (process.env.GITHUB_RUN_ID || 'local') + '-' +
          Date.now() + '-' + screen;

        await register(owner, 'Автор петли', 'loop1.author.' + nonce + '@example.invalid');
        await owner.goto(base + '/me/books');
        await owner.getByRole('link', { name: 'Создать книгу' }).click();
        await owner.waitForURL(base + '/me/books/new');
        await owner.getByLabel('Название книги').fill('Книга петли ' + nonce);
        await owner.getByLabel('Язык').fill('ru');
        await owner.getByRole('button', { name: 'Создать книгу' }).click();
        await owner.waitForURL(/\/me\/books\/\d+$/);
        const bookId = owner.url().split('/').pop();
        assert(/^\d+$/.test(bookId), screen + ': invalid author book id');

        await owner.getByRole('link', { name: 'Добавить главу' }).click();
        await owner.waitForURL('**/chapters/new');
        await owner.getByLabel('Номер главы').fill('1');
        await owner.getByLabel('Название главы').fill('Первый выпуск петли');
        await owner.getByLabel('Текст главы').fill('Начало истории петли.');
        await owner.getByRole('button', { name: 'Сохранить главу' }).click();
        await owner.waitForURL(base + '/me/books/' + bookId);
        await owner.getByRole('button', { name: 'Опубликовать книгу' }).click();
        await owner.waitForURL(base + '/books/' + bookId);

        const firstLink = owner.getByRole('link', {
          name: /Глава 1.*Первый выпуск петли/,
        });
        assert(await firstLink.isVisible(), screen + ': first public chapter missing');
        const firstHref = await firstLink.getAttribute('href');
        assert(/^\/reader\/\d+$/.test(firstHref || ''),
          screen + ': invalid first public chapter link');
        const firstChapter = firstHref.split('/').pop();

        await register(reader, 'Читатель петли', 'loop1.reader.' + nonce + '@example.invalid');
        await reader.goto(base + '/books/' + bookId);
        await reader.getByRole('button', { name: 'Следить за книгой' }).click();
        await reader.goto(base + '/read/' + firstChapter);
        assert(await reader.getByRole('button', { name: 'Глава прочитана' }).isVisible(),
          screen + ': no real end-of-chapter completion control');
        await reader.getByRole('button', { name: 'Глава прочитана' }).click();
        await reader.waitForURL(base + '/read/' + firstChapter + '#continuation');
        assert(await reader.getByRole('link', { name: 'Мои рейтинги' }).isVisible(),
          screen + ': completed chapter lost the optional ranking link');
        assert(await reader.getByRole('button', { name: 'Хочу продолжение' }).isVisible(),
          screen + ': demand action unavailable after completed chapter');
        await reader.getByRole('button', { name: 'Хочу продолжение' }).click();
        assert(await reader.getByText(/Вы уже сообщили автору/).isVisible(),
          screen + ': demand not acknowledged');

        await owner.goto(base + '/me/books/' + bookId);
        assert((await owner.content()).includes(
          'Запросов после последней опубликованной главы: <strong>1</strong>'
        ), screen + ': owner demand not visible');
        await owner.getByRole('link', { name: 'Добавить главу' }).click();
        await owner.waitForURL('**/chapters/new');
        await owner.getByLabel('Номер главы').fill('2');
        await owner.getByLabel('Название главы').fill('Второй выпуск петли');
        await owner.getByLabel('Текст главы').fill('Продолжение истории петли.');
        await owner.getByRole('button', { name: 'Сохранить главу' }).click();
        await owner.waitForURL(base + '/me/books/' + bookId);

        await reader.goto(base + '/me/library');
        let update = reader.locator('#new-chapters .library-card')
          .filter({ hasText: 'Второй выпуск петли' });
        assert(await update.count() === 1, screen + ': release missing or duplicated in feed');
        const target = await update.getByRole('link', { name: 'Читать' }).getAttribute('href');
        assert(/^\/read\/\d+$/.test(target || ''),
          screen + ': free update does not open authenticated completion-capable reader');
        assert(await update.getByRole('button', { name: 'Отметить прочитанной' }).isVisible(),
          screen + ': no unread notification receipt');
        await screenshot(reader, screen, 'new-chapter');

        await update.getByRole('button', { name: 'Отметить прочитанной' }).click();
        await reader.waitForURL(base + '/me/library#new-chapters');
        update = reader.locator('#new-chapters .library-card')
          .filter({ hasText: 'Второй выпуск петли' });
        assert(await update.getByRole('button', { name: 'Отметить непрочитанной' }).isVisible(),
          screen + ': notification receipt not persisted');
        await update.getByRole('link', { name: 'Читать' }).click();
        await reader.waitForURL(base + target);
        assert(await reader.getByText('Продолжение истории петли.').isVisible(),
          screen + ': followed chapter text missing from private reader');
        assert(await reader.getByRole('button', { name: 'Глава прочитана' }).isVisible(),
          screen + ': following update cannot be completed as meaningful reading');
        await screenshot(reader, screen, 'returned-reader');
        await reader.getByRole('button', { name: 'Глава прочитана' }).click();
        await reader.waitForURL(base + target + '#continuation');
        assert(await reader.getByText('Глава уже отмечена прочитанной.').isVisible(),
          screen + ': returning reader completion not confirmed');

        await owner.goto(base + '/me/books/' + bookId);
        assert((await owner.content()).includes(
          'Запросов после последней опубликованной главы: <strong>0</strong>'
        ), screen + ': demand not reset after new release');
      } finally {
        await ownerContext.close();
        await readerContext.close();
      }
    }
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
