'use strict';
// FLOW-03: actual browser/C actions and supervised cohort worker, no fake positives.
// Positive visit observations are never inserted by these catalogue fixtures.
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
  for (let attempt=0; attempt<150; attempt++) {
    if (await check()) return;
    await new Promise(resolve => setTimeout(resolve, 100));
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
async function navigate(page, item, source) {
  const target=source==='recommendations' ? '/' : source==='search'
    ? '/catalog?q='+encodeURIComponent(item.title) : '/catalog?sort=newest';
  assert.equal((await page.goto(base+target)).status(),200);
  let link,ticket;
  if (source==='recommendations') {
    const card=page.locator('[data-rec-card]').filter({
      has:page.locator(`a[data-rec-link][href='/books/${item.book}']`),
    }).first();
    await card.scrollIntoViewIfNeeded();
    ticket=await card.getAttribute('data-rec-ticket');
    assert.ok(ticket);
    const digest=createHash('sha256').update(ticket).digest('hex');
    await waitFor(() => scalar(`SELECT count(*) FROM rec_assignments
      WHERE ticket_digest=decode('${digest}','hex') AND viewed_at IS NOT NULL`)===1,
    'the actual recommendation impression');
    link=card.locator('a[data-rec-link]');
  } else {
    link=page.locator(`a.book-mini-card__link[href='/books/${item.book}'][data-organic-ticket]`).first();
    assert.ok(await link.isVisible());
    ticket=await link.getAttribute('data-organic-ticket');
  }
  await link.click();
  await page.waitForURL(base+`/books/${item.book}`);
  const digest=createHash('sha256').update(ticket).digest('hex');
  const table=source==='recommendations' ? 'rec_assignments' : 'organic_discovery_touches';
  await waitFor(() => scalar(`SELECT count(*) FROM ${table} WHERE ticket_digest=decode('${digest}','hex')
    AND clicked_at IS NOT NULL AND book_opened_at IS NOT NULL`)===1,
  'real '+source+' POST and authorized book open');
}
async function read(page, item) {
  assert.equal((await page.goto(base+`/read/${item.chapter}`)).status(),200);
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
function ageFacts(user) {
  // Move only real accepted fixture evidence through the production grace.
  // No fixture creates a read, action, visit, or cohort; IDs/payloads stay intact.
  const scope=`subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id=${user})`;
  sql(`BEGIN;
    UPDATE rec_subjects SET flow_enabled_at=flow_enabled_at-interval '6 minutes' WHERE user_id=${user};
    UPDATE flow_visit_receipts SET observed_at=observed_at-interval '6 minutes' WHERE ${scope};
    UPDATE flow_visits SET started_at=started_at-interval '6 minutes',last_at=last_at-interval '6 minutes',
      finalized_at=finalized_at-interval '6 minutes' WHERE ${scope};
    UPDATE rec_assignments SET created_at=created_at-interval '6 minutes',expires_at=expires_at-interval '6 minutes',
      clicked_at=clicked_at-interval '6 minutes',book_opened_at=book_opened_at-interval '6 minutes',
      viewed_at=viewed_at-interval '6 minutes' WHERE ${scope};
    UPDATE organic_discovery_touches SET created_at=created_at-interval '6 minutes',expires_at=expires_at-interval '6 minutes',
      clicked_at=clicked_at-interval '6 minutes',book_opened_at=book_opened_at-interval '6 minutes' WHERE ${scope};
    UPDATE reader_chapter_starts SET started_at=started_at-interval '6 minutes' WHERE user_id=${user};
    UPDATE reader_chapter_completions SET completed_at=completed_at-interval '6 minutes' WHERE user_id=${user};
    UPDATE reader_flow_book_actions SET occurred_at=occurred_at-interval '6 minutes' WHERE ${scope};
    UPDATE reader_flow_read_receipts SET occurred_at=occurred_at-interval '6 minutes' WHERE ${scope};
    COMMIT;`);
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
  const oldHold=sql('SELECT hold::text FROM rec_runtime_policy WHERE singleton');
  let browser;
  try {
    sql('UPDATE rec_runtime_policy SET hold=true WHERE singleton');
    const catalog=fixture('catalog',run),search=fixture('search',run);
    const rec=fixture('rec',run),unknown=fixture('unknown',run);
    books.push(catalog.book,search.book,rec.book,unknown.book);
    browser=await chromium.launch({headless:true});
    const adminContext=await browser.newContext({viewport:{width:1440,height:900},userAgent:ordinaryUA});
    const readerContext=await browser.newContext({viewport:{width:390,height:844},userAgent:ordinaryUA});
    const adminPage=await adminContext.newPage(),readerPage=await readerContext.newPage();
    const errors=[];
    for (const page of [adminPage,readerPage]) page.on('pageerror',error=>errors.push(error.message));
    const adminEmail=`flow-funnel-admin-${run}@tor-ci.example`;
    const readerEmail=`flow-funnel-reader-${run}@tor-ci.example`;
    const admin=await register(adminPage,adminEmail),reader=await register(readerPage,readerEmail);
    users.push(admin,reader);
    sql(`INSERT INTO account_roles(user_id,role) VALUES(${admin},'administrator')`);
    await adminContext.clearCookies();
    await login(adminPage,adminEmail);
    await choose(readerPage,recForm,true);
    assert.equal(await readerPage.locator(flowForm).getByRole('checkbox').isChecked(),false);
    await choose(readerPage,flowForm,true);
    const scope=JSON.parse(sql(`SELECT json_build_object('subject',subject_id,'epoch',flow_epoch)
      FROM rec_subjects WHERE user_id=${reader}`));
    await privateResponse(await readerPage.goto(base+'/admin/audience/funnel'),403);

    await navigate(readerPage,catalog,'catalog');
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
    await waitFor(() => scalar(`SELECT count(DISTINCT book_id) FROM flow_visit_receipts
      WHERE subject_id='${scope.subject}'::uuid AND book_id IS NOT NULL`)===4,'all admitted book contexts');
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_starts WHERE user_id=${reader}`),2);
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_completions WHERE user_id=${reader}`),2);
    assert.equal(scalar(`SELECT count(*) FROM reader_flow_read_receipts WHERE subject_id='${scope.subject}'::uuid`),4);
    assert.equal(scalar(`SELECT count(*) FROM reader_flow_book_actions WHERE subject_id='${scope.subject}'::uuid`),2);
    assert.equal(scalar(`SELECT count(*) FROM flow_book_cohorts WHERE subject_id='${scope.subject}'::uuid`),0,
      'the production grace is not bypassed');
    await waitFor(() => scalar(`SELECT count(*) FROM flow_visit_receipts
      WHERE subject_id='${scope.subject}'::uuid AND book_id=${unknown.book} AND book_kind=1`)>=1,
    'the final admitted reader page');
    // The positive last request is an async queue barrier before clock fixtures.
    const beforePages=ownPages(reader);
    assert.equal((await readerPage.goto(base+`/reader/${unknown.chapter}`)).status(),200);
    await waitPages(reader,beforePages+1);
    ageFacts(reader);
    await waitFor(() => scalar(`SELECT count(*) FROM flow_book_cohorts
      WHERE subject_id='${scope.subject}'::uuid AND (book_id NOT IN(${catalog.book},${rec.book})
      OR (started AND completed))`)===4,'actual supervised FLOW worker and C delivery');
    const own=sql(`SELECT count(*),sum(has_detail::int),sum(has_reader::int),sum(started::int),sum(completed::int),
      sum(saved::int),sum(followed::int),sum(new_reader::int),sum((d7_state=1)::int)
      FROM flow_book_cohorts WHERE subject_id='${scope.subject}'::uuid`);
    assert.equal(own,'4|4|3|2|2|1|1|2|2');
    for (const [item,kind] of [[catalog,1],[search,2],[rec,0],[unknown,3]]) {
      assert.equal(sql(`SELECT discovery_kind FROM flow_book_cohorts
        WHERE subject_id='${scope.subject}'::uuid AND book_id=${item.book}`),String(kind));
    }
    for (const days of [1,7,30,90]) {
      await privateResponse(await adminPage.goto(base+`/admin/audience/funnel?days=${days}`));
      await assertReport(adminPage,days);
      const html=await adminPage.content();
      for (const secret of [readerEmail,scope.subject,scope.epoch,catalog.title,'abcdefghij']) {
        assert.equal(html.includes(secret),false,'private identity or content leaked');
      }
    }
    for (const width of [390,1440]) {
      await adminPage.setViewportSize({width,height:900});
      await adminPage.goto(base+'/admin/audience/funnel?days=7');
      assert.ok(await adminPage.getByRole('heading',{name:'Путь от визита к чтению',exact:true}).isVisible());
      assert.match(await adminPage.locator('body').innerText(),/поиска или фильтров каталога/i);
      assert.equal(await adminPage.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth+1),true);
      await adminPage.screenshot({path:path.join(shots,`flow-funnel-${width}.png`),fullPage:true});
    }
    await choose(readerPage,flowForm,false);
    for (const table of ['flow_book_cohorts','flow_book_action_receipts','reader_flow_book_actions',
      'reader_flow_read_receipts','reader_flow_activity_days']) {
      assert.equal(scalar(`SELECT count(*) FROM ${table} WHERE subject_id='${scope.subject}'::uuid`),0);
    }
    assert.equal(scalar(`SELECT count(*) FROM reader_library_items WHERE user_id=${reader}`),1);
    assert.equal(scalar(`SELECT count(*) FROM reader_book_follows WHERE user_id=${reader}`),1);
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${admin} AND role='administrator'`);
    await privateResponse(await adminPage.goto(base+'/admin/audience/funnel'),403);
    assert.equal(await adminPage.locator('[data-metric]').count(),0);
    assert.deepEqual(errors,[]);
    console.log('FLOW funnel browser PASS: actual REC/catalog/search/direct navigation, C start/completion, saved/followed book, scoped worker cohorts, all periods, private admin, erasure, 390/1440');
  } finally {
    if (browser) await browser.close();
    sql(`UPDATE rec_runtime_policy SET hold=${oldHold} WHERE singleton`);
    if (users.length) sql(`DELETE FROM rec_subjects WHERE user_id IN(${users.join(',')});
      DELETE FROM account_roles WHERE user_id IN(${users.join(',')})`);
    if (books.length) sql(`UPDATE books SET admin_hold=true WHERE id IN(${books.join(',')})`);
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
