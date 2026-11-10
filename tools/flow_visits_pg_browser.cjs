'use strict';
// FLOW-02: explicit opt-in, actual pages, account scope, private report, erasure.
// Positive visit observations are never inserted by these catalogue fixtures.
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
  throw new Error('FLOW visit acceptance requires disposable loopback tor_ci');
}
fs.mkdirSync(shots, {recursive:true});
const ordinaryUA = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36';
const recForm = "form[action='/me/recommendations/consent']";
const flowForm = 'form[data-flow-consent]';

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
async function waitFor(check, label) {
  for (let attempt=0; attempt<100; attempt++) {
    if (await check()) return;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw new Error('not observed: '+label);
}
function ownPages(user) {
  return scalar(`SELECT COALESCE(sum(v.pageviews),0) FROM flow_visits v
    JOIN rec_subjects s ON s.subject_id=v.subject_id WHERE s.user_id=${user}`);
}
async function waitPages(user, expected) {
  await waitFor(() => {
    const count = ownPages(user);
    assert.ok(count <= expected, `duplicate/extra capture ${count} > ${expected}`);
    return count === expected;
  }, `${expected} successful consented HTML pages`);
}
async function register(page, email) {
  await page.goto(base+'/register');
  await page.getByLabel('Отображаемое имя').fill('Проверка визитов');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('FlowVisitsBrowserPassword2026');
  await page.getByRole('button', {name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
  return scalar(`SELECT id FROM users WHERE email_normalized='${email}'`);
}
async function login(page, email) {
  await page.goto(base+'/login');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль').fill('FlowVisitsBrowserPassword2026');
  await page.getByRole('button', {name:'Войти', exact:true}).click();
  await page.waitForURL('**/me');
}
async function choose(page, selector, enabled) {
  await page.goto(base+'/me/recommendations');
  const form = page.locator(selector);
  await form.getByRole('checkbox').setChecked(enabled);
  await form.getByRole('button').click();
  await page.waitForURL('**/me/recommendations');
  assert.equal(await page.locator(selector).getByRole('checkbox').isChecked(), enabled);
}
async function privateResponse(response, status=200) {
  assert.equal(response.status(), status);
  const headers = await response.allHeaders();
  assert.equal(headers['cache-control'], 'no-store');
  assert.match(headers['x-robots-tag'], /noindex/);
  assert.match(headers['x-robots-tag'], /nofollow/);
}
async function invalidConsent(page, csrf) {
  return page.evaluate(async ({selector,csrf}) => {
    const form = document.querySelector(selector);
    const data = new URLSearchParams(new FormData(form));
    data.set('enabled','1');
    if (csrf !== null) data.set('csrf_token', csrf);
    const response = await fetch(form.action, {method:'POST', credentials:'same-origin',
      headers:{'Content-Type':'application/x-www-form-urlencoded'}, body:data});
    return response.status;
  }, {selector:flowForm,csrf});
}
async function assertReport(page, days) {
  const day = sql("SELECT (now() AT TIME ZONE 'UTC')::date::text");
  for (const [key, offset] of [['current',0],['previous',days]]) {
    const section = page.getByTestId('flow-visits-'+key);
    assert.equal(await section.count(), 1);
    const start = `('${day}'::date+1-${days + offset})`;
    const until = `('${day}'::date+1-${offset})`;
    const values = JSON.parse(sql(`WITH totals AS (
      SELECT visits,pageviews,observed_seconds FROM flow_visit_daily
        WHERE cohort_day>=${start} AND cohort_day<${until}
      UNION ALL
      SELECT 1,pageviews,floor(extract(epoch FROM(last_at-started_at)))::bigint
        FROM flow_visits WHERE finalized_at IS NULL
        AND (started_at AT TIME ZONE 'UTC')::date>=${start}
        AND (started_at AT TIME ZONE 'UTC')::date<${until}
    ) SELECT json_build_object('visits',sum(visits),'pageviews',sum(pageviews),
      'observed-seconds',sum(observed_seconds)) FROM totals`));
    for (const metric of ['visits','pageviews','observed-seconds']) {
      const value = (await section.locator(`dd[data-metric='${metric}']`).innerText()).trim();
      assert.equal(value, values[metric] === null ? 'Нет измерения' : String(values[metric]), key+' '+metric);
    }
    const distinct = await section.locator("dd[data-metric='unique-subjects']").innerText();
    if (days + offset > 30) {
      assert.equal(distinct.trim(), 'Нет измерения');
    } else {
      const exact = scalar(`SELECT count(DISTINCT subject_id) FROM flow_visit_account_days
        WHERE bucket_date>=${start} AND bucket_date<${until}`);
      const measured = sql(`SELECT EXISTS(SELECT 1 FROM flow_visit_account_days
          WHERE bucket_date>=${start} AND bucket_date<${until})
        OR EXISTS(SELECT 1 FROM flow_visit_daily WHERE cohort_day>=${start} AND cohort_day<${until})
        OR EXISTS(SELECT 1 FROM flow_visits WHERE (started_at AT TIME ZONE 'UTC')::date>=${start}
          AND (started_at AT TIME ZONE 'UTC')::date<${until})`) === 't';
      // A retained measured zero is distinct from no measurement at all.
      assert.equal(distinct.trim(), measured ? String(exact) : 'Нет измерения');
    }
  }
}

(async () => {
  const run = Date.now()+'-'+randomBytes(4).toString('hex');
  const readerEmail = `flow-visits-reader-${run}@tor-ci.example`;
  const users = [];
  let browser, book, chapter;
  try {
    book = scalar(`INSERT INTO books(source_identity,target_language,title,visibility,access_level)
      VALUES('ci:flow:visits:browser:${run}','ru','FLOW-VISITS-PRIVATE-${run}','public','basic') RETURNING id`);
    chapter = scalar(`INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
      VALUES(${book},1,'Visit fixture','public','free',0) RETURNING id`);
    const version = scalar(`INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
      VALUES(${chapter},1,'FLOW readable fixture') RETURNING id`);
    sql(`INSERT INTO chapter_publications(chapter_id,version_id) VALUES(${chapter},${version})`);
    browser = await chromium.launch({headless:true});
    const errors = [];
    const adminContext = await browser.newContext({viewport:{width:1440,height:900},userAgent:ordinaryUA});
    const readerContext = await browser.newContext({viewport:{width:390,height:844},userAgent:ordinaryUA});
    const adminPage = await adminContext.newPage();
    const readerPage = await readerContext.newPage();
    for (const page of [adminPage,readerPage]) page.on('pageerror', error => errors.push(error.message));
    const admin = await register(adminPage, `flow-visits-admin-${run}@tor-ci.example`);
    users.push(admin);
    const reader = await register(readerPage, readerEmail);
    users.push(reader);
    sql(`INSERT INTO account_roles(user_id,role) VALUES(${admin},'administrator')`);
    await readerPage.goto(base+'/me/recommendations');
    assert.equal(await readerPage.locator(recForm).getByRole('checkbox').isChecked(), false);
    assert.equal(await readerPage.locator(flowForm).getByRole('checkbox').isChecked(), false);
    assert.equal(await invalidConsent(readerPage, null), 403, 'FLOW requires separate REC prerequisite');
    await choose(readerPage, recForm, true);
    assert.equal(await readerPage.locator(flowForm).getByRole('checkbox').isChecked(), false,
      'earlier REC consent never grants FLOW collection');
    assert.equal((await readerPage.goto(base+`/books/${book}`)).status(), 200);
    assert.equal(ownPages(reader), 0);
    await readerPage.goto(base+'/me/recommendations');
    assert.equal(await invalidConsent(readerPage, 'invalid'), 403);
    assert.equal(sql(`SELECT flow_epoch IS NULL FROM rec_subjects WHERE user_id=${reader}`), 't');
    await choose(readerPage, flowForm, true);
    const scope = JSON.parse(sql(`SELECT json_build_object('subject',subject_id,'epoch',flow_epoch)
      FROM rec_subjects WHERE user_id=${reader}`));
    assert.equal((await readerPage.goto(base+`/books/${book}`)).status(), 200);
    await waitPages(reader, 1);
    assert.equal((await readerPage.goto(base+'/catalog?sort=newest')).status(), 200);
    await waitPages(reader, 2);
    await readerPage.locator(`a.book-mini-card__link[href='/books/${book}']`).first().click();
    await readerPage.waitForURL(base+`/books/${book}`);
    await waitPages(reader, 3);
    assert.equal((await readerPage.goto(base+`/read/${chapter}`)).status(), 200);
    await waitPages(reader, 4);
    assert.equal((await readerPage.goto(base+'/me/library')).status(), 200);
    await waitPages(reader, 5);
    assert.equal(sql(`SELECT count(*),min(landing_surface),min(last_surface) FROM flow_visits
      WHERE subject_id='${scope.subject}'::uuid`), '1|2|4');
    // A second browser login is the same opted-in account, not a second human.
    const secondContext = await browser.newContext({userAgent:ordinaryUA});
    const secondPage = await secondContext.newPage();
    await login(secondPage, readerEmail);
    assert.equal((await secondPage.goto(base+`/books/${book}`)).status(), 200);
    await waitPages(reader, 6);
    assert.equal(scalar(`SELECT count(*) FROM flow_visits WHERE subject_id='${scope.subject}'::uuid`), 1);
    await secondContext.close();
    const before = scalar('SELECT count(*) FROM flow_visit_receipts');
    const guestContext = await browser.newContext({userAgent:ordinaryUA});
    const guestPage = await guestContext.newPage();
    assert.equal((await guestPage.goto(base+`/books/${book}`)).status(), 200);
    await privateResponse(await guestPage.goto(base+'/admin/audience/visits'), 401);
    await guestContext.close();
    const botContext = await browser.newContext({storageState:await readerContext.storageState(), userAgent:'Googlebot/2.1'});
    const botPage = await botContext.newPage();
    assert.equal((await botPage.goto(base+`/books/${book}`)).status(), 200);
    await botContext.close();
    await readerPage.evaluate(async () => {
      for (const path of ['/api/v1/books?limit=1','/assets/site.css']) {
        const response = await fetch(path, {credentials:'same-origin'});
        if (!response.ok) throw new Error('negative capture fixture unavailable');
      }
    });
    assert.equal((await readerPage.goto(base+'/books/9223372036854775807')).status(), 404);
    await privateResponse(await readerPage.goto(base+'/admin/audience/visits'), 403);
    assert.equal((await readerPage.goto(base+`/read/${chapter}`)).status(), 200);
    await waitPages(reader, 7);
    assert.equal(scalar('SELECT count(*) FROM flow_visit_receipts'), before+1);
    for (const days of [1,7,30,90]) {
      await privateResponse(await adminPage.goto(base+`/admin/audience/visits?days=${days}`));
      await assertReport(adminPage, days);
      const html = await adminPage.content();
      for (const forbidden of [readerEmail,scope.subject,scope.epoch,`FLOW-VISITS-PRIVATE-${run}`,'FLOW readable fixture']) {
        assert.equal(html.includes(forbidden), false, 'private per-account/content information leaked');
      }
    }
    for (const width of [390,1440]) {
      await adminPage.setViewportSize({width,height:900});
      await adminPage.goto(base+'/admin/audience/visits?days=7');
      assert.ok(await adminPage.getByRole('heading',{name:'Визиты и входы',exact:true}).isVisible());
      assert.equal(await adminPage.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth+1), true);
      await adminPage.screenshot({path:path.join(shots,`flow-visits-${width}.png`),fullPage:true});
    }
    const beforeReports = ownPages(reader);
    assert.equal(beforeReports, 7, 'admin report GETs are read-only and excluded from capture');
    await choose(readerPage, flowForm, false);
    assert.equal(await readerPage.locator(recForm).getByRole('checkbox').isChecked(), true);
    for (const table of ['flow_visits','flow_visit_receipts','flow_visit_account_days']) {
      assert.equal(scalar(`SELECT count(*) FROM ${table} WHERE subject_id='${scope.subject}'::uuid`), 0);
    }
    await choose(readerPage, flowForm, true);
    assert.notEqual(sql(`SELECT flow_epoch FROM rec_subjects WHERE user_id=${reader}`), scope.epoch);
    await readerPage.goto(base+`/books/${book}`);
    await waitPages(reader, 1);
    await choose(readerPage, recForm, false);
    assert.equal(await readerPage.locator(flowForm).getByRole('checkbox').isChecked(), false);
    assert.equal(ownPages(reader), 0);
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${admin} AND role='administrator'`);
    await privateResponse(await adminPage.goto(base+'/admin/audience/visits'), 403);
    assert.equal(await adminPage.locator('[data-metric]').count(), 0);
    assert.deepEqual(errors, []);
    console.log('FLOW visits browser PASS: explicit scope, 7 actual pages, one account visit across browsers, all periods, private report, erasure');
  } finally {
    if (browser) await browser.close();
    if (users.length) {
      sql(`DELETE FROM account_roles WHERE user_id IN(${users.join(',')})`);
      sql(`DELETE FROM users WHERE id IN(${users.join(',')})`);
    }
    if (book) {
      sql(`DELETE FROM reader_book_positions WHERE book_id=${book};
        DELETE FROM reader_chapter_starts WHERE book_id=${book};
        DELETE FROM reader_chapter_completions WHERE book_id=${book};
        DELETE FROM chapter_publications WHERE chapter_id IN(SELECT id FROM chapters WHERE book_id=${book});
        DELETE FROM book_members WHERE book_id=${book}; DELETE FROM books WHERE id=${book}`);
    }
  }
})().catch(error => { console.error(error); process.exitCode=1; });
