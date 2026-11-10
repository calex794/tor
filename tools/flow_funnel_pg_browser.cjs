'use strict';
// FLOW-03 retirement: actual C reading/save/follow remains, individual linking stops.
// Historical report rows are explicit old-installation fixtures, separate from C commands.
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
const {legacyScope,optionalSnapshot,assertNoControls,retiredConsent,historicalFunnel} =
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
async function register(page, email) {
  await page.goto(base+'/register');
  await page.getByLabel('Отображаемое имя').fill('Проверка пути к чтению');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('FlowFunnelBrowserPassword2026');
  await page.getByRole('button', {name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
  return scalar(`SELECT id FROM users WHERE email_normalized='${email}'`);
}
async function login(page, email) {
  await page.goto(base+'/login');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль').fill('FlowFunnelBrowserPassword2026');
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

async function csrf(page) {
  const cookie = (await page.context().cookies()).find(item => item.name === 'tor-dev-csrf');
  assert.ok(cookie);
  return cookie.value;
}
function fixture(name, run) {
  const title=`FLOW-FUNNEL-${name}-${run}`;
  const book=scalar(`INSERT INTO books(source_identity,target_language,title,visibility,access_level)
    VALUES('ci:flow-funnel:${name}:${run}','ru','${title}','public','basic') RETURNING id`);
  const chapter=scalar(`INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
    VALUES(${book},1,'FLOW funnel chapter','public','free') RETURNING id`);
  const version=scalar(`INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
    VALUES(${chapter},1,'abcdefghij') RETURNING id`);
  sql(`INSERT INTO chapter_publications(chapter_id,version_id) VALUES(${chapter},${version})`);
  return {book,chapter,version,title};
}
async function navigate(page,item,source) {
  const target=source==='recommendations' ? '/' : source==='search'
    ? '/catalog?q='+encodeURIComponent(item.title) : '/catalog?sort=newest';
  assert.equal((await page.goto(base+target)).status(),200);
  await assertNoControls(page);
  const link=source==='recommendations'
    ? page.locator(`a[data-rec-link][href='/books/${item.book}']`).first()
    : page.locator(`a.book-mini-card__link[href='/books/${item.book}']`).first();
  await link.scrollIntoViewIfNeeded();
  assert.ok(await link.isVisible());
  await link.click();
  await page.waitForURL(base+`/books/${item.book}`);
  await assertNoControls(page);
}
async function read(page, item) {
  // The manual reader uses native POST forms and keeps connect-src closed.
  // Exercise the functional AJAX progress API from its actual public reader.
  assert.equal((await page.goto(base+`/read/${item.chapter}`)).status(),200);
  assert.equal((await page.goto(base+`/reader/${item.chapter}`)).status(),200);
  await page.waitForFunction(() =>
    document.querySelector('[data-free-progress-chapter]')?.dataset.freeProgressReady==='1');
  await page.evaluate(async ({chapter,version,csrf}) => {
    const state=await fetch('/api/v1/me/progress/free?chapter_id='+chapter,{credentials:'same-origin'});
    if (!state.ok) throw new Error('progress state rejected '+state.status);
    let revision=(await state.json()).revision;
    for (const [offset,expected] of [[2,200],[10,200],[10,409]]) {
      const response=await fetch('/api/v1/me/progress/free',{
        method:'PUT',credentials:'same-origin',
        headers:{'Content-Type':'application/json','X-CSRF-Token':csrf},
        body:JSON.stringify({chapter_id:String(chapter),version_id:String(version),
          block_id:1,char_offset:offset,expected_revision:revision}),
      });
      if (response.status!==expected) throw new Error(`C save ${response.status}, expected ${expected}`);
      if (expected===200) revision=(await response.json()).revision;
    }
  },{...item,csrf:await csrf(page)});
}
const metricNames=['cohorts','detail-opens','reader-opens','starts','completions','saves','follows',
  'new-readers','d7-anchors','d7-pending','d7-returned','d7-unknown'];
function expectedTotals(from,until) {
  // Independent arithmetic over retained cohorts and anonymized daily facts.
  // No repository report call or public endpoint supplies these expectations.
  const fields=['cohorts','detail_opens','reader_opens','starts','completions','saves','follows',
    'new_readers','d7_anchors','d7_pending','d7_returned','d7_unknown'];
  const raw=['1','has_detail::int','has_reader::int','started::int','completed::int','saved::int',
    'followed::int','new_reader::int','(d7_state>0)::int','(d7_state=1)::int','(d7_state=2)::int','(d7_state=3)::int'];
  return JSON.parse(sql(`WITH samples AS (
    SELECT ${raw.map((value,i)=>value+' AS '+fields[i]).join(',')}
      FROM flow_book_cohorts WHERE finalized_at IS NULL
      AND (first_entry_at AT TIME ZONE 'UTC')::date>=${from}
      AND (first_entry_at AT TIME ZONE 'UTC')::date<${until}
    UNION ALL SELECT ${fields.map(field=>field==='d7_pending'?'0':field).join(',')}
      FROM flow_book_daily WHERE cohort_day>=${from} AND cohort_day<${until}
  ) SELECT json_build_array(${fields.map(field=>'sum('+field+')').join(',')}) FROM samples`));
}
async function assertReport(page,days) {
  const today=sql("SELECT (now() AT TIME ZONE 'UTC')::date::text");
  for (const [name,offset] of [['current',0],['previous',days]]) {
    const section=page.getByTestId('flow-funnel-'+name);
    const from=`('${today}'::date+1-${days+offset})`,until=`('${today}'::date+1-${offset})`;
    const expected=expectedTotals(from,until);
    if (expected[0]===null) {
      assert.match(await section.innerText(),/Нет сохранённых посещений книг/);
      assert.equal(await section.locator('dd[data-metric=cohorts]').count(),0);
      continue;
    }
    for (const [index,metric] of metricNames.entries()) {
      const value=(await section.locator(`:scope > dl > dd[data-metric='${metric}']`).innerText()).trim();
      assert.equal(value,String(expected[index]),name+' '+metric);
    }
  }
}

(async () => {
  const run=Date.now()+'-'+randomBytes(4).toString('hex');
  const users=[],books=[];
  let browser;
  try {
    const catalog=fixture('catalog',run),search=fixture('search',run);
    const rec=fixture('rec',run),unknown=fixture('unknown',run);
    books.push(catalog.book,search.book,rec.book,unknown.book);
    browser=await chromium.launch({headless:true});
    const adminContext=await browser.newContext({viewport:{width:1440,height:900},userAgent:ordinaryUA});
    const readerContext=await browser.newContext({viewport:{width:390,height:844},userAgent:ordinaryUA});
    const adminPage=await adminContext.newPage(),readerPage=await readerContext.newPage();
    const errors=[],unexpectedAnalytics=[];
    for (const page of [adminPage,readerPage]) {
      page.on('pageerror',error=>errors.push(error.message));
      page.on('request',request=>{
        if (/\/api\/v1\/(?:recommendations\/(?:impressions|activate)|discovery\/activate)$/.test(new URL(request.url()).pathname)) {
          unexpectedAnalytics.push(new URL(request.url()).pathname);
        }
      });
    }
    const adminEmail=`flow-funnel-admin-${run}@tor-ci.example`;
    const readerEmail=`flow-funnel-reader-${run}@tor-ci.example`;
    const admin=await register(adminPage,adminEmail),reader=await register(readerPage,readerEmail);
    users.push(admin,reader);
    await assertNoControls(readerPage);
    assert.equal(scalar(`SELECT count(*) FROM rec_subjects WHERE user_id=${reader}`),0);
    sql(`INSERT INTO account_roles(user_id,role) VALUES(${admin},'administrator')`);
    await adminContext.clearCookies();
    await login(adminPage,adminEmail);

    // Explicit legacy scope and anonymous mature totals, before real navigation.
    const scope=legacyScope(reader);
    historicalFunnel(catalog.book);
    const before=optionalSnapshot(reader);
    const historyQuery=`SELECT jsonb_agg(to_jsonb(d) ORDER BY cohort_day)
      FROM flow_book_daily d WHERE book_id=${catalog.book}`;
    const historical=sql(historyQuery);
    await privateResponse(await readerPage.goto(base+'/admin/audience/funnel'),403);
    await navigate(readerPage,catalog,'catalog');
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_starts WHERE user_id=${reader}`),0,
      'GET and native link do not manufacture reading');
    await readerPage.getByRole('button',{name:'Следить за книгой',exact:true}).click();
    await readerPage.waitForURL(base+`/books/${catalog.book}`);
    await readerPage.getByRole('link',{name:'Добавить в библиотеку',exact:true}).click();
    await readerPage.getByRole('button',{name:'Сохранить книгу',exact:true}).click();
    await readerPage.waitForURL('**/me/library');
    await read(readerPage,catalog);
    await navigate(readerPage,search,'search');
    await navigate(readerPage,rec,'recommendations');
    await read(readerPage,rec);
    assert.equal((await readerPage.goto(base+`/books/${unknown.book}`)).status(),200);
    assert.equal((await readerPage.goto(base+`/reader/${unknown.chapter}`)).status(),200);
    assert.equal((await readerPage.goto(base+`/me/library/saved/${catalog.chapter}`)).status(),200);
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_starts WHERE user_id=${reader}`),2);
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_completions WHERE user_id=${reader}`),2);
    assert.equal(scalar(`SELECT count(*) FROM reader_book_positions WHERE user_id=${reader}`),2);
    assert.equal(scalar(`SELECT count(*) FROM reader_library_items WHERE user_id=${reader}`),1);
    assert.equal(scalar(`SELECT count(*) FROM reader_book_follows WHERE user_id=${reader}`),1);
    assert.equal(scalar(`SELECT count(*) FROM outbox_events WHERE
      event_type IN('ChapterStartedV1','ChapterCompletedV1') AND payload->>'user_id'='${reader}'`),4);
    assert.deepEqual(optionalSnapshot(reader),before,
      'real accepted C commands must not add optional activity, return, source or FLOW proof');

    for (const days of [1,7,30,90]) {
      await privateResponse(await adminPage.goto(base+`/admin/audience/funnel?days=${days}`));
      assert.equal(await adminPage.getByTestId('collection-retired').count(),1);
      assert.equal(await adminPage.locator('[data-current-measurement="unavailable"]').count(),1);
      assert.equal(await adminPage.locator('[data-measurement="legacy-consented"]').count(),2);
      await assertReport(adminPage,days);
      const html=await adminPage.content();
      for (const secret of [readerEmail,scope.subject,scope.epoch,catalog.title,'abcdefghij']) {
        assert.equal(html.includes(secret),false,'private identity/content leaked');
      }
    }
    for (const width of [390,1440]) {
      await adminPage.setViewportSize({width,height:900});
      await adminPage.goto(base+'/admin/audience/funnel?days=90');
      assert.ok(await adminPage.getByRole('heading',{name:'Путь от визита к чтению',exact:true}).isVisible());
      assert.ok(await adminPage.locator('[data-discovery="2"]').count()>0);
      assert.equal(await adminPage.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),true);
      await adminPage.screenshot({path:path.join(shots,`flow-funnel-${width}.png`),fullPage:true});
      await preview(adminPage,`flow-funnel-${width}`);
    }
    await readerPage.goto(base+'/me/recommendations');
    await assertNoControls(readerPage);
    for (const endpoint of ['/me/recommendations/consent','/me/recommendations/flow-consent']) {
      await retiredConsent(readerPage,endpoint,false);
      await retiredConsent(readerPage,endpoint,true);
    }
    assert.deepEqual(optionalSnapshot(reader),before);
    assert.equal(sql(historyQuery),historical,'browser activity cannot rewrite historical report totals');
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_completions WHERE user_id=${reader}`),2);
    assert.equal(scalar(`SELECT count(*) FROM reader_library_items WHERE user_id=${reader}`),1);
    assert.equal(scalar(`SELECT count(*) FROM reader_book_follows WHERE user_id=${reader}`),1);
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${admin} AND role='administrator'`);
    await privateResponse(await adminPage.goto(base+'/admin/audience/funnel'),403);
    assert.equal(await adminPage.locator('[data-metric]').count(),0);
    assert.deepEqual(unexpectedAnalytics,[]);
    assert.deepEqual(errors,[]);
    console.log('FLOW funnel browser PASS: native REC/catalog/search/direct navigation, real C start/completion/save/follow, old scope inert, independent historical totals, retired410, admin ACL/privacy, 390/1440');
  } finally {
    if (browser) await browser.close();
    if (users.length) sql(`DELETE FROM rec_subjects WHERE user_id IN(${users.join(',')});
      DELETE FROM account_roles WHERE user_id IN(${users.join(',')})`);
    if (books.length) sql(`DELETE FROM flow_book_daily WHERE book_id IN(${books.join(',')});
      UPDATE books SET admin_hold=true WHERE id IN(${books.join(',')})`);
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
