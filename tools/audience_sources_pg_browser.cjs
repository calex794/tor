'use strict';
// Retained source totals are historical fixtures, separate from real ordinary C reading.
// Existing enabled scopes and old process flags must not resume optional capture.
const {chromium} = require('playwright');
const {randomBytes} = require('node:crypto');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const preview = require('./ci_browser_preview.cjs');

const db = require('./legacy_analytics_fixture.cjs')();
const base = process.env.TOR_AUDIENCE_BASE || 'http://127.0.0.1:3108';
const shots = process.env.TOR_AUDIENCE_SHOTS || '/tmp/audience-browser-shots';
const parsed = new URL(db);
if (parsed.protocol !== 'postgresql:' || parsed.hostname !== '127.0.0.1'
    || parsed.port !== '5432' || parsed.pathname !== '/tor_ci_legacy_analytics'
    || !/^http:\/\/127\.0\.0\.1:\d+$/.test(base)) {
  throw new Error('Source acceptance requires disposable loopback legacy analytics baseline 191');
}
fs.mkdirSync(shots, {recursive:true});
const sources = ['catalog', 'search', 'recommendations', 'unattributed'];
const metrics = ['clicks', 'book-opens', 'starts', 'completions'];

const {sql,scalar,csrf,privateResponse,chapter,read} = require('./audience_report_pg_helpers.cjs')({db,base});
async function register(page, email) {
  await page.goto(base+'/register');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('AudienceSourcesPassword2026');
  await page.getByRole('button', {name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
  return scalar(`SELECT id FROM users WHERE email_normalized='${email}'`);
}
const retired = require('./audience_retired_pg_helpers.cjs')({sql,scalar,base});
const navigate = retired.navigate;

async function numbers(page, period='current') {
  const section = page.getByTestId('audience-sources-'+period);
  assert.equal(await section.count(), 1);
  const result = {};
  for (const source of sources) {
    const row = section.locator(`[data-source="${source}"]`);
    assert.equal(await row.count(), 1);
    result[source] = {};
    for (const metric of metrics) {
      const value = (await row.locator(`dd[data-metric="${metric}"]`).innerText()).trim();
      assert.ok(/^\d+$/.test(value) || value === 'Нет данных' || value === 'Не измеряется', value);
      result[source][metric] = /^\d+$/.test(value) ? Number(value) : null;
    }
  }
  assert.equal(result.unattributed.clicks, null, 'unknown navigation cannot become zero measured clicks');
  assert.equal(result.unattributed['book-opens'], null);
  return result;
}
function assertDelta(before, after, changes, legacyRecOpens=false) {
  for (const source of sources) {
    for (const metric of metrics) {
      if (source === 'unattributed' && ['clicks','book-opens'].includes(metric)) continue;
      if (source === 'recommendations' && metric === 'book-opens' && legacyRecOpens) {
        assert.equal(after[source][metric], null, 'partial historical REC measurement remains unknown');
        continue;
      }
      const change = changes[source]?.[metric] || 0;
      const expected = (before[source][metric] ?? 0)+change;
      assert.equal(after[source][metric] ?? 0, expected, source+' '+metric);
    }
  }
}
async function calendarPeriods(page, days) {
  const bounds = {};
  for (const period of ['current','previous']) {
    bounds[period] = await page.getByTestId('audience-sources-'+period)
      .locator('time[datetime]').evaluateAll(nodes => nodes.map(node => node.getAttribute('datetime')));
    assert.equal(bounds[period].length, 2);
    bounds[period].forEach(value => assert.match(value, /^\d{4}-\d{2}-\d{2}$/));
    assert.equal(Date.parse(bounds[period][1])-Date.parse(bounds[period][0]), days*86400000);
  }
  assert.equal(bounds.previous[1], bounds.current[0]);
  assert.equal(bounds.current[1], sql("SELECT ((now() AT TIME ZONE 'UTC')::date+1)::text"));
}

(async () => {
  const run = Date.now()+'-'+randomBytes(4).toString('hex');
  const emails = ['admin','reader','direct'].map(kind => `flow-sources-${kind}-${run}@tor-ci.example`);
  const fixtures = [], legacyTokens = [], users = [];
  let browser;
  try {
    for (const key of [...sources,'mixed','after-retirement']) {
      const title = `FLOW-SOURCE-SENTINEL-${run}-${key}`;
      const book = scalar(`INSERT INTO books(source_identity,target_language,title,visibility,access_level)
        VALUES('ci:flow:sources:${run}:${key}','ru','${title}','public','basic') RETURNING id`);
      fixtures.push({key,book,title,...chapter(book,1)});
    }
    const fixture = key => fixtures.find(item => item.key === key);
    const second = chapter(fixture('mixed').book, 2);
    browser = await chromium.launch({headless:true});
    const pages = [];
    const errors = [];
    for (const [index,email] of emails.entries()) {
      const context = await browser.newContext({viewport:{width:index === 0 ? 1440 : 390,height:900}});
      const page = await context.newPage();
      page.on('pageerror', error => errors.push(error.message));
      users.push(await register(page,email));
      pages.push(page);
    }
    const [adminPage,readerPage,directPage] = pages;
    sql(`INSERT INTO account_roles(user_id,role) VALUES(${users[0]},'administrator')`);
    await readerPage.goto(base+'/catalog?sort=newest');
    assert.equal(await readerPage.locator('[data-organic-ticket]').count(), 0);
    await privateResponse(await readerPage.goto(base+'/odmen/audience'), 403);
    assert.equal(await readerPage.locator('[data-metric]').count(), 0);
    retired.oldScopes(users.slice(1));
    const scope = retired.scopeSnapshot(users);
    await retired.settings(readerPage);
    legacyTokens.push(await retired.rejectedLegacyCommands(readerPage));
    await retired.settings(directPage);
    legacyTokens.push(await retired.rejectedLegacyCommands(directPage));
    retired.legacyBoundary();
    await privateResponse(await adminPage.goto(base+'/odmen/audience?days=7'));
    const before = await numbers(adminPage);
    const previous = await numbers(adminPage,'previous');
    const legacyRecOpens = sql(`SELECT EXISTS(SELECT 1 FROM rec_quality_daily
      WHERE book_opens IS NULL AND bucket_date>=(now() AT TIME ZONE 'UTC')::date-6
        AND bucket_date<(now() AT TIME ZONE 'UTC')::date+1)`) === 't';

    retired.history([
      {book:fixture('catalog').book,source:'catalog',issued:2,clicks:1,opens:1,starts:1,completions:1},
      {book:fixture('search').book,source:'search',issued:2,clicks:1,opens:1,starts:1,completions:1},
      {book:fixture('recommendations').book,source:'recommendations',impressions:2,clicks:1,opens:1,starts:1,completions:1},
      {book:fixture('unattributed').book,source:'unattributed',starts:1,completions:1},
      {book:fixture('mixed').book,source:'catalog',issued:2,clicks:1,opens:1},
      {book:fixture('mixed').book,source:'search',issued:2,clicks:1,opens:1,starts:1,completions:1},
      {book:fixture('mixed').book,source:'recommendations',impressions:2,clicks:1,opens:1},
    ]);
    retired.noCapture(users,fixtures.map(item=>item.book));
    await privateResponse(await adminPage.goto(base+'/odmen/audience?days=7'));
    const historical = await numbers(adminPage);
    await retired.marked(adminPage,['audience-sources-current','audience-sources-previous']);

    for (const source of ['catalog','search','recommendations']) {
      await navigate(readerPage,fixture(source),source);
      await read(readerPage,fixture(source));
    }
    assert.equal((await directPage.goto(base+`/books/${fixture('unattributed').book}`)).status(), 200);
    await read(directPage,fixture('unattributed'));
    // All three ordinary routes remain usable; none binds the later C fact to a source.
    for (const source of ['catalog','recommendations','search']) {
      await navigate(readerPage,fixture('mixed'),source);
    }
    await read(readerPage,fixture('mixed'));
    await read(readerPage,second);
    // Let actual retention housekeeping run after canonical C commands.
    await readerPage.goto(base+'/me');
    await directPage.goto(base+'/me');
    retired.legacyBoundary();
    retired.noCapture(users,fixtures.map(item=>item.book));
    const mixed = fixture('mixed').book;
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_completions WHERE user_id=${users[1]} AND book_id=${mixed}`), 2,
      'both canonical C chapter completions remain distinct');
    assert.equal(scalar(`SELECT COALESCE(sum(book_opens),0) FROM rec_quality_daily
      WHERE book_id IN(${fixtures.map(item => item.book).join(',')})`),2,
      'retained fixture totals remain unchanged after genuine navigation');
    await privateResponse(await adminPage.goto(base+'/odmen/audience?days=7'));
    assert.deepEqual(await numbers(adminPage),historical,'new C facts cannot augment old source totals');
    const changes = {
      catalog:{clicks:2,'book-opens':2,starts:1,completions:1},
      search:{clicks:2,'book-opens':2,starts:2,completions:2},
      recommendations:{clicks:2,'book-opens':2,starts:1,completions:1},
      unattributed:{starts:1,completions:1},
    };
    const event = sql(`SELECT source_event_id FROM reader_chapter_completions
      WHERE user_id=${users[1]} AND chapter_id=${fixture('mixed').chapter}`);
    sql(`UPDATE books SET admin_hold=true WHERE id=${mixed}`);
    for (const width of [390,1440]) {
      await adminPage.setViewportSize({width,height:width === 390 ? 844 : 900});
      await privateResponse(await adminPage.goto(base+'/odmen/audience?days=7'));
      assertDelta(before,await numbers(adminPage),changes,legacyRecOpens);
      assert.deepEqual(await numbers(adminPage,'previous'),previous);
      await calendarPeriods(adminPage,7);
      assert.equal(await adminPage.getByTestId('audience-sources-health').count(),1);
      const html = await adminPage.content();
      for (const forbidden of ['FLOW-SOURCE-SENTINEL','Readable text',event,...emails,...legacyTokens]) {
        assert.ok(!html.includes(forbidden), 'source report exposed content or identity');
      }
      assert.ok(await adminPage.evaluate(() => document.documentElement.scrollWidth <= innerWidth+1),
        'source report must fit the viewport');
      await retired.marked(adminPage,['audience-sources-current','audience-sources-previous']);
      await adminPage.screenshot({path:path.join(shots,`audience-sources-${width}.png`),fullPage:true});
      await preview(adminPage,`audience-sources-${width}`);
    }
    for (const days of [1,30,90]) {
      await adminPage.locator('select[name="days"]').selectOption(String(days));
      await adminPage.locator('form[action="/odmen/audience"]').getByRole('button').click();
      await adminPage.waitForURL(url => url.pathname==='/odmen/audience' && url.searchParams.get('days')===String(days));
      await calendarPeriods(adminPage,days);
    }

    await retired.settings(readerPage);
    legacyTokens.push(await retired.rejectedLegacyCommands(readerPage));
    await read(readerPage,fixture('after-retirement'));
    await readerPage.goto(base+'/catalog?sort=newest');
    assert.equal(await readerPage.locator('[data-organic-ticket],[data-rec-ticket]').count(),0);
    retired.legacyBoundary();
    retired.noCapture(users,fixtures.map(item=>item.book));
    await privateResponse(await adminPage.goto(base+'/odmen/audience?days=7'));
    assert.deepEqual(await numbers(adminPage),historical,'later reading also leaves history unchanged');
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_completions WHERE user_id=${users[1]}`),6,
      'ordinary C progress remains usable without optional collection');
    await retired.settings(directPage);
    legacyTokens.push(await retired.rejectedLegacyCommands(directPage));
    assert.equal(retired.scopeSnapshot(users),scope,'retired commands do not toggle old scopes');
    retired.noCapture(users,fixtures.map(item=>item.book));
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${users[0]} AND role='administrator'`);
    await privateResponse(await adminPage.goto(base+'/odmen/audience'),403);
    assert.equal(await adminPage.locator('[data-metric]').count(),0);
    assert.deepEqual(errors,[]);
    console.log('FLOW sources browser PASS: labelled source history, genuine ordinary navigation/C reads, no optional raw or deliveries, chapter distinction, live admin, private 390/1440 report');
  } finally {
    if (browser) await browser.close();
    if (users.length) sql(`DELETE FROM account_roles WHERE user_id IN(${users.join(',')})`);
    if (fixtures.length) {
      const ids = fixtures.map(item => item.book).join(',');
      sql(`BEGIN;
        DELETE FROM reader_book_positions WHERE book_id IN(${ids});
        DELETE FROM reader_chapter_starts WHERE book_id IN(${ids});
        DELETE FROM reader_chapter_completions WHERE book_id IN(${ids});
        DELETE FROM chapter_publications WHERE chapter_id IN(SELECT id FROM chapters WHERE book_id IN(${ids}));
        DELETE FROM book_members WHERE book_id IN(${ids});
        DELETE FROM books WHERE id IN(${ids});
        DELETE FROM rec_quality_daily WHERE book_id IN(${ids});
        DELETE FROM organic_discovery_quality_daily WHERE book_id IN(${ids});
        DELETE FROM rec_dirty_buckets WHERE book_id IN(${ids});
        DELETE FROM organic_discovery_dirty WHERE book_id IN(${ids});
        COMMIT`);
    }
    sql(`DELETE FROM users WHERE email_normalized IN(${emails.map(email => `'${email}'`).join(',')})`);
  }
})().catch(error => {console.error(error);process.exitCode=1;});
