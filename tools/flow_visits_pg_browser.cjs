'use strict';
// FLOW-02 retirement: native navigation stays usable, historical reports stay labeled.
// Current migration-193 HTML creates no visits or behavioral identities.
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
const {assertCurrentSchema,optionalSnapshot,assertNoControls,retiredConsent} =
  require('./collection_retirement_fixtures.cjs')(sql);
const preview = require('./ci_browser_preview.cjs');

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
function ownPages(user) {
  return scalar(`SELECT COALESCE(sum(v.pageviews),0) FROM flow_visits v
    JOIN rec_subjects s ON s.subject_id=v.subject_id WHERE s.user_id=${user}`);
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
async function privateResponse(response, status=200) {
  assert.equal(response.status(), status);
  const headers = await response.allHeaders();
  assert.equal(headers['cache-control'], 'no-store');
  assert.match(headers['x-robots-tag'], /noindex/);
  assert.match(headers['x-robots-tag'], /nofollow/);
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
          WHERE bucket_date>=${start} AND bucket_date<${until})`) === 't';
      // An anonymous daily total does not reconstruct erased account-day evidence.
      // Only retained source rows can prove an exact DISTINCT for this period.
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
    assertCurrentSchema();
    book = scalar(`INSERT INTO books(source_identity,target_language,title,visibility,access_level)
      VALUES('ci:flow:visits:browser:${run}','ru','FLOW-VISITS-PRIVATE-${run}','public','basic') RETURNING id`);
    chapter = scalar(`INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
      VALUES(${book},1,'Visit fixture','public','free',0) RETURNING id`);
    const version = scalar(`INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
      VALUES(${chapter},1,'FLOW readable fixture') RETURNING id`);
    sql(`INSERT INTO chapter_publications(chapter_id,version_id) VALUES(${chapter},${version})`);
    browser = await chromium.launch({headless:true});
    const errors = [],unexpectedAnalytics = [];
    const adminContext = await browser.newContext({viewport:{width:1440,height:900},userAgent:ordinaryUA});
    const readerContext = await browser.newContext({viewport:{width:390,height:844},userAgent:ordinaryUA});
    const adminPage = await adminContext.newPage(), readerPage = await readerContext.newPage();
    for (const page of [adminPage,readerPage]) {
      page.on('pageerror', error => errors.push(error.message));
      page.on('request', request => {
        if (/\/api\/v1\/(?:recommendations\/(?:impressions|activate)|discovery\/activate)$/.test(new URL(request.url()).pathname)) {
          unexpectedAnalytics.push(new URL(request.url()).pathname);
        }
      });
    }
    const admin = await register(adminPage, `flow-visits-admin-${run}@tor-ci.example`);
    const reader = await register(readerPage, readerEmail);
    users.push(admin,reader);
    sql(`INSERT INTO account_roles(user_id,role) VALUES(${admin},'administrator')`);
    await readerPage.goto(base+'/me/recommendations');
    await assertNoControls(readerPage);
    assert.equal(scalar(`SELECT count(*) FROM rec_subjects WHERE user_id=${reader}`),0);
    for (const endpoint of ['/me/recommendations/consent','/me/recommendations/flow-consent']) {
      await retiredConsent(readerPage,endpoint,true);
    }
    assert.equal(scalar(`SELECT count(*) FROM rec_subjects WHERE user_id=${reader}`),0,
      'retired requests cannot introduce an analytics subject');

    const before = optionalSnapshot(reader);
    assert.equal(ownPages(reader),0);
    for (const target of [`/books/${book}`,'/catalog?sort=newest',`/reader/${chapter}`,
      `/read/${chapter}`,'/me/library','/']) {
      assert.equal((await readerPage.goto(base+target)).status(),200,target);
      await assertNoControls(readerPage);
      assert.deepEqual(optionalSnapshot(reader),before,target);
    }
    // The ordinary server has no private paid-reader adapter. Its positive
    // purchased-library route is covered with a real checkout in paid HTTP tests.
    assert.equal((await readerPage.goto(base+'/me/purchases')).status(),404);
    assert.deepEqual(optionalSnapshot(reader),before);
    await readerPage.goto(base+'/catalog?sort=newest');
    await readerPage.locator(`a.book-mini-card__link[href='/books/${book}']`).first().click();
    await readerPage.waitForURL(base+`/books/${book}`);
    const secondContext = await browser.newContext({userAgent:ordinaryUA});
    const secondPage = await secondContext.newPage();
    await login(secondPage,readerEmail);
    assert.equal((await secondPage.goto(base+`/books/${book}`)).status(),200);
    await assertNoControls(secondPage);
    await secondContext.close();
    const guestContext = await browser.newContext({userAgent:ordinaryUA});
    const guestPage = await guestContext.newPage();
    assert.equal((await guestPage.goto(base+`/books/${book}`)).status(),200);
    await assertNoControls(guestPage);
    await privateResponse(await guestPage.goto(base+'/admin/audience/visits'),401);
    await guestContext.close();
    const botContext = await browser.newContext({storageState:await readerContext.storageState(),userAgent:'Googlebot/2.1'});
    const botPage = await botContext.newPage();
    assert.equal((await botPage.goto(base+`/books/${book}`)).status(),200);
    await botContext.close();
    // These explicit HTTP reads share the real browser's session. Public book
    // HTML retains its strict CSP; analytics no longer relaxes it for fetch().
    for (const target of ['/api/v1/books?limit=1','/assets/site.css']) {
      assert.equal((await readerPage.request.get(base+target)).status(),200,target);
    }
    assert.equal((await readerPage.goto(base+'/books/9223372036854775807')).status(),404);
    await privateResponse(await readerPage.goto(base+'/admin/audience/visits'),403);
    assert.deepEqual(optionalSnapshot(reader),before);
    for (const days of [1,7,30,90]) {
      await privateResponse(await adminPage.goto(base+`/admin/audience/visits?days=${days}`));
      assert.equal(await adminPage.getByTestId('collection-retired').count(),1);
      assert.equal(await adminPage.locator('[data-current-measurement="unavailable"]').count(),1);
      assert.equal(await adminPage.locator('[data-measurement="legacy-consented"]').count(),2);
      await assertReport(adminPage,days);
      const html = await adminPage.content();
      for (const forbidden of [readerEmail,`FLOW-VISITS-PRIVATE-${run}`,'FLOW readable fixture']) {
        assert.equal(html.includes(forbidden),false,'private identity/content leaked');
      }
    }
    for (const width of [390,1440]) {
      await adminPage.setViewportSize({width,height:900});
      await adminPage.goto(base+'/admin/audience/visits?days=7');
      assert.ok(await adminPage.getByRole('heading',{name:'Визиты и входы',exact:true}).isVisible());
      assert.equal(await adminPage.evaluate(() => document.documentElement.scrollWidth <= innerWidth+1),true);
      await adminPage.screenshot({path:path.join(shots,`flow-visits-${width}.png`),fullPage:true});
      await preview(adminPage,`flow-visits-${width}`);
    }
    await readerPage.goto(base+'/me/recommendations');
    for (const endpoint of ['/me/recommendations/consent','/me/recommendations/flow-consent']) {
      await retiredConsent(readerPage,endpoint,false);
      await retiredConsent(readerPage,endpoint,true);
    }
    assert.deepEqual(optionalSnapshot(reader),before,
      'all HTML, reports, browser restarts and old client commands leave optional stores empty');
    for (const cookie of await readerContext.cookies()) {
      assert.ok(['tor-dev-session','tor-dev-csrf'].includes(cookie.name),'unexpected cookie '+cookie.name);
    }
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${admin} AND role='administrator'`);
    await privateResponse(await adminPage.goto(base+'/admin/audience/visits'),403);
    assert.equal(await adminPage.locator('[data-metric]').count(),0);
    assert.deepEqual(unexpectedAnalytics,[]);
    assert.deepEqual(errors,[]);
    console.log('FLOW visits browser PASS: current193 barrier, no analytics identity or visits, native navigation across browsers, retired consent410, all periods/ACL/privacy, 390/1440');
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
