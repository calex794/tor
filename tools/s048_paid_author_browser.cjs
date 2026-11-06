// S048: real paid author editor and publication through Axum + PG18 + MinIO.
// Credentials are ephemeral test-only process environment, never logged.
const assert = require('node:assert/strict');
const { chromium } = require('playwright');
const origin = 'http://127.0.0.1:3098';
const book = process.env.S048_BOOK_ID;
const chapter = process.env.S048_CHAPTER_ID;
const owner = process.env.S048_OWNER_SESSION;
const other = process.env.S048_OTHER_SESSION;
const csrf = process.env.S048_CSRF;
assert(/^\d+$/.test(book) && /^\d+$/.test(chapter) &&
  owner && other && csrf, 'isolated S048 fixture missing');
const editor = origin + '/me/books/' + book + '/chapters/' + chapter + '/edit';
const issuesPage = origin + '/me/books/' + book + '/issues';
function cookies(session) {
  return [
    { name: 'tor-dev-session', value: session, url: origin },
    { name: 'tor-dev-csrf', value: csrf, url: origin },
  ];
}
(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [device, width, previous, next] of [
      ['desktop', 1440, 'Owner correction V3', 'S048 browser correction V4'],
      ['mobile', 390, 'S048 browser correction V4', 'S048 browser correction V5'],
    ]) {
      const context = await browser.newContext({ viewport: { width, height: 900 } });
      const otherContext = await browser.newContext({ viewport: { width, height: 900 } });
      try {
        await context.addCookies(cookies(owner));
        await otherContext.addCookies(cookies(other));
        const stranger = await otherContext.newPage();
        const denied = await stranger.goto(editor);
        assert.equal(denied.status(), 404, device + ': another user reached paid editor');
        assert(!(await stranger.content()).includes(previous),
          device + ': paid plaintext leaked to another user');
        const page = await context.newPage();
        const response = await page.goto(editor);
        assert.equal(response.status(), 200, device + ': owner paid editor unavailable');
        assert.equal(await page.getByLabel('Текст главы (Markdown, до 50 000 символов)').inputValue(),
          previous, device + ': expected exact current protected edition');
        assert(await page.getByLabel('Название главы').getAttribute('readonly') !== null,
          device + ': paid chapter title not frozen');
        assert(await page.getByLabel('Номер главы').getAttribute('readonly') !== null,
          device + ': chapter number not frozen');
        assert((await page.locator('main').innerText()).includes('100 копеек'),
          device + ': immutable paid price missing');
        const sizes = await page.evaluate(() =>
          [document.documentElement.scrollWidth, document.documentElement.clientWidth]);
        assert(sizes[0] <= sizes[1] + 1, device + ': horizontal overflow');
        await page.getByLabel('Текст главы (Markdown, до 50 000 символов)').fill(next);
        await page.getByRole('button', { name: 'Сохранить изменения', exact: true }).click();
        await page.waitForURL(issuesPage);
        const edited = await page.goto(editor);
        assert.equal(edited.status(), 200, device + ': updated editor unavailable');
        assert.equal(await page.getByLabel('Текст главы (Markdown, до 50 000 символов)').inputValue(),
          next, device + ': paid revision not published and reloaded');
      } finally {
        await otherContext.close();
        await context.close();
      }
    }
    console.log('S048 Chromium: owner paid V3->V4->V5, denied other actor, desktop/mobile PASS');
  } finally {
    await browser.close();
  }
})().catch(error => {
  // Do not print cookies, keys, chapter plaintext or page HTML.
  console.error('S048 Chromium failed: ' + error.name);
  process.exitCode = 1;
});
