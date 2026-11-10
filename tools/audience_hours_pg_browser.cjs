'use strict';
// FLOW-04: actual browser requests reach RIL, then the private bounded report.
// Historical aggregate fixtures exercise comparison only; they are not visits.
const {chromium} = require('playwright');
const {execFileSync} = require('node:child_process');
const {randomBytes} = require('node:crypto');
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
  throw new Error('Hourly acceptance requires disposable loopback tor_ci');
}
fs.mkdirSync(shots, {recursive:true});
const surfaces = ['home','catalog','book','reader'];
// A default headless UA is correctly excluded by FLOW. This one context models
// an ordinary allowed client; no production anti-bot rule is changed.
const ordinaryUA = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36';

function sql(statement) {
  return execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1',
    '--dbname',db,'--command',statement], {
    encoding:'utf8', env:{...process.env, PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'},
  }).trim();
}
function scalar(statement) {
  const value = sql(statement);
  assert.match(value, /^\d+$/);
  return Number(value);
}
function json(statement) { return JSON.parse(sql(statement)); }
async function waitFor(check, label) {
  for (let attempt=0; attempt<180; attempt++) {
    if (await check()) return;
    await new Promise(resolve => setTimeout(resolve, 200));
  }
  throw new Error('not observed: '+label);
}
async function register(page, email) {
  await page.goto(base+'/register');
  await page.getByLabel('Отображаемое имя').fill('Проверка часов');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('AudienceHoursPassword2026');
  await page.getByRole('button', {name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
  return scalar(`SELECT id FROM users WHERE email_normalized='${email}'`);
}
async function privateResponse(response, status=200) {
  assert.equal(response.status(), status);
  const headers = await response.allHeaders();
  assert.equal(headers['cache-control'], 'no-store');
  assert.match(headers['x-robots-tag'], /noindex/);
  assert.match(headers['x-robots-tag'], /nofollow/);
}

function installHistory(anchor) {
  // All affected keys are old hours, so a live pump cannot write into them.
  const epochs = [3,4,27,171].map(hours => anchor-hours*3600);
  const predicate = 'bucket IN('+epochs.map(epoch => `to_timestamp(${epoch})`).join(',')+')';
  const saved = sql(`SELECT extract(epoch FROM bucket)::bigint,surface_code,acquisition_hint,
    html_requests,opted_in_html_requests,observed_opted_in_visit_starts
    FROM ril_flow_hourly WHERE ${predicate}`);
  const [current,empty,dayPrevious,previous] = epochs;
  sql(`BEGIN; DELETE FROM ril_flow_hourly WHERE ${predicate};
    INSERT INTO ril_flow_hourly VALUES
    (to_timestamp(${current}),0,0,2,0,0),(to_timestamp(${current}),0,1,3,0,0),
    (to_timestamp(${current}),1,0,0,0,0),(to_timestamp(${current}),3,3,7,0,0),
    (to_timestamp(${current}),4,0,999999,0,0),
    (to_timestamp(${previous}),0,0,11,0,0),(to_timestamp(${previous}),2,0,13,0,0),
    (to_timestamp(${previous}),3,0,0,0,0),(to_timestamp(${dayPrevious}),2,0,17,0,0);
    COMMIT`);
  return {
    current, empty, previous,
    restore() {
      const rows = saved ? saved.split('\n').map(line => {
        const fields = line.split('|');
        assert.equal(fields.length, 6);
        fields.forEach(field => assert.match(field, /^\d+$/));
        return `INSERT INTO ril_flow_hourly VALUES(to_timestamp(${fields[0]}),${fields.slice(1).join(',')});`;
      }).join('') : '';
      sql(`BEGIN; DELETE FROM ril_flow_hourly WHERE ${predicate}; ${rows} COMMIT`);
    },
  };
}

async function checkReport(page, days) {
  const bounds = {};
  for (const period of ['current','previous']) {
    const section = page.getByTestId('audience-hours-'+period);
    bounds[period] = await section.locator('time[datetime]').evaluateAll(nodes =>
      nodes.map(node => Date.parse(node.getAttribute('datetime'))/1000));
    assert.equal(bounds[period].length, 2);
    const [from,until] = bounds[period];
    assert.ok(Number.isSafeInteger(from) && Number.isSafeInteger(until));
    assert.equal(from%3600, 0);
    assert.equal(until-from, days*86400);
    const expected = json(`SELECT json_agg(row_to_json(r) ORDER BY r.surface) FROM (
      SELECT s.surface, sum(f.html_requests)::text AS saved,
        count(DISTINCT f.bucket)::int AS hours
      FROM (VALUES(0),(1),(2),(3)) s(surface)
      LEFT JOIN ril_flow_hourly f ON f.surface_code=s.surface
        AND f.bucket>=to_timestamp(${from}) AND f.bucket<to_timestamp(${until})
      GROUP BY s.surface) r`);
    for (let index=0;index<surfaces.length;index++) {
      const card = section.locator(`[data-surface="${surfaces[index]}"]`);
      assert.equal((await card.locator('[data-metric="saved-views"]').innerText()).trim(), expected[index].saved ?? 'Нет данных');
      assert.equal((await card.locator('[data-metric="recorded-hours"]').innerText()).trim(), String(expected[index].hours));
    }
  }
  assert.equal(bounds.previous[1], bounds.current[0]);
  const quality = await page.getByTestId('audience-data-quality').innerText();
  assert.match(quality, /Недостоверность измерений/);
  assert.match(quality, /процесс/);
  const table = page.getByTestId('audience-hours-table');
  const hours = await table.locator('tr[data-hour]').evaluateAll(rows => rows.map(row => Number(row.dataset.hour)));
  assert.equal(hours.length, days*24);
  assert.equal(hours[0], bounds.current[0]);
  assert.equal(hours.at(-1), bounds.current[1]-3600);
  assert.equal(new Set(hours).size, days*24);
  assert.equal(await table.locator(`tr[data-hour="${bounds.current[1]}"]`).count(), 0, 'unfinished current hour is excluded');
  for (let index=1;index<hours.length;index++) assert.equal(hours[index]-hours[index-1], 3600);
  return bounds;
}

async function checkFixtureCells(page, fixture) {
  const row = page.getByTestId('audience-hours-table').locator(`tr[data-hour="${fixture.current}"]`);
  for (const [period,expected] of [['current',['5','0','—','7']],['previous',['11','—','13','0']]]) {
    for (let index=0;index<surfaces.length;index++) {
      const cell = row.locator(`td[data-surface="${surfaces[index]}"][data-period="${period}"]`);
      assert.equal((await cell.innerText()).trim(), expected[index], period+' '+surfaces[index]);
    }
  }
  const empty = page.getByTestId('audience-hours-table').locator(`tr[data-hour="${fixture.empty}"]`);
  for (const surface of surfaces) {
    assert.equal((await empty.locator(`td[data-surface="${surface}"][data-period="current"]`).innerText()).trim(), '—');
  }
}

function publicCounts(from) {
  return json(`SELECT json_agg(row_to_json(r) ORDER BY r.surface) FROM (
    SELECT s.surface, COALESCE(sum(f.html_requests),0)::bigint AS count
    FROM (VALUES(0),(1),(2),(3)) s(surface)
    LEFT JOIN ril_flow_hourly f ON f.surface_code=s.surface AND f.bucket>=to_timestamp(${from})
    GROUP BY s.surface) r`).map(row => row.count);
}

(async () => {
  const run = Date.now()+'-'+randomBytes(4).toString('hex');
  const adminEmail = `flow-hours-admin-${run}@tor-ci.example`;
  const readerEmail = `flow-hours-reader-${run}@tor-ci.example`;
  let browser, administrator, reader, book, history;
  try {
    browser = await chromium.launch({headless:true});
    const adminContext = await browser.newContext({viewport:{width:1440,height:900}});
    const readerContext = await browser.newContext();
    const guestContext = await browser.newContext({userAgent:ordinaryUA});
    const adminPage = await adminContext.newPage();
    const readerPage = await readerContext.newPage();
    const guestPage = await guestContext.newPage();
    const errors = [];
    adminPage.on('pageerror', error => errors.push(error.message));
    administrator = await register(adminPage, adminEmail);
    reader = await register(readerPage, readerEmail);
    sql(`INSERT INTO account_roles(user_id,role) VALUES(${administrator},'administrator')`);
    await adminPage.goto(base+'/me');
    await adminPage.getByRole('link', {name:'Аудитория',exact:true}).click();
    await adminPage.waitForURL(base+'/admin/audience');
    await privateResponse(await guestPage.goto(base+'/admin/audience'),401);
    await privateResponse(await readerPage.goto(base+'/admin/audience'),403);
    assert.equal(await readerPage.getByTestId('audience-hours').count(), 0);

    const anchor = scalar("SELECT extract(epoch FROM date_trunc('hour',now(),'UTC'))::bigint");
    history = installHistory(anchor);
    for (const width of [390,1440]) {
      await adminPage.setViewportSize({width,height:width===390 ? 844 : 900});
      await privateResponse(await adminPage.goto(base+'/admin/audience?days=7'));
      await checkReport(adminPage,7);
      const hours = adminPage.getByTestId('audience-hours');
      assert.match(await hours.innerText(), /час.*сохран|сохран.*час/);
      assert.ok(!(await hours.innerText()).includes('999999'), 'private-page category excluded');
      await hours.scrollIntoViewIfNeeded();
      await adminPage.screenshot({path:path.join(shots,`audience-hours-${width}.png`)});
      await hours.getByText('Все часы и категории',{exact:true}).click();
      assert.ok(await adminPage.getByTestId('audience-hours-table').isVisible());
      await checkFixtureCells(adminPage,history);
      assert.ok(await adminPage.evaluate(() => document.documentElement.scrollWidth<=innerWidth+1), 'hour table must scroll inside its own container');
      await adminPage.getByTestId('audience-hours-table').scrollIntoViewIfNeeded();
      await adminPage.screenshot({path:path.join(shots,`audience-hours-table-${width}.png`)});
    }
    for (const days of [1,30,90]) {
      await adminPage.locator('select[name="days"]').selectOption(String(days));
      await adminPage.locator('form[action="/admin/audience"]').getByRole('button').click();
      await adminPage.waitForURL('**/admin/audience?days='+days);
      if (days===1) await checkReport(adminPage,days);
      else {
        assert.equal(await adminPage.getByTestId('audience-hours-unsupported').count(),1);
        assert.equal(await adminPage.getByTestId('audience-hours-table').count(),0);
        assert.equal(await adminPage.getByTestId('audience-sources-current').count(),1);
      }
    }

    book = scalar(`INSERT INTO books(source_identity,target_language,title,visibility,access_level)
      VALUES('ci:flow:hours:${run}','ru','FLOW-HOURS-PRIVATE-SENTINEL-${run}','public','basic') RETURNING id`);
    const chapter = scalar(`INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
      VALUES(${book},1,'Hours chapter','public','free',0) RETURNING id`);
    const version = scalar(`INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
      VALUES(${chapter},1,'Hours actual page text') RETURNING id`);
    sql(`INSERT INTO chapter_publications(chapter_id,version_id) VALUES(${chapter},${version})`);
    const captureFrom = scalar("SELECT extract(epoch FROM date_trunc('hour',now(),'UTC'))::bigint");
    const before = publicCounts(captureFrom);
    for (const route of ['/', '/catalog', `/books/${book}`, `/reader/${chapter}`, `/reader/${chapter}`]) {
      assert.equal((await guestPage.goto(base+route)).status(),200);
    }
    const expected = before.map((value,index) => value+[1,1,1,2][index]);
    await waitFor(() => publicCounts(captureFrom).every((value,index) => value>=expected[index]), 'actual HTML requests saved by the existing RIL pump');
    assert.deepEqual(publicCounts(captureFrom),expected,'repeat page opens are views, not new unique people');
    const batches = scalar('SELECT count(*) FROM ril_flush_batches WHERE finished_at IS NOT NULL');
    assert.equal((await guestPage.request.get(base+'/api/v1/books')).status(),200);
    assert.equal((await guestPage.request.get(base+'/flow-hours-unregistered')).status(),404);
    const botContext = await browser.newContext({userAgent:'Googlebot/2.1'});
    assert.equal((await botContext.request.get(base+'/')).status(),200);
    await waitFor(() => scalar('SELECT count(*) FROM ril_flush_batches WHERE finished_at IS NOT NULL')>batches, 'negative request batch flushed');
    assert.deepEqual(publicCounts(captureFrom),expected,'API, 404 and bot requests must not become pageviews');
    await botContext.close();
    await privateResponse(await adminPage.goto(base+'/admin/audience?days=7'));
    await checkReport(adminPage,7);
    const content = await adminPage.content();
    for (const forbidden of [adminEmail,readerEmail,'FLOW-HOURS-PRIVATE-SENTINEL','Hours actual page text']) {
      assert.ok(!content.includes(forbidden),'aggregate report exposed identity or content');
    }
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${administrator} AND role='administrator'`);
    await privateResponse(await adminPage.goto(base+'/admin/audience'),403);
    assert.equal(await adminPage.getByTestId('audience-hours').count(),0);
    await adminPage.goto(base+'/me');
    assert.equal(await adminPage.locator('a[href="/admin/audience"]').count(),0);
    assert.deepEqual(errors,[]);
    console.log('FLOW hourly browser PASS: real HTML/RIL capture, excluded API/404/bot, unfinished hour excluded, 1/7 periods, unknown/zero, 30/90 limits, live admin revocation, 390/1440 px');
  } finally {
    if (browser) await browser.close();
    if (history) history.restore();
    if (administrator) sql(`DELETE FROM account_roles WHERE user_id=${administrator}`);
    if (book) sql(`BEGIN;
      DELETE FROM chapter_publications WHERE chapter_id IN(SELECT id FROM chapters WHERE book_id=${book});
      DELETE FROM book_members WHERE book_id=${book}; DELETE FROM books WHERE id=${book}; COMMIT`);
    if (administrator || reader) sql(`DELETE FROM users WHERE email_normalized IN('${adminEmail}','${readerEmail}')`);
  }
})().catch(error => {console.error(error);process.exitCode=1;});
