'use strict';
// Loop 1: real browser and disposable PostgreSQL; no external email/push.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const { approvePendingBookForIsolatedCi } = require('./site_moderation_ci.cjs');

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
      const profileContext = await browser.newContext({ viewport: { width, height } });
      try {
        const owner = await ownerContext.newPage();
        const reader = await readerContext.newPage();
        const profileFollower = await profileContext.newPage();
        const nonce = (process.env.GITHUB_RUN_ID || 'local') + '-' +
          Date.now() + '-' + screen;
        const authorName = 'Автор петли ' + nonce;
        const profileUrl = base + '/users/' + encodeURIComponent(authorName);

        await register(owner, authorName, 'loop1.author.' + nonce + '@example.invalid');
        await owner.goto(base + '/me/books');
        await owner.locator('#main').getByRole('link', { name: 'Создать книгу' }).click();
        await owner.waitForURL(base + '/me/books/new');
        await owner.getByLabel('Название книги').fill('Книга петли ' + nonce);
        await owner.getByLabel('Язык', { exact: true }).fill('ru');
        await owner.getByRole('button', { name: 'Создать книгу' }).click();
        await owner.waitForURL(/\/me\/books\/\d+$/);
        const bookId = owner.url().split('/').pop();
        assert(/^\d+$/.test(bookId), screen + ': invalid author book id');

        await owner.getByRole('link', { name: 'Добавить главу' }).click();
        await owner.waitForURL('**/chapters/new');
        assert(await owner.getByLabel('Номер главы').inputValue() === '1',
          screen + ': automatic first chapter number must be 1');
        await owner.getByLabel('Название главы').fill('Первый выпуск петли');
        await owner.getByLabel('Текст главы').fill('Начало истории петли.');
        await owner.getByRole('button', { name: 'Сохранить главу' }).click();
        await owner.waitForURL(base + '/me/books/' + bookId);
        await owner.getByRole('button', { name: 'Опубликовать книгу' }).click();
        await owner.waitForURL(base + '/books/' + bookId);
        const awaitingSiteApproval = await owner.goto(base + '/books/' + bookId);
        assert(awaitingSiteApproval && awaitingSiteApproval.status() === 404,
          screen + ': author release bypassed mandatory site moderation');
        approvePendingBookForIsolatedCi(bookId);
        const approvedBook = await owner.goto(base + '/books/' + bookId);
        assert(approvedBook && approvedBook.status() === 200,
          screen + ': site-approved book was not publicly available');

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

        // S014: a separate reader follows the author from the public profile,
        // without ever creating a Book follow or purchasing a chapter.
        await register(profileFollower, 'Читатель автора ' + nonce,
          'loop1.profile.' + nonce + '@example.invalid');
        const authorProfile = await profileFollower.goto(profileUrl);
        assert(authorProfile && authorProfile.status() === 200,
          screen + ': author profile is unavailable to signed reader');
        await profileFollower.getByRole('button', { name: 'Подписаться', exact: true }).click();
        assert(await profileFollower.getByRole('button', {
          name: 'Отписаться', exact: true,
        }).isVisible(), screen + ': profile follow did not persist');
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
        assert(await owner.getByLabel('Номер главы').inputValue() === '2',
          screen + ': automatic second chapter number must be 2');
        await owner.getByLabel('Название главы').fill('Второй выпуск петли');
        await owner.getByLabel('Текст главы').fill('Продолжение истории петли.');
        await owner.getByRole('button', { name: 'Сохранить главу' }).click();
        await owner.waitForURL(base + '/me/books/' + bookId);

        // Verify the profile-only subscription receives this fresh release.
        await profileFollower.goto(base + '/me/library?view=updates');
        const profileUpdate = profileFollower.locator('#new-chapters .library-update-card')
          .filter({ hasText: 'Второй выпуск петли' });
        assert(await profileUpdate.count() === 1,
          screen + ': profile-only author release missing or duplicated');
        const profileChapter = await profileUpdate.getByRole('link', { name: 'Читать' })
          .getAttribute('href');
        assert(/^\/read\/\d+$/.test(profileChapter || ''),
          screen + ': profile-follow update has no readable chapter link');
        await profileUpdate.getByRole('link', { name: 'Читать' }).click();
        await profileFollower.waitForURL(base + profileChapter);
        assert(await profileFollower.getByText('Продолжение истории петли.').isVisible(),
          screen + ': profile-follow release cannot be read');
        await screenshot(profileFollower, screen, 'profile-follow-return');
        await profileFollower.goto(profileUrl);
        await profileFollower.getByRole('button', { name: 'Отписаться', exact: true }).click();
        assert(await profileFollower.getByRole('button', {
          name: 'Подписаться', exact: true,
        }).isVisible(), screen + ': profile unsubscribe did not persist');

        await reader.goto(base + '/me/library?view=updates');
        let update = reader.locator('#new-chapters .library-update-card')
          .filter({ hasText: 'Второй выпуск петли' });
        assert(await update.count() === 1, screen + ': release missing or duplicated in feed');
        const target = await update.getByRole('link', { name: 'Читать' }).getAttribute('href');
        assert(/^\/read\/\d+$/.test(target || ''),
          screen + ': free update does not open authenticated completion-capable reader');
        assert(await update.getByRole('button', { name: 'Отметить прочитанной' }).isVisible(),
          screen + ': no unread notification receipt');
        await screenshot(reader, screen, 'new-chapter');

        await update.getByRole('button', { name: 'Отметить прочитанной' }).click();
        await reader.waitForURL(base + '/me/library?view=updates#new-chapters');
        update = reader.locator('#new-chapters .library-update-card')
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
        await profileContext.close();
      }
    }
  } finally {
    await browser.close();
  }
  console.log(
    'TOR Loop 01 Chromium PASS: desktop/mobile real accounts, demand, owner release, ' +
    'on-site update, profile-only author follow, read-state and completed reader return'
  );
})().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
