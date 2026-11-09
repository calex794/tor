'use strict';
// BR-060: actual Chromium + Rust HTTP + disposable PostgreSQL, no route mocks.
// Owner submits one password-confirmed deletion request; an independent site
// moderator approves through real HTTP before the reader restores pinned V1.
const assert = require('node:assert/strict');
const { chromium } = require('playwright');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs/promises');
const path = require('node:path');
const { approvePendingBookForIsolatedCi } = require('./site_moderation_ci.cjs');

const base = 'http://127.0.0.1:3098';
const password = 'Br060BrowserArchive2026Pass';
const nonce = (process.env.GITHUB_RUN_ID || 'ci') + '-' + Date.now();
const address = (role) => 'br060.' + role + '.' + nonce + '@example.invalid';
const db = process.env.APP_URL;

// All SQL below mutates disposable fixtures, never production or migrations.
if (process.env.GITHUB_ACTIONS !== 'true' || !db) {
  throw new Error('BR-060 browser fixture requires isolated GitHub CI');
}
const dbUrl = new URL(db);
if (!['postgres:', 'postgresql:'].includes(dbUrl.protocol) ||
    dbUrl.hostname !== '127.0.0.1' || dbUrl.port !== '5432' ||
    dbUrl.pathname !== '/tor_ci' || dbUrl.username !== 'tor_app' ||
    dbUrl.search || dbUrl.hash) {
  throw new Error('Refusing non-disposable database');
}

