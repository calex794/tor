'use strict';
// Actual server, native no-JS reports, real C progress; synthetic historical placement only.
const {chromium} = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {Fixture, sql, scalar, owner, base, shots} = require('./catalog_interest_pg_fixture.cjs');
fs.mkdirSync(shots, {recursive: true});
async function privateResponse(response, expected = 200) {
  assert.equal(response.status(), expected);
  const headers = await response.allHeaders();
  assert.equal(headers['cache-control'], 'no-store');
  assert.match(headers['x-robots-tag'], /noindex/);
  assert.match(headers['x-robots-tag'], /nofollow/);
}
async function register(page, email) {
  await page.goto(base + '/register');
  await page.getByLabel('Отображаемое имя').fill('Проверка каталога');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('CatalogueBrowserPassword2026');
  await page.getByRole('button', {name: 'Создать учётную запись', exact: true}).click();
  await page.waitForURL('**/me');
  return scalar("SELECT id FROM users WHERE email_normalized='" + email + "'");
}
async function complete(context, page, pair) {
  assert.equal((await page.goto(base + '/reader/' + pair.chapter)).status(), 200);
  const csrf = (await context.cookies()).find(cookie => cookie.name === 'tor-dev-csrf')?.value;
  assert.ok(csrf);
  async function save(offset, revision, expected = 200) {
    const response = await context.request.put(base + '/api/v1/me/progress/free', {
      headers: {'Content-Type': 'application/json', 'X-CSRF-Token': csrf, Origin: base},
      data: {chapter_id: String(pair.chapter), version_id: String(pair.version),
        block_id: 1, char_offset: offset, expected_revision: revision},
    });
    assert.equal(response.status(), expected, 'production C progress response: ' + await response.text());
    return expected === 200 ? (await response.json()).revision : null;
  }
  // The CAS revision belongs to the account/book, including a different chapter.
  const state = await context.request.get(base + '/api/v1/me/progress/free?chapter_id=' + pair.chapter);
  assert.equal(state.status(), 200);
  const currentRevision = Number((await state.json()).revision);
  assert.ok(Number.isSafeInteger(currentRevision) && currentRevision >= 0);
  const started = await save(3, currentRevision);
  const completed = await save(13, started);
  await save(13, completed, 409);
  assert.equal(scalar('SELECT count(*) FROM reader_chapter_starts WHERE chapter_id=' + pair.chapter), 1);
  assert.equal(scalar('SELECT count(*) FROM reader_chapter_completions WHERE chapter_id=' + pair.chapter), 1);
  return currentRevision;
}
async function metric(locator, name) {
  return (await locator.locator('[data-metric="' + name + '"]').first().innerText()).trim();
}
async function ids(page, testid) {
  return page.getByTestId(testid).evaluateAll(rows => rows.map(row => Number(row.dataset.bookId)));
}
function bookRow(page, id, testid = 'exposure-row') {
  return page.locator('[data-testid="' + testid + '"][data-book-id="' + id + '"]');
}
async function readingState(page, id) {
  return bookRow(page, id).getByTestId('exposure-reading').getAttribute('data-reading-state');
}
async function submit(page, action, predicate) {
  await page.locator('form[action="' + action + '"]').getByRole('button', {name: 'Показать', exact: true}).click();
  await page.waitForURL(predicate);
}
async function snapshot(page, name, target) {
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
    'no horizontal overflow');
  if (target) await target.scrollIntoViewIfNeeded();
  const bytes = target ? await target.screenshot({path: path.join(shots, name), animations: 'disabled'}) :
    await page.screenshot({path: path.join(shots, name), animations: 'disabled'});
  if (process.env.TOR_BROWSER_PROOF_IMAGES === '1') {
    console.log('FLOW_VISUAL_BEGIN ' + JSON.stringify({path: name, mime: 'image/png', bytes: bytes.length}));
    const encoded = bytes.toString('base64');
    for (let offset = 0; offset < encoded.length; offset += 3000) console.log('FLOW_VISUAL_DATA ' + encoded.slice(offset, offset + 3000));
    console.log('FLOW_VISUAL_END');
  }
}
(async () => {
  let browser, f;
  const evidence = {javaScriptEnabled: false, viewports: [390, 1440], exposureSource: 'private synthetic F history', readingSource: 'actual C HTTP commands'};
  try {
    f = new Fixture();
    f.seed();
    browser = await chromium.launch({headless: true});
    const context = await browser.newContext({javaScriptEnabled: false, viewport: {width: 1440, height: 900}});
    const readerContext = await browser.newContext({javaScriptEnabled: false, viewport: {width: 390, height: 844}});
    const page = await context.newPage();
    const readerPage = await readerContext.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    const ranked = base + '/admin/audience/reading?days=7&tag=' + encodeURIComponent(f.tag) + '&view=top20';
    const all = base + '/admin/audience/reading?days=7&tag=' + encodeURIComponent(f.tag);
    const calendar = base + '/admin/audience/exposure?days=1&group=tag&label=' + encodeURIComponent(f.tag);
    const closed = calendar + '&window=closed';
    await privateResponse(await page.goto(ranked), 401);
    await privateResponse(await page.goto(closed), 401);
    f.actors.push(await register(page, f.emails[0]));
    f.actors.push(await register(readerPage, f.emails[1]));
    await privateResponse(await page.goto(ranked), 403);
    await privateResponse(await readerPage.goto(closed), 403);
    sql("INSERT INTO account_roles(user_id,role) VALUES(" + f.actors[0] + ",'administrator')");

    const old = f.chapter(0, 1);
    const newer = f.chapter(0, 2);
    for (const pair of [old, newer, f.chapter(51, 1), f.chapter(52, 1)]) {
      const previousRevision = await complete(readerContext, readerPage, pair);
      if (pair.chapter === newer.chapter) assert.ok(previousRevision > 0, 'same-book revision must survive a chapter change');
      f.placeVerifiedFact(pair.chapter, pair.chapter === old.chapter ? 200 * 86400 : 7200);
    }
    assert.equal(scalar('SELECT count(*) FROM reader_chapter_completions WHERE book_id=' + f.books[0]), 2);
    const events = sql('SELECT source_event_id FROM reader_chapter_completions WHERE book_id=ANY(ARRAY[' + f.books + '])').split('\n');
    const before = f.fingerprint();
    await privateResponse(await page.goto(all));
    assert.deepEqual(await ids(page, 'canonical-row'), f.books.slice(0, 20));
    assert.equal(await metric(page.getByTestId('canonical-totals'), 'current'), '2');
    evidence.oldPairCurrent = Number(await metric(bookRow(page, f.books[0], 'canonical-row'), 'current'));
    assert.equal(evidence.oldPairCurrent, 0,
      'a later chapter must not become a second canonical first account/book pair');
    await page.getByTestId('canonical-next').click();
    await page.waitForURL(url => url.searchParams.get('after_id') === String(f.books[19]));
    assert.deepEqual(await ids(page, 'canonical-row'), f.books.slice(20, 40));
    assert.equal(await metric(page.getByTestId('canonical-totals'), 'books'), '53');
    await page.getByTestId('canonical-first').click();
    await page.waitForURL(url => !url.searchParams.has('after_id'));
    await page.getByLabel('Представление', {exact: true}).selectOption('top20');
    await submit(page, '/admin/audience/reading', url => url.searchParams.get('view') === 'top20');
    assert.deepEqual((await ids(page, 'canonical-rank-row')).slice(0, 2), f.books.slice(51, 53));
    assert.equal(await page.getByTestId('canonical-rank-row').count(), 20);
    assert.equal(await metric(page.getByTestId('canonical-rank-totals'), 'books'), '53');
    assert.equal(await metric(page.getByTestId('canonical-rank-totals'), 'current'), '2');
    assert.equal(await page.getByTestId('canonical-next').count(), 0);
    assert.equal(await page.getByTestId('canonical-rank-small-current').count(), 20);
    assert.equal(await page.locator('img[src="x"]').count(), 0);
    evidence.ranking = {
      books: Number(await metric(page.getByTestId('canonical-rank-totals'), 'books')),
      currentPairs: Number(await metric(page.getByTestId('canonical-rank-totals'), 'current')),
      rows: await page.getByTestId('canonical-rank-row').count(),
      firstBooks: (await ids(page, 'canonical-rank-row')).slice(0, 2),
    };
    for (const width of [390, 1440]) {
      await page.setViewportSize({width, height: width === 390 ? 844 : 900});
      await privateResponse(await page.goto(ranked));
      await snapshot(page, 'catalog-ranking-form-' + width + '.png');
      await snapshot(page, 'catalog-ranking-row-' + width + '.png', page.getByTestId('canonical-rank-row').first());
    }
    for (const days of [1, 30, 90]) {
      await page.getByLabel('Дней в периоде', {exact: true}).selectOption(String(days));
      await submit(page, '/admin/audience/reading', url => url.searchParams.get('days') === String(days));
      assert.equal(await metric(page.getByTestId('canonical-rank-totals'), 'current'), '2');
    }
    for (const kind of ['genres', 'tags']) {
      await page.getByLabel('Группировать по', {exact: true}).selectOption(kind);
      await submit(page, '/admin/audience/reading', url => url.searchParams.get('kind') === kind);
      assert.equal(await metric(page.getByTestId('canonical-rank-totals'), 'books'), '53');
      assert.equal(await metric(page.getByTestId('canonical-rank-totals'), 'current'), '2');
      assert.equal(await page.getByTestId('canonical-rank-row').count(), kind === 'genres' ? 4 : 1);
    }

    await privateResponse(await page.goto(calendar));
    assert.equal(await page.getByTestId('exposure-reading').count(), 0, 'calendar mode does not mix windows');
    await page.getByLabel('Граница периода', {exact: true}).selectOption('closed');
    await submit(page, '/admin/audience/exposure', url => url.searchParams.get('window') === 'closed');
    assert.equal(await readingState(page, f.books[1]), 'unknown', 'no durable health means unknown');
    f.certify();
    await privateResponse(await page.reload());
    assert.equal(await page.getByTestId('exposure-health-unknown').count(), 1, 'previous window intentionally incomplete');
    assert.equal(await page.getByTestId('exposure-health-complete').count(), 0);
    assert.equal(await readingState(page, f.books[1]), 'observed-without-verified-reading');
    assert.equal(await readingState(page, f.books[0]), 'reading-recorded');
    assert.equal(await metric(bookRow(page, f.books[0]), 'chapter-starts'), '1');
    assert.equal(await metric(bookRow(page, f.books[0]), 'chapter-completions'), '1');
    assert.equal(await readingState(page, f.books[3]), 'unknown', 'DOM19 cannot certify an observed base');
    assert.equal(await metric(page.getByTestId('exposure-totals'), 'books'), '53');
    assert.deepEqual(await ids(page, 'exposure-row'), f.books.slice(0, 50));
    evidence.currentReading = {
      starts: Number(await metric(bookRow(page, f.books[0]), 'chapter-starts')),
      chapterCompletions: Number(await metric(bookRow(page, f.books[0]), 'chapter-completions')),
      oldReaderState: await readingState(page, f.books[0]),
      retainedZeroState: await readingState(page, f.books[1]),
      smallBaseState: await readingState(page, f.books[3]),
      currentConfirmedHours: Number(await page.locator('[data-health-period="current"] [data-metric="confirmed-hours"]').innerText()),
      previousConfirmedHours: Number(await page.locator('[data-health-period="previous"] [data-metric="confirmed-hours"]').innerText()),
    };
    const html = await page.content();
    for (const secret of [...events, f.emails[1], 'Audience text']) assert.ok(!html.includes(secret));
    assert.equal(await page.locator('[data-flow-proof]').count(), 0, 'read-only history never turns collection on');
    for (const width of [390, 1440]) {
      await page.setViewportSize({width, height: width === 390 ? 844 : 900});
      await privateResponse(await page.goto(closed));
      await snapshot(page, 'catalog-reading-badge-' + width + '.png', bookRow(page, f.books[1]));
      await snapshot(page, 'catalog-reading-health-' + width + '.png', page.getByTestId('exposure-health'));
    }
    await page.getByTestId('exposure-next').click();
    await page.waitForURL(url => url.searchParams.get('after') === String(f.books[49]));
    assert.deepEqual(await ids(page, 'exposure-row'), f.books.slice(50));
    assert.equal(await metric(page.getByTestId('exposure-totals'), 'books'), '53');
    assert.equal(await readingState(page, f.books[51]), 'reading-recorded');
    assert.equal(await readingState(page, f.books[52]), 'reading-recorded');
    await page.getByLabel('Наблюдения', {exact: true}).selectOption('observed');
    await submit(page, '/admin/audience/exposure', url => url.searchParams.get('class') === 'observed' && !url.searchParams.has('after'));
    assert.ok(!(await ids(page, 'exposure-row')).includes(f.books[3]));
    assert.equal(await metric(page.getByTestId('exposure-totals'), 'books'), '53',
      'page diagnostics do not redefine complete F totals');
    assert.equal(f.fingerprint(), before, 'reading reports must not mutate source C/F facts');
    evidence.sourceFactsUnchangedByReports = true;

    // An owned canonical row without its matching C receipt is unknown, not zero.
    const missingReceipt = sql('SELECT source_event_id FROM reader_chapter_completions WHERE chapter_id=' + newer.chapter);
    assert.match(missingReceipt, /^[0-9a-f-]{36}$/);
    sql("BEGIN;DELETE FROM event_deliveries WHERE event_id='" + missingReceipt + "';" +
      "DELETE FROM outbox_events WHERE id='" + missingReceipt + "';COMMIT", owner);
    await privateResponse(await page.goto(closed));
    assert.equal(await readingState(page, f.books[0]), 'unknown');
    evidence.missingReceiptState = await readingState(page, f.books[0]);
    assert.equal(await metric(bookRow(page, f.books[0]), 'chapter-completions'), 'неизвестно');
    assert.equal(await readingState(page, f.books[1]), 'observed-without-verified-reading');
    sql('INSERT INTO flow_observation_health_hourly(epoch,bucket_ms,lost) VALUES(' +
      f.epoch + ',' + (f.anchor - 3600) * 1000 + ',true)', owner);
    await privateResponse(await page.goto(closed));
    assert.equal(await readingState(page, f.books[1]), 'unknown', 'known collector loss excludes a badge');
    sql('DELETE FROM flow_observation_health_hourly WHERE epoch=' + f.epoch, owner);
    sql('UPDATE books SET admin_hold=true WHERE id=' + f.books[1]);
    await privateResponse(await page.reload());
    assert.equal(await bookRow(page, f.books[1]).count(), 0);
    assert.equal(await metric(page.getByTestId('exposure-totals'), 'books'), '52');
    sql('UPDATE books SET admin_hold=true WHERE id=' + f.books[51]);
    await privateResponse(await page.goto(ranked));
    assert.equal(await bookRow(page, f.books[51], 'canonical-rank-row').count(), 0);
    assert.equal(await metric(page.getByTestId('canonical-rank-totals'), 'books'), '51');
    assert.equal(await metric(page.getByTestId('canonical-rank-totals'), 'current'), '1');
    sql('DELETE FROM account_roles WHERE user_id=' + f.actors[0]);
    await privateResponse(await page.goto(ranked), 403);
    await privateResponse(await page.goto(closed), 403);
    sql('UPDATE sessions SET revoked_at=now() WHERE user_id=' + f.actors[0]);
    await privateResponse(await page.goto(ranked), 401);
    await privateResponse(await page.goto(closed), 401);
    f.assertBoundary();
    assert.deepEqual(errors, []);
    console.log('CATALOG_INTEREST_BODY_RECEIPT ' + JSON.stringify(evidence));
    console.log('CATALOG_INTEREST_BROWSER_PASS real C writes/replay; old-reader new-chapter included; full top20; ID keysets; native no-JS filters; independent closed-window badges; missing C receipt/unknown/DOM19/loss; read-only/live ACL; 390/1440');
  } finally {
    if (browser) await browser.close();
    if (f) f.cleanup();
  }
})().catch(error => {console.error(error); process.exitCode = 1;});
