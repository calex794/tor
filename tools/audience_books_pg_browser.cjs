'use strict';
// Historical book report fixtures, with real ordinary navigation and C progress.
// Current activity must not populate the retired consent-based measurements.
const {chromium} = require('playwright');
const {execFileSync} = require('node:child_process');
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
  throw new Error('Book-report acceptance requires disposable loopback legacy analytics baseline 191');
}
fs.mkdirSync(shots, {recursive:true});
const metrics = ['issued', 'impressions', 'clicks', 'book-opens', 'starts'];

function sql(statement) {
  return execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1',
    '--dbname',db,'--command',statement], {
    encoding:'utf8', env:{...process.env, PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'},
  }).trim();
}
function scalar(statement) {
  const raw = sql(statement);
  assert.match(raw, /^\d+$/, 'expected numeric database evidence');
  return Number(raw);
}
async function register(page, email) {
  await page.goto(base+'/register');
  await page.getByLabel('Отображаемое имя').fill('Проверка книг');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('AudienceBooksPassword2026');
  await page.getByRole('button', {name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
  return scalar(`SELECT id FROM users WHERE email_normalized='${email}'`);
}
async function csrf(page) {
  const value = (await page.context().cookies()).find(cookie => cookie.name === 'tor-dev-csrf')?.value;
  assert.ok(value, 'account forms must issue the normal CSRF cookie');
  return value;
}
async function privateResponse(response, status=200) {
  assert.equal(response.status(), status);
  const headers = await response.allHeaders();
  assert.equal(headers['cache-control'], 'no-store');
  assert.match(headers['x-robots-tag'], /noindex/);
  assert.match(headers['x-robots-tag'], /nofollow/);
}
function chapter(book, number) {
  const id = scalar(`INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
    VALUES(${book},${number},'Source chapter ${number}','public','free',0) RETURNING id`);
  const version = scalar(`INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
    VALUES(${id},1,'Readable text') RETURNING id`);
  sql(`INSERT INTO chapter_publications(chapter_id,version_id) VALUES(${id},${version})`);
  return {chapter:id, version};
}
async function read(page, fixture) {
  assert.equal((await page.goto(base+`/read/${fixture.chapter}`)).status(), 200);
  assert.equal((await page.goto(base+`/reader/${fixture.chapter}`)).status(), 200);
  await page.waitForFunction(() => document.querySelector('[data-free-progress-chapter]')?.dataset.freeProgressReady === '1');
  await page.evaluate(async ({chapter,version,csrf}) => {
    const state = await fetch('/api/v1/me/progress/free?chapter_id='+chapter, {
      credentials:'same-origin',
    });
    if (!state.ok) throw new Error('C position lookup rejected: '+state.status);
    const initialRevision = (await state.json()).revision;
    async function save(offset, revision, expected=200) {
      const response = await fetch('/api/v1/me/progress/free', {
        method:'PUT', credentials:'same-origin',
        headers:{'Content-Type':'application/json','X-CSRF-Token':csrf},
        body:JSON.stringify({chapter_id:String(chapter),version_id:String(version),
          block_id:1,char_offset:offset,expected_revision:revision}),
      });
      if (response.status !== expected) throw new Error(`C progress ${response.status}, expected ${expected}`);
      return expected === 200 ? (await response.json()).revision : null;
    }
    const started = await save(3, initialRevision);
    const finished = await save(13, started);
    await save(13, finished, 409);
  }, {...fixture, csrf:await csrf(page)});
}
const retired = require('./audience_retired_pg_helpers.cjs')({sql,scalar,base});
const navigate = retired.navigate;

async function numbers(page, book, period='current') {
  const row = page.getByTestId('audience-book-row').filter({
    has:page.locator(`a[href='/books/${book}']`),
  });
  assert.equal(await row.count(),1,'current public book row '+book);
  const values = {};
  for (const metric of metrics) {
    const text = (await row.locator(`[data-period='${period}'] [data-metric='${metric}']`).innerText()).trim();
    assert.ok(/^\d+$/.test(text) || text === 'Нет данных',text);
    values[metric] = /^\d+$/.test(text) ? Number(text) : null;
  }
  return values;
}

async function filter(page, days, genre, tag='') {
  await page.locator('#books-days').selectOption(String(days));
  await page.locator('#books-genre').fill(genre);
  await page.locator('#books-tag').fill(tag);
  await page.locator('#audience-books-filters').getByRole('button',{name:'Показать',exact:true}).click();
  await page.waitForURL(url => url.pathname === '/admin/audience/books'
    && url.searchParams.get('days') === String(days)
    && url.searchParams.get('genre') === genre && url.searchParams.get('tag') === tag);
}

async function periods(page, days) {
  const bounds = {};
  for (const period of ['current','previous']) {
    bounds[period] = (await page.getByTestId('audience-books-'+period).innerText())
      .match(/\b\d{4}-\d{2}-\d{2}\b/g) || [];
    assert.equal(bounds[period].length,2);
    assert.equal(Date.parse(bounds[period][1])-Date.parse(bounds[period][0]),days*86400000);
  }
  assert.equal(bounds.previous[1],bounds.current[0]);
  assert.equal(bounds.current[1],sql("SELECT ((now() AT TIME ZONE 'UTC')::date+1)::text"));
}

(async () => {
  const run = Date.now()+'-'+randomBytes(4).toString('hex');
  const genre = 'испытание '+run;
  const tag = 'путь + чтение';
  const emails = ['admin','reader'].map(role => `flow-books-${role}-${run}@tor-ci.example`);
  const fixtures = [], users = [], legacyTokens = [], errors = [];
  let browser;
  function create(key, title) {
    const book = scalar(`INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres,tags)
      VALUES('ci:flow:books:${run}:${key}','ru','${title}','public','basic',ARRAY['${genre}'],ARRAY['${tag}']) RETURNING id`);
    const fixture = {key,book,title};
    fixtures.push(fixture);
    Object.assign(fixture,chapter(book,1));
    return fixture;
  }
  const fixture = key => fixtures.find(item => item.key === key);
  try {
    for (const [key,title] of [
      ['catalog','Дорога из каталога'],['search','Находка в поиске'],
      ['recommendations','Совет читателю'],['direct','Чтение по прямой ссылке'],
      ['unread','Путешествие без первого чтения'],['hidden','HIDDEN-BOOK-SENTINEL-'+run],
    ]) create(key,title+' '+run);
    browser = await chromium.launch({headless:true});
    const pages = [];
    for (const [index,email] of emails.entries()) {
      const context = await browser.newContext({viewport:{width:index === 0 ? 1440 : 390,height:900}});
      const page = await context.newPage();
      page.on('pageerror',error => errors.push(error.message));
      users.push(await register(page,email));
      pages.push(page);
    }
    const [adminPage,readerPage] = pages;
    sql(`INSERT INTO account_roles(user_id,role) VALUES(${users[0]},'administrator')`);
    await privateResponse(await readerPage.goto(base+'/admin/audience/books?days=invalid'),403);
    assert.equal(await readerPage.getByTestId('audience-book-row').count(),0);
    retired.oldScopes(users.slice(1));
    const scope = retired.scopeSnapshot(users);
    await retired.settings(readerPage);
    legacyTokens.push(await retired.rejectedLegacyCommands(readerPage));
    retired.history([
      {book:fixture('catalog').book,source:'catalog',issued:2,clicks:1,opens:1,starts:1,completions:1},
      {book:fixture('search').book,source:'search',issued:2,clicks:1,opens:1,starts:1,completions:1},
      {book:fixture('recommendations').book,source:'recommendations',impressions:2,clicks:1,opens:1,starts:1,completions:1},
      {book:fixture('direct').book,source:'unattributed',starts:1,completions:1},
      {book:fixture('unread').book,source:'catalog',issued:1},
    ]);
    retired.noCapture(users,fixtures.map(item=>item.book));
    for (const source of ['catalog','search','recommendations']) {
      await navigate(readerPage,fixture(source),source);
      await read(readerPage,fixture(source));
    }
    await readerPage.goto(base+`/books/${fixture('direct').book}`);
    await read(readerPage,fixture('direct'));
    await readerPage.goto(base+'/me');
    await readerPage.waitForLoadState('networkidle');
    retired.legacyBoundary();
    retired.noCapture(users,fixtures.map(item=>item.book));
    // A new public book with no historical row remains unknown.
    const zero = create('zero','Новая книга без наблюдений '+run);
    sql(`UPDATE books SET admin_hold=true WHERE id=${fixture('hidden').book}`);
    await adminPage.goto(base+'/me');
    await adminPage.locator('a[href="/admin/audience"]').click();
    await adminPage.waitForURL('**/admin/audience');
    await adminPage.locator('a[href^="/admin/audience/books"]').click();
    await adminPage.waitForURL(url => url.pathname === '/admin/audience/books');
    await filter(adminPage,7,genre,tag);
    await periods(adminPage,7);
    await retired.marked(adminPage,['audience-books-current','audience-books-previous']);
    const reportUrl = adminPage.url();
    const baseline = {};
    for (const item of fixtures.filter(item => item.key !== 'hidden')) {
      baseline[item.key] = await numbers(adminPage,item.book);
      const previous = await numbers(adminPage,item.book,'previous');
      assert.ok(Object.values(previous).every(value => value === null),'new books have no previous buckets');
    }
    for (const key of ['catalog','search','recommendations','direct']) {
      assert.equal(baseline[key].starts,1,'retained historical first reading '+key);
    }
    for (const key of ['catalog','search']) assert.ok(baseline[key].issued > 0);
    assert.ok(baseline.recommendations.impressions > 0,'retained historical REC impression');
    assert.equal(baseline.unread.starts,0,'issued book has no confirmed first reading');
    assert.ok(baseline.unread.issued > 0,'an issued link is observed separately from visible impressions');
    assert.ok(Object.values(baseline.zero).every(value => value === null),'no records must stay unknown');
    for (const [key,state] of [
      ['catalog','with-starts'],['search','with-starts'],['recommendations','with-starts'],
      ['direct','with-starts'],['unread','observed-without-starts'],['zero','no-observations'],
    ]) {
      assert.equal(await adminPage.locator(`[data-book-id='${fixture(key).book}'] [data-period='current']`).getAttribute('data-state'),state);
    }
    assert.equal(await adminPage.getByTestId('audience-books-current').locator('[data-state="with-starts"]').innerText(),'4');
    assert.equal(await adminPage.getByTestId('audience-books-current').locator('[data-state="observed-without-starts"]').innerText(),'1');
    assert.equal(await adminPage.getByTestId('audience-books-current').locator('[data-state="no-observations"]').innerText(),'1');
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_starts WHERE user_id=${users[1]}`),4);

    for (const width of [390,1440]) {
      await adminPage.setViewportSize({width,height:width === 390 ? 844 : 900});
      await privateResponse(await adminPage.goto(reportUrl));
      for (const item of fixtures.filter(item => item.key !== 'hidden')) {
        assert.deepEqual(await numbers(adminPage,item.book),baseline[item.key],'report reads cannot increase counters');
      }
      const html = await adminPage.content();
      for (const forbidden of ['HIDDEN-BOOK-SENTINEL','Readable text',...emails,...legacyTokens]) {
        assert.ok(!html.includes(forbidden),'report leaks hidden content or identity');
      }
      assert.ok(await adminPage.evaluate(() => document.documentElement.scrollWidth <= innerWidth+1),
        'book report must fit the viewport');
      await retired.marked(adminPage,['audience-books-current','audience-books-previous']);
      await adminPage.screenshot({path:path.join(shots,`audience-books-${width}.png`),fullPage:true});
      await preview(adminPage,`audience-books-${width}`);
    }
    for (const days of [1,30,90]) {
      await filter(adminPage,days,genre,tag);
      await periods(adminPage,days);
      assert.equal((await numbers(adminPage,fixture('catalog').book)).starts,1);
    }
    await filter(adminPage,7,genre,'несуществующая метка');
    assert.equal(await adminPage.getByTestId('audience-book-row').count(),0);
    assert.equal(await adminPage.getByTestId('audience-books-empty').count(),1);
    await adminPage.getByRole('link',{name:'Сбросить фильтры',exact:true}).click();
    await adminPage.waitForURL(base+'/admin/audience/books?days=7');
    assert.equal(await adminPage.locator('#books-genre').inputValue(),'');
    assert.equal(await adminPage.locator('#books-tag').inputValue(),'');
    await filter(adminPage,7,genre,tag);
    // Current metadata, including visibility, controls this named book report.
    sql(`UPDATE books SET tags=ARRAY['другая метка'] WHERE id=${fixture('search').book}`);
    await privateResponse(await adminPage.goto(reportUrl));
    assert.equal(await adminPage.locator(`[data-book-id='${fixture('search').book}']`).count(),0);
    await filter(adminPage,7,genre,'другая метка');
    assert.equal(await adminPage.getByTestId('audience-book-row').count(),1);
    assert.equal((await numbers(adminPage,fixture('search').book)).starts,1);
    sql(`UPDATE books SET tags=ARRAY['${tag}'],admin_hold=true WHERE id=${fixture('search').book}`);
    await privateResponse(await adminPage.goto(reportUrl));
    assert.equal(await adminPage.locator(`[data-book-id='${fixture('search').book}']`).count(),0);

    await retired.settings(readerPage);
    legacyTokens.push(await retired.rejectedLegacyCommands(readerPage));
    await read(readerPage,zero);
    await readerPage.goto(base+'/catalog?sort=newest');
    assert.equal(await readerPage.locator('[data-organic-ticket],[data-rec-ticket]').count(),0);
    retired.legacyBoundary();
    retired.noCapture(users,fixtures.map(item=>item.book));
    assert.equal(retired.scopeSnapshot(users),scope,'retired routes do not mutate old scopes');
    await privateResponse(await adminPage.goto(reportUrl));
    for (const item of fixtures.filter(item => !['hidden','search'].includes(item.key))) {
      assert.deepEqual(await numbers(adminPage,item.book),baseline[item.key],
        'new C reading cannot change the historical report');
    }
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_starts WHERE user_id=${users[1]}`),5,
      'ordinary C reading continues, including a book with no historical analytics row');
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${users[0]} AND role='administrator'`);
    await privateResponse(await adminPage.goto(reportUrl),403);
    assert.equal(await adminPage.getByTestId('audience-book-row').count(),0);
    assert.deepEqual(errors,[]);
    console.log('FLOW books browser PASS: retained labelled history, real ordinary navigation/C reading, no new capture, filters, periods, hidden metadata, live admin, 390/1440');
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