function pg(command) {
  return execFileSync('psql', [
    '-X', '-A', '-t', '-q', '-v', 'ON_ERROR_STOP=1',
    '--dbname', db, '--command', command,
  ], { encoding: 'utf8', timeout: 15000, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

function id(value) {
  const raw = String(value);
  assert.match(raw, /^[1-9]\d{0,18}$/, 'invalid ephemeral database identifier');
  return raw;
}

async function register(page, email, name) {
  await page.goto(base + '/register');
  await page.getByLabel('Отображаемое имя').fill(name);
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill(password);
  await Promise.all([
    page.waitForURL(base + '/me'),
    page.getByRole('button', { name: 'Создать учётную запись' }).click(),
  ]);
}

async function login(page, email) {
  await page.goto(base + '/login');
  await page.locator('[name=email]').fill(email);
  await page.locator('[name=password]').fill(password);
  await Promise.all([
    page.waitForURL(base + '/me'),
    page.locator('form[action="/login"] button').click(),
  ]);
}

async function createPublishedBook(owner) {
  await owner.goto(base + '/me/books');
  await owner.locator('#main').getByRole('link', { name: 'Создать книгу' }).click();
  await owner.waitForURL(base + '/me/books/new');
  await owner.getByLabel('Название книги').fill('Архивная книга BR-060 ' + nonce);
  await owner.getByLabel('Язык', { exact: true }).fill('ru');
  await owner.getByRole('button', { name: 'Создать книгу' }).click();
  await owner.waitForURL(/\/me\/books\/\d+$/);
  const book = id(owner.url().split('/').pop());
  await owner.getByRole('link', { name: 'Добавить главу' }).click();
  await owner.waitForURL('**/chapters/new');
  await owner.getByLabel('Название главы').fill('Старая редакция');
  // Different UTF-16 lengths, paragraph boundaries and enough real scroll
  // distance to detect accidental top-of-page resume on mobile.
  const chapterBody = Array.from({ length: 24 }, (_, i) =>
    'Абзац ' + (i + 1) + ': Архив V1 😀 и символ &. ' +
    ('Сохраняем точную позицию чтения. ').repeat(24)
  ).join('\n\n');
  await owner.getByLabel('Текст главы').fill(chapterBody);
  await owner.getByRole('button', { name: 'Сохранить главу' }).click();
  await owner.waitForURL(base + '/me/books/' + book);
  await owner.getByRole('button', { name: 'Опубликовать книгу' }).click();
  await owner.waitForURL(base + '/books/' + book);
  const invisible = await owner.goto(base + '/books/' + book);
  assert.equal(invisible.status(), 404, 'Book moderation may not be bypassed');
  approvePendingBookForIsolatedCi(book);
  assert.equal((await owner.goto(base + '/books/' + book)).status(), 200);
  const chapter = id(pg('SELECT id FROM chapters WHERE book_id=' + book +
    ' AND chapter_no=1 AND visibility=\'public\''));
  return { book, chapter };
}

async function finalizeOrdinaryDelete(owner, reviewer, reader, book) {
  const url = base + '/me/books/' + book + '/settings/delete';
  const request = await owner.goto(url);
  assert.equal(request.status(), 200);
  assert.equal(await owner.locator('[name=action]').inputValue(), 'start');
  await owner.locator('[name=account_password]').fill(password);
  const [submitted] = await Promise.all([
    owner.waitForResponse(response =>
      response.url() === url && response.request().method() === 'POST'),
    owner.getByRole('button', { name: 'Начать удаление' }).click(),
  ]);
  assert.equal(submitted.status(), 303, 'owner must confirm a valid deletion request');
  assert.equal(pg('SELECT lifecycle_status FROM books WHERE id=' + book), 'pending_deletion');
  assert.equal(pg('SELECT COUNT(*) FROM book_ordinary_deletion_decisions WHERE book_id=' + book), '0');
  const pending = await owner.goto(url);
  assert.equal(pending.status(), 200);
  assert(await owner.getByText(/Заявка направлена на модерацию/).isVisible());
  assert.equal(await owner.locator('[name=action]').count(), 0,
    'owner must have no unreviewed finalization path');
  assert.equal((await reader.goto(base + '/moderation/books/deletions')).status(), 403,
    'ordinary reader has no moderator queue permission');

  const pendingReview = pg(
    'SELECT COUNT(*) FROM book_ordinary_deletion_requests r JOIN books b ' +
    'ON b.id=r.book_id AND b.lifecycle_revision=r.request_revision ' +
    'WHERE r.book_id=' + book + " AND b.lifecycle_status='pending_deletion'"
  );
  assert.equal(pendingReview, '1', 'owner request not committed at current Book revision');
  const queue = await reviewer.goto(base + '/moderation/books/deletions');
  assert.equal(queue.status(), 200, 'independent site moderator cannot view deletion queue');
  assert.equal(reviewer.url(), base + '/moderation/books/deletions',
    'unexpected navigation while opening moderator queue');
  const form = reviewer.locator('form[action="/moderation/books/' + book + '/deletion"]');
  const forms = await form.count();
  if (forms !== 1) {
    // Never block diagnostic output on a missing H1: capture the actual page
    // structure and independently prove the request exists in the disposable DB.
    const diagnosis = await reviewer.evaluate(() => ({
      title: document.title,
      headings: [...document.querySelectorAll('h1')].map(h => h.textContent?.trim()).slice(0, 3),
      formActions: [...document.forms].map(f => f.getAttribute('action')).slice(0, 8),
      emptyQueue: document.body?.textContent?.includes('Ожидающих решения заявок нет.'),
      pageLength: document.documentElement.outerHTML.length,
    }));
    const reviewerId = id(pg(
      "SELECT id FROM users WHERE email_normalized='" + address('reviewer') + "'"
    ));
    const eligible = pg(
      'SELECT COUNT(*) FROM book_ordinary_deletion_requests r ' +
      'JOIN books b ON b.id=r.book_id WHERE r.book_id=' + book +
      " AND b.lifecycle_status='pending_deletion'" +
      ' AND b.lifecycle_revision=r.request_revision AND NOT b.is_deleted' +
      " AND r.owner_user_id<>" + reviewerId +
      ' AND NOT EXISTS (SELECT 1 FROM book_ordinary_deletion_decisions d ' +
      ' WHERE d.book_id=r.book_id AND d.request_revision=r.request_revision)'
    );
    assert.equal(forms, 1,
      'pending request missing in moderator UI: ' +
      JSON.stringify({ diagnosis, queueStatus: queue.status(), pendingReview, eligible }));
  }
  const revision = await form.locator('[name=request_revision]').inputValue();
  assert.match(revision, /^[1-9]\d*$/);
  // A first click only displays consequences; no persistent state is changed.
  const decisionUrl = base + '/moderation/books/' + book + '/deletion';
  const [preview] = await Promise.all([
    reviewer.waitForResponse(response =>
      response.url() === decisionUrl && response.request().method() === 'POST'),
    form.locator('button[name=decision][value=approve]').click(),
  ]);
  assert.equal(preview.status(), 200, 'deletion approval confirmation was not displayed');
  assert.equal(pg('SELECT lifecycle_status FROM books WHERE id=' + book), 'pending_deletion',
    'the confirmation preview cannot finalize deletion');
  assert.equal(pg('SELECT COUNT(*) FROM book_ordinary_deletion_decisions WHERE book_id=' + book), '0',
    'the preview cannot append an approval decision');
  assert.equal(await reviewer.getByRole('heading', {
    name: /Подтверждение решения по удалению книги/,
  }).count(), 1, 'confirmation screen must identify deletion action');
  const [decision] = await Promise.all([
    reviewer.waitForResponse(response =>
      response.url() === decisionUrl && response.request().method() === 'POST'),
    reviewer.getByRole('button', { name: 'Подтвердить удаление' }).click(),
  ]);
  assert.equal(decision.status(), 303, 'independent moderation approval refused');
  assert.equal(pg('SELECT lifecycle_status FROM books WHERE id=' + book), 'ordinary_deleted');
  assert.equal(pg('SELECT COUNT(*) FROM book_ordinary_deletion_decisions WHERE book_id=' +
    book + " AND request_revision=" + revision + " AND decision='approve'"), '1');
  assert.equal(pg('SELECT COUNT(*) FROM book_ordinary_deletion_events WHERE book_id=' +
    book), '2', 'owner request and moderator finalization must both be audited');
  const closed = await owner.goto(url);
  assert.equal(closed.status(), 200);
  assert(await owner.getByText('Книга удалена из публичного доступа.').isVisible());
}

async function checkViewport(page, screen) {
  const result = await page.evaluate(() => ({
    width: document.documentElement.scrollWidth,
    viewport: document.documentElement.clientWidth,
  }));
  assert(result.width <= result.viewport + 1, screen + ' horizontal overflow');
  const root = process.env.TOR_READER_SHOTS;
  if (root) {
    await fs.mkdir(root, { recursive: true });
    await page.screenshot({
      path: path.join(root, 'br060-archived-v1-' + screen + '.png'),
      fullPage: true,
    });
  }
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  const ownerContext = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const readerContext = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const outsiderContext = await browser.newContext({ viewport: { width: 390, height: 844 } });
  const reviewerContext = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const readerMail = address('reader');
  let freshContext;
  try {
    const owner = await ownerContext.newPage();
    const reader = await readerContext.newPage();
    await register(owner, address('owner'), 'Владелец BR-060');
    const { book, chapter } = await createPublishedBook(owner);
    await register(reader, readerMail, 'Читатель BR-060');
    const reviewer = await reviewerContext.newPage();
    const reviewerMail = address('reviewer');
    await register(reviewer, reviewerMail, 'Сайт-модератор BR-060');
    const reviewerId = id(pg("SELECT id FROM users WHERE email_normalized='" + reviewerMail + "'"));
    pg("INSERT INTO account_roles(user_id,role,is_active) VALUES(" +
      reviewerId + ",'moderator',TRUE)");

    // Actual browser bookmark action pins the published free V1.
    await reader.goto(base + '/me/library/add/' + book);
    await Promise.all([
      reader.waitForURL(base + '/me/library'),
      reader.getByRole('button', { name: 'Сохранить книгу' }).click(),
    ]);
    const pin = pg(
      'SELECT v.version_no FROM reader_saved_free_chapters p ' +
      'JOIN chapter_versions v ON v.id=p.version_id ' +
      'JOIN users u ON u.id=p.user_id ' +
      'WHERE u.email_normalized=\'' + readerMail + '\' AND p.chapter_id=' + chapter
    );
    assert.equal(pin, '1', 'bookmark must pin exact V1 before author replaces it');
    // A 160-chapter archived book exercises both real keyset directions.
    // These are disposable fixture pins created BEFORE the moderated deletion.
    pg('INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level) ' +
      'SELECT ' + book + ',n,\'Archive extra \'||n,\'public\',\'free\' ' +
      'FROM generate_series(2,160) AS n');
    pg('INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) ' +
      'SELECT id,1,\'ARCHIVE_HISTORY_V1_\'||chapter_no ' +
      'FROM chapters WHERE book_id=' + book + ' AND chapter_no BETWEEN 2 AND 160');
    pg('INSERT INTO chapter_publications(chapter_id,version_id) ' +
      'SELECT c.id,v.id FROM chapters c ' +
      'JOIN chapter_versions v ON v.chapter_id=c.id ' +
      'WHERE c.book_id=' + book + ' AND c.chapter_no BETWEEN 2 AND 160');
    pg('INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id) ' +
      'SELECT li.user_id,c.id,p.version_id FROM reader_library_items li ' +
      'JOIN chapters c ON c.book_id=li.book_id ' +
      'JOIN chapter_publications p ON p.chapter_id=c.id ' +
      'WHERE li.book_id=' + book + ' AND c.chapter_no BETWEEN 2 AND 160 ' +
      'AND li.user_id=(SELECT id FROM users WHERE email_normalized=\'' +
        readerMail + '\') ON CONFLICT DO NOTHING');


    // Simulate the author's later publication without modifying the old pin.
    const v2 = id(pg(
      'INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) ' +
      'VALUES(' + chapter + ',2,\'BR060_UNPINNED_FREE_V2\') RETURNING id'
    ));
    assert.equal(pg('UPDATE chapter_publications SET version_id=' + v2 +
      ' WHERE chapter_id=' + chapter + ' RETURNING version_id'), v2);
    await finalizeOrdinaryDelete(owner, reviewer, reader, book);
    assert.equal(pg('SELECT lifecycle_status FROM books WHERE id=' + book),
      'ordinary_deleted');
    assert.equal((await reader.goto(base + '/books/' + book)).status(), 404);
    assert.equal((await reader.goto(base + '/read/' + chapter)).status(), 404);

    const archivedURL = base + '/me/library/saved/' + chapter;
    const original = await reader.goto(archivedURL, { waitUntil: 'domcontentloaded' });
    assert.equal(original.status(), 200, 'private free V1 vanished after removal');
    assert(await reader.locator('[data-free-progress-archive="1"]').count() === 1);
    assert((await reader.content()).includes('Архив V1 😀'));
    assert(!(await reader.content()).includes('BR060_UNPINNED_FREE_V2'));
    await reader.waitForFunction(() =>
      document.querySelector('[data-free-progress-chapter]')?.dataset.freeProgressReady === '1'
    );
    await checkViewport(reader, 'desktop-before-save');
    const archiveToc = reader.locator('[data-archive-toc]');
    assert.equal(await archiveToc.count(), 1, 'private archive TOC missing');
    assert.equal(await archiveToc.locator('.archive-toc__row--current').count(), 1);
    assert.equal(await archiveToc.locator('a[aria-current="page"]').count(), 1);
    assert.equal(await archiveToc.locator('a[href*="purchase_id="]').count(), 0,
      'no direct purchase was made, so archive cannot invent a paid link');
    await archiveToc.locator('summary').click();
    assert(await archiveToc.evaluate(el => el.open), 'archive TOC did not open');
    const desktopModal = await archiveToc.locator('[data-archive-toc-modal]')
      .evaluate(el => {
        const rect = el.getBoundingClientRect();
        return { left: rect.left, right: rect.right, top: rect.top,
          bottom: rect.bottom, width: innerWidth, height: innerHeight,
          columns: getComputedStyle(el.querySelector('.archive-toc__list'))
            .gridTemplateColumns.split(' ').length };
      });
    assert(desktopModal.left >= 0 && desktopModal.top >= 0 &&
      desktopModal.right <= desktopModal.width + 1 &&
      desktopModal.bottom <= desktopModal.height + 1 &&
      desktopModal.columns === 3, 'desktop archive modal must be viewport-fixed and 3-column');
    await archiveToc.locator('[data-archive-toc-close]').click();
    assert(!(await archiveToc.evaluate(el => el.open)), 'archive TOC close failed');
    assert(await archiveToc.locator('summary')
      .evaluate(el => el === document.activeElement), 'archive focus was not restored');
    await archiveToc.locator('summary').click();
    await reader.keyboard.press('Escape');
    assert(!(await archiveToc.evaluate(el => el.open)), 'Escape did not close archive TOC');

    const savedResponse = reader.waitForResponse(response =>
      response.url() === base + '/api/v1/me/progress/archived-free' &&
      response.request().method() === 'PUT', { timeout: 15000 });
    await reader.evaluate(() => window.scrollTo(
      0, Math.round(document.documentElement.scrollHeight * 0.66)
    ));
    const receipt = await savedResponse;
    assert.equal(receipt.status(), 200, 'real Chromium scroll save failed');
    const progress = await receipt.json();
    assert.equal(progress.chapter_id, chapter);
    assert.equal(progress.version_id, pg('SELECT version_id FROM ' +
      'reader_saved_free_chapters WHERE chapter_id=' + chapter +
      ' AND user_id=(SELECT id FROM users WHERE email_normalized=\'' + readerMail + '\')'));
    assert(progress.block_id > 1 || progress.char_offset > 100,
      'scrolled viewport produced top-of-chapter progress');
    const stored = pg(
      'SELECT version_id::text || \',\' || block_index::text || \',\' || char_offset::text ' +
      'FROM reader_book_positions WHERE book_id=' + book +
      ' AND user_id=(SELECT id FROM users WHERE email_normalized=\'' + readerMail + '\')'
    ).split(',');
    assert.deepEqual(stored, [
      progress.version_id, String(progress.block_id), String(progress.char_offset),
    ], 'server cursor differs from the browser save');
    assert.equal(pg('SELECT COUNT(*) FROM reader_chapter_completions ' +
      'WHERE chapter_id=' + chapter), '0', 'scroll must not mark chapter read');

    // A different browser context, smaller screen, and a brand-new session.
    await readerContext.close();
    freshContext = await browser.newContext({ viewport: { width: 390, height: 844 } });
    const fresh = await freshContext.newPage();
    await login(fresh, readerMail);
    const library = await fresh.goto(base + '/me/library');
    assert.equal(library.status(), 200);
    assert((await fresh.content()).includes('Удалена'),
      'personal library lost the ordinary-deleted tombstone');
    const toc = await fresh.goto(base + '/me/library/' + book + '/saved');
    assert.equal(toc.status(), 200);
    assert(await fresh.locator('a[href="/me/library/saved/' + chapter + '"]').count() === 1);
    assert.equal((await fresh.goto(archivedURL, { waitUntil: 'networkidle' })).status(), 200);
    await fresh.waitForFunction(() =>
      document.querySelector('[data-free-progress-chapter]')?.dataset.freeProgressRestored === '1'
    );
    const scroll = await fresh.evaluate(() => window.scrollY);
    assert(scroll > 100, 'fresh mobile login did not restore saved text position');
    assert(await fresh.locator('#saved-resume').count() === 1);
    await checkViewport(fresh, 'mobile-after-login');
    const mobileToc = fresh.locator('[data-archive-toc]');
    await mobileToc.locator('summary').click();
    const mobileModal = await mobileToc.locator('[data-archive-toc-modal]')
      .evaluate(el => {
        const rect = el.getBoundingClientRect();
        return { left: rect.left, right: rect.right, bottom: rect.bottom,
          width: innerWidth, height: innerHeight,
          columns: getComputedStyle(el.querySelector('.archive-toc__list'))
            .gridTemplateColumns.split(' ').length };
      });
    assert(mobileModal.left >= 0 && mobileModal.right <= mobileModal.width + 1 &&
      Math.abs(mobileModal.bottom - mobileModal.height) <= 2 &&
      mobileModal.columns === 1, 'mobile archive TOC must be a 1-column bottom sheet');
    await mobileToc.locator('[data-archive-toc-close]').click();
    const positionAfterToc = await fresh.evaluate(() => window.scrollY);
    assert(Math.abs(positionAfterToc - scroll) < 12,
      'opening archive TOC must not replace the pinned reading scroll position');
    assert(!(await fresh.content()).includes('BR060_UNPINNED_FREE_V2'));
    const midChapter = id(pg('SELECT id FROM chapters WHERE book_id=' +
      book + ' AND chapter_no=80'));
    const midUrl = base + '/me/library/saved/' + midChapter;
    assert.equal((await fresh.goto(midUrl, { waitUntil: 'networkidle' })).status(), 200);
    const middleToc = fresh.locator('[data-archive-toc]');
    assert.equal(await middleToc.locator('.archive-toc__row').count(), 50,
      'initial archived window should be 25 older plus 25 newer editions');
    assert.match(await middleToc.getAttribute('data-archive-toc-before'), /^\d+\.\d+$/);
    assert.match(await middleToc.getAttribute('data-archive-toc-after'), /^\d+\.\d+$/);
    assert.equal(await middleToc.locator('.archive-toc__row--current').count(), 1);
    const currentArchiveTitle = await middleToc
      .locator('.archive-toc__row--current .archive-toc__title').textContent();
    assert(currentArchiveTitle?.includes('Глава 80.'),
      'archived current row mismatch: ' + JSON.stringify(currentArchiveTitle));
    const archiveApi = '/api/v1/me/library/saved/' + midChapter + '/toc/window';
    const archiveWindow = await fresh.evaluate(async url => {
      const reply = await fetch(url, { credentials: 'same-origin' });
      return { code: reply.status, cache: reply.headers.get('cache-control'),
        data: await reply.json() };
    }, archiveApi + '?before=80.' + midChapter + '&limit=60');
    assert.equal(archiveWindow.code, 200);
    assert.equal(archiveWindow.cache, 'no-store');
    assert.equal(archiveWindow.data.items.length, 60);
    assert(!JSON.stringify(archiveWindow.data).includes('ARCHIVE_HISTORY_V1_'),
      'private TOC must not deliver archived body text');
    const invalidArchive = await fresh.evaluate(async url =>
      (await fetch(url, { credentials: 'same-origin' })).status,
    archiveApi + '?before=080.' + midChapter);
    assert.equal(invalidArchive, 400, 'noncanonical cursor must be rejected');
    const wideArchive = await fresh.evaluate(async url =>
      (await fetch(url, { credentials: 'same-origin' })).status,
    archiveApi + '?before=80.' + midChapter + '&limit=61');
    assert.equal(wideArchive, 400, 'archive pages must be bounded at 60');
    await middleToc.locator('summary').click();
    const viewport = middleToc.locator('[data-archive-toc-scroll]');
    await viewport.evaluate(el => {
      el.scrollTop = 0;
      el.dispatchEvent(new Event('scroll'));
    });
    await fresh.waitForFunction(() =>
      document.querySelectorAll('.archive-toc__row').length > 50, null, { timeout: 15000 });
    const previousLoaded = await middleToc.locator('.archive-toc__row').count();
    assert(previousLoaded > 50, 'scrolling up did not load earlier saved editions');
    assert((await middleToc.locator('.archive-toc__title').allTextContents())
      .some(text => text.startsWith('Глава 1.')),
      'first archived chapter must be reachable without leaving the modal');
    await viewport.evaluate(el => {
      el.scrollTop = el.scrollHeight;
      el.dispatchEvent(new Event('scroll'));
    });
    await fresh.waitForFunction(count =>
      document.querySelectorAll('.archive-toc__row').length > count,
    previousLoaded, { timeout: 15000 });
    const headings = await middleToc.locator('.archive-toc__title').allTextContents();
    assert(headings.some(text => text.startsWith('Глава 160.')),
      'last archived chapter must be reachable without a separate archive page');
    assert.equal(await middleToc.locator('.archive-toc__row').count(), 160);
    const keys = await middleToc.locator('[data-archive-toc-id]')
      .evaluateAll(rows => rows.map(row => row.dataset.archiveTocId));
    assert.equal(new Set(keys).size, keys.length, 'pagination duplicated archived chapters');
    await middleToc.locator('[data-archive-toc-close]').click();
    console.log('BR-060 archive TOC PASS: 160 pinned chapters, private JSON, both edges, mobile');


    // Another signed-in user has neither a Book shelf item nor the old V1.
    const outsider = await outsiderContext.newPage();
    await register(outsider, address('outsider'), 'Посторонний BR-060');
    assert.equal((await outsider.goto(archivedURL)).status(), 404);
    const outsiderReply = await outsiderContext.request.get(
      base + '/api/v1/me/library/saved/' + chapter + '/toc/window?after=1.' + chapter,
    );
    const outsiderToc = outsiderReply.status();
    assert.equal(outsiderToc, 404, 'other signed-in account cannot enumerate archive');

    const outsidersLibrary = await outsider.goto(base + '/me/library');
    assert.equal(outsidersLibrary.status(), 200);
    assert(!(await outsider.content()).includes('Архивная книга BR-060 ' + nonce));
    assert.equal(pg('SELECT COUNT(*) FROM reader_chapter_completions ' +
      'WHERE chapter_id=' + chapter), '0');
    console.log('BR-060 PASS: owner request + independent moderator approval, Chromium archived V1 autosave, ' +
      'fresh mobile login restore, immutable pin, outsider denial and no completion');
  } finally {
    await ownerContext.close();
    await outsiderContext.close();
    await reviewerContext.close();
    if (freshContext) await freshContext.close();
    await browser.close();
  }
})().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
