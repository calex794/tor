// F-05: actual Chromium + disposable PostgreSQL.
// S008 browser acceptance: only actual C-owned chapter completion may make
// a book eligible for ranking. Never seed private ranking tables from SQL.
const { chromium } = require('playwright');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const base = 'http://127.0.0.1:3098';
const db = process.env.APP_URL;
const shots = process.env.TOR_RANKING_SHOTS || 'ranking-shots';
fs.mkdirSync(shots, { recursive: true });
function assert(ok, message) { if (!ok) throw new Error(message); }
function pg(sql) {
  assert(db && db.endsWith('/tor_ci'), 'F05 refuses non-test PostgreSQL');
  return execFileSync('psql', ['-X', '-v', 'ON_ERROR_STOP=1', '-At', '-d', db, '-c', sql],
    { encoding: 'utf8', timeout: 10000, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}
function positive(value) {
  // psql prints a command tag after INSERT ... RETURNING even with -At.
  // Select the single numeric row and reject missing/multiple IDs.
  const rows = String(value).split(/\r?\n/).map(v => v.trim())
    .filter(v => /^[1-9][0-9]*$/.test(v));
  assert(rows.length === 1, 'non-numeric database ID');
  return rows[0];
}
async function screenshot(page, name) {
  const size = await page.evaluate(() => ({
    content: document.documentElement.scrollWidth,
    viewport: document.documentElement.clientWidth,
    body: document.body.scrollWidth
  }));
  assert(size.content <= size.viewport + 1 && size.body <= size.viewport + 1,
    name + ': mobile horizontal overflow ' + JSON.stringify(size));
  await page.screenshot({ path: path.join(shots, 'ranking-' + name + '.png'), fullPage: true });
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [viewport, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const context = await browser.newContext({ viewport: { width, height } });
      const page = await context.newPage();
      const email = 'f05.visual.' + viewport + '@example.test';
      await page.goto(base + '/register');
      await page.getByLabel('Отображаемое имя').fill('Читатель F05 ' + viewport);
      await page.getByLabel('Электронная почта').fill(email);
      await page.getByLabel('Пароль (12–128 символов)').fill('F05BrowserSafePassword2026');
      await page.getByRole('button', { name: 'Создать учётную запись' }).click();
      await page.waitForURL('**/me');
      await page.getByRole('link', { name: 'Мой рейтинг' }).first().click();
      await page.waitForURL('**/me/rankings');
      assert(await page.getByText('Пока нет книг для сравнения').isVisible(),
        viewport + ': empty ranking must not invent completed reads');
      await screenshot(page, viewport + '-empty');

      const uid = positive(pg("SELECT id FROM users WHERE email_normalized='" + email + "'"));
      const a = positive(pg("INSERT INTO books(source_identity,target_language,title,visibility) " +
        "VALUES('ci:f05:visual:" + viewport + ":a','ru','Знакомая книга','public') RETURNING id"));
      const b = positive(pg("INSERT INTO books(source_identity,target_language,title,visibility) " +
        "VALUES('ci:f05:visual:" + viewport + ":b','ru','Любимая книга','public') RETURNING id"));
      // The public book records are fixtures, but chapter completion and
      // ranking eligibility must travel through real browser + C + outbox + F.
      const chapterA = positive(pg("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) " +
        "VALUES(" + a + ",1,'Глава А','public','free',0) RETURNING id"));
      const chapterB = positive(pg("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) " +
        "VALUES(" + b + ",1,'Глава Б','public','free',0) RETURNING id"));
      for (const chapterId of [chapterA, chapterB]) {
        const versionId = positive(pg("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) " +
          "VALUES(" + chapterId + ",1,'Проверенная глава рейтинга: **последний абзац**.') RETURNING id"));
        pg("INSERT INTO chapter_publications(chapter_id,version_id) VALUES(" +
          chapterId + "," + versionId + ")");
      }
      for (const [chapterId, expectedBookId] of [[chapterA, a], [chapterB, b]]) {
        const opened = await page.goto(base + '/read/' + chapterId, { waitUntil: 'networkidle' });
        assert(opened && opened.status() === 200,
          viewport + ': S008 verified public chapter unavailable');
        assert(await page.getByRole('button', { name: 'Глава прочитана' }).isVisible(),
          viewport + ': S008 C-owned completion button missing');
        assert(pg("SELECT count(*) FROM reader_chapter_completions WHERE user_id=" + uid +
          " AND book_id=" + expectedBookId) === '0',
          viewport + ': S008 GET falsely completed the chapter');
        await page.getByRole('button', { name: 'Глава прочитана' }).click();
        await page.waitForURL(base + '/read/' + chapterId + '#continuation');
        assert(pg("SELECT count(*) FROM reader_chapter_completions WHERE user_id=" + uid +
          " AND book_id=" + expectedBookId) === '1',
          viewport + ': S008 completion fact not stored exactly once');
      }
      assert(pg("SELECT count(*) FROM user_book_rank WHERE user_id=" + uid) === '1',
        viewport + ': S008 first trusted completion did not start a ranking');
      assert(pg("SELECT count(*) FROM user_book_ranking_state WHERE user_id=" + uid) === '1',
        viewport + ': S008 second trusted completion did not start a comparison');

      const active = await page.goto(base + '/me/rankings', { waitUntil: 'networkidle' });
      assert(active && active.status() === 200, viewport + ': ranked question HTTP failed');
      // A ranked book also appears in the private history; scope the active
      // question to avoid Playwright strict-mode ambiguity.
      const questionPair = page.locator('.ranking-pair');
      assert(await questionPair.getByRole('link', { name: 'Знакомая книга', exact: true }).isVisible(),
        viewport + ': original A title missing from question');
      assert(await questionPair.getByRole('link', { name: 'Любимая книга', exact: true }).isVisible(),
        viewport + ': original B title missing from question');
      assert(await page.getByRole('button', { name: 'Первая лучше' }).isVisible(),
        viewport + ': comparison controls missing');
      assert(await page.getByRole('button', { name: 'Отложить вторую' }).isVisible(),
        viewport + ': defer control missing');
      await screenshot(page, viewport + '-question');

      await page.getByRole('button', { name: 'Пропустить' }).click();
      await page.waitForURL('**/me/rankings');
      assert(await page.getByText('Все предложенные пары пропущены').isVisible(),
        viewport + ': skipped pair reappeared without explicit resume');
      await screenshot(page, viewport + '-exhausted');

      await page.getByRole('button', { name: 'Повторить вопросы' }).click();
      await page.waitForURL('**/me/rankings');
      assert(await page.getByRole('button', { name: 'Равнозначны' }).isVisible(),
        viewport + ': question was not explicitly resumed');
      await page.getByRole('button', { name: 'Равнозначны' }).click();
      await page.waitForURL('**/me/rankings');
      await page.getByRole('heading', { name: 'Мои книги' }).waitFor();
      const savedBooks = page.locator('.ranking-books .ranking-book');
      assert(await savedBooks.count() === 2, viewport + ': both tied books must be visible');
      assert(await page.locator('.ranking-book__tie').count() === 2,
        viewport + ': tie group not marked');
      assert((await page.locator('.ranking-book__place').allTextContents())
        .every(label => label === 'Место 1'), viewport + ': tied books got different places');
      assert(pg('SELECT count(*) FROM user_book_comparison WHERE user_id=' + uid) === '2',
        viewport + ': skip and tie were not recorded once each');
      await screenshot(page, viewport + '-placed');

      // Defer is a durable reader choice, not an automatic negative signal.
      // The deferred book may return only after a NEW verified C completion.
      const deferred = positive(pg("INSERT INTO books(source_identity,target_language,title,visibility) " +
        "VALUES('ci:f05:visual:" + viewport + ":deferred','ru','Книга после отсрочки','public') RETURNING id"));
      const deferredChapter = positive(pg("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) " +
        "VALUES(" + deferred + ",1,'Первая проверенная глава','public','free',0) RETURNING id"));
      const deferredVersion = positive(pg("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) " +
        "VALUES(" + deferredChapter + ",1,'Первая глава для отсрочки ' || repeat('д',1200)) RETURNING id"));
      pg("INSERT INTO chapter_publications(chapter_id,version_id) VALUES(" +
        deferredChapter + "," + deferredVersion + ")");
      await page.goto(base + '/read/' + deferredChapter);
      await page.getByRole('button', { name: 'Глава прочитана' }).click();
      await page.waitForURL(base + '/read/' + deferredChapter + '#continuation');
      await page.goto(base + '/me/rankings');
      const deferPair = page.locator('.ranking-pair');
      assert(await deferPair.getByRole('link', { name: 'Книга после отсрочки' }).isVisible(),
        viewport + ': S008 third verified book did not start a comparison');
      await page.getByRole('button', { name: 'Отложить вторую' }).click();
      await page.waitForURL('**/me/rankings');
      assert(pg("SELECT count(*) FROM user_book_ranking_defer WHERE user_id=" + uid +
        " AND book_id=" + deferred) === '1',
        viewport + ': S008 deferred book was not recorded');
      await page.reload();
      assert(await page.locator('.ranking-pair').count() === 0,
        viewport + ': S008 deferred book reappeared without fresh reading');

      const secondChapter = positive(pg("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) " +
        "VALUES(" + deferred + ",2,'Новая глава для возвращения','public','free',0) RETURNING id"));
      const secondVersion = positive(pg("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) " +
        "VALUES(" + secondChapter + ",1,'Новая глава после отсрочки ' || repeat('ч',1200)) RETURNING id"));
      pg("INSERT INTO chapter_publications(chapter_id,version_id) VALUES(" +
        secondChapter + "," + secondVersion + ")");
      await page.goto(base + '/read/' + secondChapter);
      await page.getByRole('button', { name: 'Глава прочитана' }).click();
      await page.waitForURL(base + '/read/' + secondChapter + '#continuation');
      assert(pg("SELECT count(*) FROM reader_chapter_completions WHERE user_id=" + uid +
        " AND book_id=" + deferred) === '2',
        viewport + ': S008 fresh C completion missing');
      assert(pg("SELECT count(*) FROM user_book_ranking_defer WHERE user_id=" + uid +
        " AND book_id=" + deferred) === '0',
        viewport + ': S008 fresh completion did not clear deferral');
      await page.goto(base + '/me/rankings');
      assert(await page.locator('.ranking-pair').getByRole('link',
        { name: 'Книга после отсрочки' }).isVisible(),
        viewport + ': S008 deferred book did not return after fresh reading');
      await page.getByRole('button', { name: 'Первая лучше' }).click();
      await page.waitForURL('**/me/rankings');
      assert(await page.locator('.ranking-books .ranking-book').count() === 3,
        viewport + ': S008 accepted fresh completion did not rank third book');
      await screenshot(page, viewport + '-defer-and-fresh-read');

      // Achievement isolation: S008 already completed several real chapters.
      // F12 must use its own reader, so it still proves zero-before/one-after.
      const badgeContext = await browser.newContext({ viewport: { width, height } });
      try {
        const badgePage = await badgeContext.newPage();
        const badgeEmail = 'f12.visual.' + viewport + '@example.test';
        await badgePage.goto(base + '/register');
        await badgePage.getByLabel('Отображаемое имя').fill('Читатель F12 ' + viewport);
        await badgePage.getByLabel('Электронная почта').fill(badgeEmail);
        await badgePage.getByLabel('Пароль (12–128 символов)').fill('F12BrowserSafePassword2026');
        await badgePage.getByRole('button', { name: 'Создать учётную запись' }).click();
        await badgePage.waitForURL('**/me');
        const badgeUid = positive(pg("SELECT id FROM users WHERE email_normalized='" + badgeEmail + "'"));

      // F-12/S046: real Chromium -> C verified completion -> private badge.
      // An empty personal dashboard does not pretend that reading occurred.
      const beforeBadge = await badgePage.goto(base + '/me', { waitUntil: 'networkidle' });
      assert(beforeBadge && beforeBadge.status() === 200,
        viewport + ': private achievement dashboard unavailable');
      assert(await badgePage.getByRole('heading', { name: 'Достижения' }).isVisible(),
        viewport + ': achievement section missing');
      assert(await badgePage.getByText('Пока не получено: 0/1 глав').isVisible(),
        viewport + ': opening the dashboard must not grant a badge');

      const chapter = await badgePage.goto(base + '/read/101', { waitUntil: 'networkidle' });
      assert(chapter && chapter.status() === 200,
        viewport + ': trusted free chapter unavailable');
      assert(await badgePage.getByRole('button', { name: 'Глава прочитана' }).isVisible(),
        viewport + ': server-validated chapter completion button missing');
      await badgePage.getByRole('button', { name: 'Глава прочитана' }).click();
      await badgePage.waitForURL('**/read/101#continuation');
      const rankings = badgePage.getByRole('link', { name: 'Мои рейтинги' });
      assert(await rankings.isVisible(),
        viewport + ': completed chapter must offer a voluntary rankings link');
      await rankings.click();
      await badgePage.waitForURL('**/me/rankings');

      const withBadge = await badgePage.goto(base + '/me', { waitUntil: 'networkidle' });
      assert(withBadge && withBadge.status() === 200,
        viewport + ': profile unavailable after completion');
      assert(await badgePage.getByText('Получено: 1/1 глав').isVisible(),
        viewport + ': verified chapter did not award the first badge');
      assert(await badgePage.getByText('Дата получения:', { exact: false }).count() === 1,
        viewport + ': first achievement timestamp missing or duplicated');
      assert(pg('SELECT count(*) FROM user_achievement_grants WHERE user_id=' + badgeUid) === '1',
        viewport + ': duplicate or absent immutable achievement grant');
      await screenshot(badgePage, viewport + '-achievement');

      await badgePage.goto(base + '/read/101');
      assert(await badgePage.getByText('Глава уже отмечена прочитанной.').isVisible(),
        viewport + ': completed chapter is still offered for completion');
      await badgePage.goto(base + '/me');
      assert(await badgePage.getByText('Получено: 1/1 глав').isVisible(),
        viewport + ': badge lost after returning to the profile');

      // A different browser identity must not receive these personal facts.
      const guestContext = await browser.newContext({ viewport: { width, height } });
      try {
        const guest = await guestContext.newPage();
        await guest.goto(base + '/me', { waitUntil: 'networkidle' });
        assert(new URL(guest.url()).pathname === '/login',
          viewport + ': guest could see private achievements');
        assert(!(await guest.content()).includes('Получено: 1/1 глав'),
          viewport + ': private badge leaked to a guest');
      } finally {
        await guestContext.close();
      }

      } finally {
        await badgeContext.close();
      }

      await context.close();
      console.log('F05 Chromium ' + viewport + ' PASS: session, skip, resume, tie and real PG');
    }
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
