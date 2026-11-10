'use strict';
// FLOW: real navigation + consent + C progress, then F's retained source totals.
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
  throw new Error('Source acceptance requires disposable loopback tor_ci');
}
fs.mkdirSync(shots, {recursive:true});
const sources = ['catalog', 'search', 'recommendations', 'unattributed'];
const metrics = ['clicks', 'book-opens', 'starts', 'completions'];

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
  await page.getByLabel('Отображаемое имя').fill('Проверка источников');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('AudienceSourcesPassword2026');
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
  const policy = JSON.parse(sql('SELECT row_to_json(p) FROM rec_runtime_policy p WHERE singleton'));
  const fixtures = [], tickets = [], users = [];
  let browser;
  try {
    sql('UPDATE rec_runtime_policy SET hold=true WHERE singleton');
    for (const key of [...sources,'mixed','no-consent']) {
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
    await privateResponse(await readerPage.goto(base+'/admin/audience'), 403);
    assert.equal(await readerPage.locator('[data-metric]').count(), 0);
    await consent(readerPage,true);
    await consent(directPage,true);
    await workerSettled();
    await privateResponse(await adminPage.goto(base+'/admin/audience?days=7'));
    const before = await numbers(adminPage);
    const previous = await numbers(adminPage,'previous');
    const legacyRecOpens = sql(`SELECT EXISTS(SELECT 1 FROM rec_quality_daily
      WHERE book_opens IS NULL AND bucket_date>=(now() AT TIME ZONE 'UTC')::date-6
        AND bucket_date<(now() AT TIME ZONE 'UTC')::date+1)`) === 't';

    for (const source of ['catalog','search','recommendations']) {
      tickets.push(await navigate(readerPage,fixture(source),source));
      await read(readerPage,fixture(source));
    }
    assert.equal((await directPage.goto(base+`/books/${fixture('unattributed').book}`)).status(), 200);
    await read(directPage,fixture('unattributed'));
    // All three successful navigation channels compete for the same C fact.
    for (const source of ['catalog','recommendations','search']) {
      tickets.push(await navigate(readerPage,fixture('mixed'),source));
    }
    await read(readerPage,fixture('mixed'));
    await read(readerPage,second);
    // Moving away stops unrelated in-flight client observations before read-only checks.
    await readerPage.goto(base+'/me');
    await directPage.goto(base+'/me');
    await workerSettled();
    const mixed = fixture('mixed').book;
    assert.equal(scalar(`SELECT count(*) FROM organic_discovery_first_conversions c
      JOIN organic_discovery_touches t ON t.id=c.touch_id WHERE c.book_id=${mixed} AND t.source='search'`), 2);
    assert.equal(scalar(`SELECT count(*) FROM rec_first_conversions WHERE book_id=${mixed}`), 0);
    assert.equal(scalar(`SELECT count(*) FROM organic_discovery_unattributed_reads WHERE book_id=${mixed}`), 0);
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_completions WHERE user_id=${users[1]} AND book_id=${mixed}`), 2,
      'C chapters remain distinct while F counts one first book completion');
    assert.equal(scalar(`SELECT COALESCE(sum(book_opens),0) FROM rec_quality_daily
      WHERE book_id IN(${fixtures.map(item => item.book).join(',')})`),2,
    'the worker must materialize both genuine REC book openings');
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
      await privateResponse(await adminPage.goto(base+'/admin/audience?days=7'));
      assertDelta(before,await numbers(adminPage),changes,legacyRecOpens);
      assert.deepEqual(await numbers(adminPage,'previous'),previous);
      await calendarPeriods(adminPage,7);
      assert.equal(await adminPage.getByTestId('audience-sources-health').count(),1);
      const html = await adminPage.content();
      for (const forbidden of ['FLOW-SOURCE-SENTINEL','Readable text',event,...emails,...tickets]) {
        assert.ok(!html.includes(forbidden), 'source report exposed content or identity');
      }
      assert.ok(await adminPage.evaluate(() => document.documentElement.scrollWidth <= innerWidth+1),
        'source report must fit the viewport');
      await adminPage.screenshot({path:path.join(shots,`audience-sources-${width}.png`),fullPage:true});
    }
    for (const days of [1,30,90]) {
      await adminPage.locator('select[name="days"]').selectOption(String(days));
      await adminPage.locator('form[action="/admin/audience"]').getByRole('button').click();
      await adminPage.waitForURL('**/admin/audience?days='+days);
      await calendarPeriods(adminPage,days);
    }

    await consent(readerPage,false);
    await read(readerPage,fixture('no-consent'));
    await readerPage.goto(base+'/catalog?sort=newest');
    assert.equal(await readerPage.locator('[data-organic-ticket]').count(),0);
    await readerPage.goto(base+'/me');
    await workerSettled();
    await privateResponse(await adminPage.goto(base+'/admin/audience?days=7'));
    assertDelta(before,await numbers(adminPage),{unattributed:{starts:1,completions:1}},legacyRecOpens);
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_completions WHERE user_id=${users[1]}`),6,
      'opting out must preserve C progress, including new reading without collection');
    await consent(directPage,false);
    await directPage.goto(base+'/me');
    await workerSettled();
    await privateResponse(await adminPage.goto(base+'/admin/audience?days=7'));
    assertDelta(before,await numbers(adminPage),{},legacyRecOpens);
    for (const table of ['rec_assignments','organic_discovery_touches','organic_discovery_unattributed_reads']) {
      assert.equal(scalar(`SELECT count(*) FROM ${table} t JOIN rec_subjects s ON s.subject_id=t.subject_id
        WHERE s.user_id IN(${users.slice(1).join(',')})`),0,'opt-out must erase '+table);
    }
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${users[0]} AND role='administrator'`);
    await privateResponse(await adminPage.goto(base+'/admin/audience'),403);
    assert.equal(await adminPage.locator('[data-metric]').count(),0);
    assert.deepEqual(errors,[]);
    console.log('FLOW sources browser PASS: catalog/search/REC/direct, last source once, chapter distinction, worker, opt-out, private 390/1440 report');
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
