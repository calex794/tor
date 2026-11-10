'use strict';
// FLOW-05: actual discovery and reading reach the administrator book report.
// Catalogue fixtures are allowed; no positive analytics fact is inserted here.
const {chromium} = require('playwright');
const {execFileSync} = require('node:child_process');
const {createHash, randomBytes} = require('node:crypto');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const db = process.env.APP_URL || '';
const base = process.env.TOR_AUDIENCE_BASE || 'http://127.0.0.1:3108';
const shots = process.env.TOR_AUDIENCE_SHOTS || '/tmp/audience-browser-shots';
const parsed = new URL(db);
if (parsed.protocol !== 'postgresql:' || parsed.hostname !== '127.0.0.1'
    || parsed.port !== '5432' || parsed.pathname !== '/tor_ci'
    || !/^http:\/\/127\.0\.0\.1:\d+$/.test(base)) {
  throw new Error('Book-report acceptance requires disposable loopback tor_ci');
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
async function waitFor(check, label) {
  for (let attempt=0; attempt<150; attempt++) {
    if (await check()) return;
    await new Promise(resolve => setTimeout(resolve, 200));
  }
  throw new Error('not observed: '+label);
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
async function consent(page, enabled) {
  await page.goto(base+'/me/recommendations');
  await page.locator("form[action='/me/recommendations/consent']").getByRole('checkbox').setChecked(enabled);
  await page.getByRole('button', {name:'Сохранить выбор',exact:true}).click();
  await page.waitForURL('**/me/recommendations');
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
async function navigate(page, fixture, source) {
  const target = source === 'recommendations' ? '/' : source === 'search'
    ? '/catalog?q='+encodeURIComponent(fixture.title) : '/catalog?sort=newest';
  assert.equal((await page.goto(base+target)).status(), 200);
  let link, ticket;
  if (source === 'recommendations') {
    const card = page.locator('[data-rec-card]').filter({
      has:page.locator(`a[data-rec-link][href='/books/${fixture.book}']`),
    }).first();
    await card.scrollIntoViewIfNeeded();
    ticket = await card.getAttribute('data-rec-ticket');
    assert.ok(ticket, 'consented recommendation must have an issued ticket');
    const digest = createHash('sha256').update(ticket).digest('hex');
    await waitFor(() => scalar(`SELECT count(*) FROM rec_assignments
      WHERE ticket_digest=decode('${digest}','hex') AND viewed_at IS NOT NULL`) === 1,
    'actual visible recommendation impression');
    link = card.locator('a[data-rec-link]');
  } else {
    link = page.locator(`a.book-mini-card__link[href='/books/${fixture.book}'][data-organic-ticket]`).first();
    assert.ok(await link.isVisible(), 'consented '+source+' result must have a ticket');
    ticket = await link.getAttribute('data-organic-ticket');
  }
  await link.click();
  await page.waitForURL(base+`/books/${fixture.book}`);
  const digest = createHash('sha256').update(ticket).digest('hex');
  const table = source === 'recommendations' ? 'rec_assignments' : 'organic_discovery_touches';
  const predicate = `ticket_digest=decode('${digest}','hex') AND clicked_at IS NOT NULL AND book_opened_at IS NOT NULL`;
  await waitFor(() => scalar(`SELECT count(*) FROM ${table} WHERE ${predicate}`) === 1,
    source+' POST and successful canonical book open');
  // Replay only the ticket already activated by the genuine click above.
  const status = await page.evaluate(async ({source,ticket,csrf}) => {
    const response = await fetch(source === 'recommendations'
      ? '/api/v1/recommendations/activate' : '/api/v1/discovery/activate', {
      method:'POST',credentials:'same-origin',
      headers:{'Content-Type':'application/x-www-form-urlencoded'},
      body:new URLSearchParams({ticket,csrf_token:csrf}),
    });
    return response.status;
  }, {source,ticket,csrf:await csrf(page)});
  assert.equal(status, source === 'recommendations' ? 200 : 204);
  assert.equal(scalar(`SELECT count(*) FROM ${table} WHERE ${predicate}`), 1);
  return ticket;
}
async function workerSettled() {
  await waitFor(() => sql(`SELECT EXISTS(SELECT 1 FROM rec_worker_health
      WHERE last_success>now()-interval '30 seconds')
      AND NOT EXISTS(SELECT 1 FROM rec_dirty_buckets)
      AND NOT EXISTS(SELECT 1 FROM organic_discovery_dirty)
      AND NOT EXISTS(SELECT 1 FROM rec_verified_read_events_v2 v
        WHERE v.occurred_at>now()-interval '30 days' AND NOT EXISTS(
          SELECT 1 FROM event_deliveries d WHERE d.event_id=v.event_id
            AND d.consumer='recommendation-feedback-v1'))`) === 't',
  'the actual feedback worker delivers C events and refreshes both daily projections');
}

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
  const policy = JSON.parse(sql('SELECT row_to_json(p) FROM rec_runtime_policy p WHERE singleton'));
  const fixtures = [], users = [], tickets = [], errors = [];
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
    sql('UPDATE rec_runtime_policy SET hold=true WHERE singleton');
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
    await consent(readerPage,true);
    for (const source of ['catalog','search','recommendations']) {
      tickets.push(await navigate(readerPage,fixture(source),source));
      await read(readerPage,fixture(source));
    }
    await readerPage.goto(base+`/books/${fixture('direct').book}`);
    await read(readerPage,fixture('direct'));
    await readerPage.goto(base+'/me');
    await readerPage.waitForLoadState('networkidle');
    await workerSettled();
    // A new public book with no issued link is the empty state, not zero demand.
    const zero = create('zero','Новая книга без наблюдений '+run);
    sql(`UPDATE books SET admin_hold=true WHERE id=${fixture('hidden').book}`);
    await adminPage.goto(base+'/me');
    await adminPage.locator('a[href="/admin/audience"]').click();
    await adminPage.waitForURL('**/admin/audience');
    await adminPage.locator('a[href^="/admin/audience/books"]').click();
    await adminPage.waitForURL(url => url.pathname === '/admin/audience/books');
    await filter(adminPage,7,genre,tag);
    await periods(adminPage,7);
    const reportUrl = adminPage.url();
    const baseline = {};
    for (const item of fixtures.filter(item => item.key !== 'hidden')) {
      baseline[item.key] = await numbers(adminPage,item.book);
      const previous = await numbers(adminPage,item.book,'previous');
      assert.ok(Object.values(previous).every(value => value === null),'new books have no previous buckets');
    }
    for (const key of ['catalog','search','recommendations','direct']) {
      assert.equal(baseline[key].starts,1,'actual first book reading '+key);
    }
    for (const key of ['catalog','search']) assert.ok(baseline[key].issued > 0);
    assert.ok(baseline.recommendations.impressions > 0,'actual visible REC impression');
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
      for (const forbidden of ['HIDDEN-BOOK-SENTINEL','Readable text',...emails,...tickets]) {
        assert.ok(!html.includes(forbidden),'report leaks hidden content or identity');
      }
      assert.ok(await adminPage.evaluate(() => document.documentElement.scrollWidth <= innerWidth+1),
        'book report must fit the viewport');
      await adminPage.screenshot({path:path.join(shots,`audience-books-${width}.png`),fullPage:true});
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

    await consent(readerPage,false);
    await read(readerPage,zero);
    await readerPage.goto(base+'/catalog?sort=newest');
    assert.equal(await readerPage.locator('[data-organic-ticket]').count(),0);
    await readerPage.goto(base+'/me');
    await readerPage.waitForLoadState('networkidle');
    await workerSettled();
    await privateResponse(await adminPage.goto(reportUrl));
    for (const item of fixtures.filter(item => !['hidden','search'].includes(item.key))) {
      assert.equal((await numbers(adminPage,item.book)).starts ?? 0,0,'opt-out erases personal attribution');
    }
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_starts WHERE user_id=${users[1]}`),5,
      'consent withdrawal preserves C reading and allows new reading');
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${users[0]} AND role='administrator'`);
    await privateResponse(await adminPage.goto(reportUrl),403);
    assert.equal(await adminPage.getByTestId('audience-book-row').count(),0);
    assert.deepEqual(errors,[]);
    console.log('FLOW books browser PASS: real catalog/search/REC/direct first reads, filters, periods, hidden metadata, replay, opt-out, live admin, 390/1440');
  } finally {
    if (browser) await browser.close();
    const uuid = value => value === null ? 'NULL' : `'${value}'::uuid`;
    sql(`UPDATE rec_runtime_policy SET active_generation=${uuid(policy.active_generation)},
      previous_generation=${uuid(policy.previous_generation)},revision=${policy.revision},
      rollout_percent=${policy.rollout_percent},hold=${policy.hold ? 'true' : 'false'} WHERE singleton`);
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
