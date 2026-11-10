'use strict';
// FLOW: genuine accepted reading today creates a pending D1/D7/D30 cohort.
// Time-dependent positive return counts belong to explicit PG/HTTP fixtures;
// this browser never inserts or backdates a positive analytics fact.
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
  throw new Error('Returns acceptance requires disposable loopback tor_ci');
}
fs.mkdirSync(shots, {recursive:true});

function sql(statement) {
  return execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1',
    '--dbname',db,'--command',statement], {
    encoding:'utf8', env:{...process.env, PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'},
  }).trim();
}
function scalar(statement) {
  const raw = sql(statement);
  assert.match(raw, /^\d+$/, 'numeric database evidence');
  return Number(raw);
}
async function waitFor(check, label) {
  for (let attempt=0; attempt<150; attempt++) {
    if (await check()) return;
    await new Promise(resolve => setTimeout(resolve,200));
  }
  throw new Error('not observed: '+label);
}
async function register(page, email) {
  await page.goto(base+'/register');
  await page.getByLabel('Отображаемое имя').fill('Проверка возвращения');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('AudienceReturnsPassword2026');
  await page.getByRole('button',{name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
  return scalar(`SELECT id FROM users WHERE email_normalized='${email}'`);
}
async function consent(page, enabled) {
  await page.goto(base+'/me/recommendations');
  await page.getByRole('checkbox').setChecked(enabled);
  await page.getByRole('button',{name:'Сохранить выбор'}).click();
  await page.waitForURL('**/me/recommendations');
}
async function csrf(page) {
  const value = (await page.context().cookies()).find(cookie => cookie.name === 'tor-dev-csrf')?.value;
  assert.ok(value,'normal authenticated forms issue the CSRF cookie');
  return value;
}
async function privateResponse(response, status=200) {
  assert.equal(response.status(),status);
  const headers = await response.allHeaders();
  assert.equal(headers['cache-control'],'no-store');
  assert.match(headers['x-robots-tag'],/noindex/);
  assert.match(headers['x-robots-tag'],/nofollow/);
}
function chapter(book, number) {
  const chapter = scalar(`INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
    VALUES(${book},${number},'Return chapter ${number}','public','free',0) RETURNING id`);
  const version = scalar(`INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
    VALUES(${chapter},1,E'Readable first paragraph.\\n\\nSecond paragraph.') RETURNING id`);
  sql(`INSERT INTO chapter_publications(chapter_id,version_id) VALUES(${chapter},${version})`);
  return {book,chapter,version};
}
async function progress(page, fixture, offset, expected=200) {
  return page.evaluate(async ({chapter,version,offset,expected,csrf}) => {
    const state = await fetch('/api/v1/me/progress/free?chapter_id='+chapter, {
      credentials:'same-origin',cache:'no-store',
    });
    if (!state.ok) throw new Error('C position lookup rejected: '+state.status);
    const revision = (await state.json()).revision;
    const response = await fetch('/api/v1/me/progress/free', {
      method:'PUT',credentials:'same-origin',
      headers:{'Content-Type':'application/json','X-CSRF-Token':csrf},
      body:JSON.stringify({chapter_id:String(chapter),version_id:String(version),
        block_id:1,char_offset:offset,expected_revision:revision}),
    });
    if (response.status !== expected) {
      throw new Error(`C progress ${response.status}, expected ${expected}`);
    }
    return response.status;
  },{...fixture,offset,expected,csrf:await csrf(page)});
}
function daysFor(user) {
  return scalar(`SELECT count(*) FROM reader_activity_days
    WHERE subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id=${user})`);
}
function cohortsFor(user) {
  return scalar(`SELECT count(*) FROM reader_return_subjects
    WHERE subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id=${user}) AND cohort_day IS NOT NULL`);
}
async function metric(locator,key) {
  return (await locator.locator(`[data-metric='${key}']`).innerText()).trim();
}
async function report(page,days=90) {
  await privateResponse(await page.goto(base+'/admin/audience/returns?days='+days));
  const result = {};
  for (const period of ['current','previous']) {
    const section = page.getByTestId('audience-returns-'+period);
    const cohort = await metric(section,'cohort');
    assert.match(cohort,/^\d+$/,'observed cohort count is explicit');
    result[period]={cohort:Number(cohort),days:{}};
    for (const day of [1,7,30]) {
      const row = section.locator(`[data-return-day='${day}']`);
      assert.equal(await row.count(),1,'each horizon is rendered once');
      const values = {};
      for (const key of ['eligible','returned','pending','unknown','rate']) {
        const value = await metric(row,key);
        values[key] = /^\d+$/.test(value) ? Number(value) : value;
      }
      result[period].days[day]=values;
    }
  }
  return result;
}
function pendingDelta(before,after,delta) {
  assert.equal(after.current.cohort,before.current.cohort+delta,'one enrolment per consent epoch');
  assert.deepEqual(after.previous,before.previous,'today does not backfill previous cohorts');
  for (const day of [1,7,30]) {
    const a=before.current.days[day], b=after.current.days[day];
    assert.equal(b.pending,a.pending+delta,'a complete target day is still awaited');
    for (const key of ['eligible','returned','unknown','rate']) {
      assert.equal(b[key],a[key],'today cannot change mature '+day+' '+key);
    }
  }
}
async function select(page,days) {
  await page.locator('select[name="days"]').selectOption(String(days));
  await page.getByRole('button',{name:'Показать',exact:true}).click();
  await page.waitForURL(url => url.pathname === '/admin/audience/returns'
    && url.searchParams.get('days') === String(days));
}
async function periods(page,days) {
  const bounds = {};
  for (const period of ['current','previous']) {
    bounds[period]=(await page.getByTestId('audience-returns-'+period).innerText())
      .match(/\b\d{4}-\d{2}-\d{2}\b/g) || [];
    assert.equal(bounds[period].length,2);
    assert.equal(Date.parse(bounds[period][1])-Date.parse(bounds[period][0]),days*86400000);
  }
  assert.equal(bounds.previous[1],bounds.current[0]);
  assert.equal(bounds.current[1],sql("SELECT ((now() AT TIME ZONE 'UTC')::date+1)::text"));
}

(async () => {
  const run=Date.now()+'-'+randomBytes(4).toString('hex');
  const emails=['admin','api-reader','manual-reader'].map(role=>`flow-returns-${role}-${run}@tor-ci.example`);
  const books=[], users=[], errors=[];
  let browser;
  function book(key) {
    const id=scalar(`INSERT INTO books(source_identity,target_language,title,visibility,access_level)
      VALUES('ci:flow:returns:${run}:${key}','ru','RETURN-TITLE-${run}-${key}','public','basic') RETURNING id`);
    books.push(id); return id;
  }
  try {
    const first=chapter(book('first'),1), later=chapter(first.book,2);
    const other=chapter(book('other'),1), manual=chapter(book('manual'),1);
    const noStart=chapter(book('zero'),1);
    browser=await chromium.launch({headless:true});
    const pages=[];
    for (const [index,email] of emails.entries()) {
      const context=await browser.newContext({viewport:{width:index===0?1440:390,height:900}});
      const page=await context.newPage();
      page.on('pageerror',error=>errors.push(error.message));
      users.push(await register(page,email)); pages.push(page);
    }
    const [adminPage,readerPage,manualPage]=pages;
    sql(`INSERT INTO account_roles(user_id,role) VALUES(${users[0]},'administrator')`);
    await waitFor(()=>sql("SELECT EXISTS(SELECT 1 FROM reader_return_coverage WHERE ended_at IS NULL AND last_heartbeat_at>now()-interval '90 seconds')")==='t',
      'the real collection worker confirms coverage before progress');
    await privateResponse(await readerPage.goto(base+'/admin/audience/returns?days=bad'),403);
    assert.equal(await readerPage.getByTestId('audience-returns-current').count(),0);
    await adminPage.goto(base+'/me');
    await adminPage.locator('a[href="/admin/audience"]').click();
    await adminPage.waitForURL('**/admin/audience');
    await adminPage.goto(base+'/admin/audience?days=7');
    await adminPage.getByTestId('audience-returns-link').click();
    await adminPage.waitForURL(url=>url.pathname==='/admin/audience/returns' && url.searchParams.get('days')==='7');
    await select(adminPage,90); await periods(adminPage,90);
    assert.ok(await adminPage.getByTestId('audience-returns-observation-start').isVisible());
    const before=await report(adminPage);

    // A real first-chapter fact without REC consent is not a new daily cohort.
    assert.equal((await readerPage.goto(base+`/read/${first.chapter}`)).status(),200);
    await progress(readerPage,first,3);
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_starts WHERE user_id=${users[1]}`),1);
    assert.equal(daysFor(users[1]),0); assert.equal(cohortsFor(users[1]),0);
    assert.deepEqual(await report(adminPage),before);
    await consent(readerPage,true);
    // A zero anchor and a rejected duplicate cannot start observation.
    await progress(readerPage,noStart,0);
    await progress(readerPage,first,3,409);
    assert.equal(daysFor(users[1]),0); assert.equal(cohortsFor(users[1]),0);
    await progress(readerPage,first,6);
    await progress(readerPage,first,9);
    await progress(readerPage,first,9,409);
    await progress(readerPage,later,3);
    await progress(readerPage,other,3);
    assert.equal(daysFor(users[1]),2,'several saves/chapters keep one fact per book and UTC day');
    assert.equal(cohortsFor(users[1]),1,'several books share the same site-wide cohort');
    pendingDelta(before,await report(adminPage),1);

    // A second real reader uses the visible manual save action.
    await consent(manualPage,true);
    assert.equal((await manualPage.goto(base+`/read/${manual.chapter}`)).status(),200);
    await manualPage.getByRole('button',{name:'Сохранить после фрагмента 2',exact:true}).click();
    await manualPage.waitForURL(base+`/read/${manual.chapter}#p-2`);
    assert.equal(daysFor(users[2]),1); assert.equal(cohortsFor(users[2]),1);
    const enrolled=await report(adminPage); pendingDelta(before,enrolled,2);
    assert.equal(scalar(`SELECT count(DISTINCT activity_day) FROM reader_activity_days
      WHERE subject_id IN(SELECT subject_id FROM rec_subjects WHERE user_id IN(${users.slice(1).join(',')}))`),1);
    for (const width of [390,1440]) {
      await adminPage.setViewportSize({width,height:width===390?844:900});
      assert.deepEqual(await report(adminPage),enrolled,'viewing the report has no side effects');
      const html=await adminPage.content();
      for (const forbidden of [...emails,'Readable first paragraph.','RETURN-TITLE-'+run]) {
        assert.ok(!html.includes(forbidden),'no identities, text or book metadata');
      }
      assert.ok(await adminPage.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'report fits viewport');
      await adminPage.screenshot({path:path.join(shots,`audience-returns-pending-${width}.png`),fullPage:true});
    }
    for (const days of [1,7,30,90]) {
      await select(adminPage,days); await periods(adminPage,days);
      assert.equal(await adminPage.getByTestId('audience-returns-current').locator('[data-return-day]').count(),3);
    }

    // Opt-out erases the still-personal cohort; accepted reading remains usable.
    const oldSubject=sql(`SELECT subject_id FROM rec_subjects WHERE user_id=${users[1]}`);
    await consent(readerPage,false);
    assert.equal(scalar(`SELECT count(*) FROM reader_activity_days WHERE subject_id='${oldSubject}'::uuid`),0);
    assert.equal(scalar(`SELECT count(*) FROM reader_return_subjects WHERE subject_id='${oldSubject}'::uuid`),0);
    await progress(readerPage,other,6);
    assert.equal(daysFor(users[1]),0);
    pendingDelta(before,await report(adminPage),1);
    await consent(readerPage,true);
    assert.notEqual(sql(`SELECT subject_id FROM rec_subjects WHERE user_id=${users[1]}`),oldSubject);
    await progress(readerPage,other,6,409);
    assert.equal(cohortsFor(users[1]),0,'re-consent cannot resurrect the old observation');
    await progress(readerPage,other,9);
    assert.equal(cohortsFor(users[1]),1); assert.equal(daysFor(users[1]),1);
    pendingDelta(before,await report(adminPage),2);
    await privateResponse(await adminPage.goto(base+'/admin/audience/returns?days=2'),400);
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${users[0]} AND role='administrator'`);
    await privateResponse(await adminPage.goto(base+'/admin/audience/returns?days=90'),403);
    assert.equal(await adminPage.getByTestId('audience-returns-current').count(),0);
    const anonymous=await browser.newPage();
    await privateResponse(await anonymous.goto(base+'/admin/audience/returns'),401);
    assert.deepEqual(errors,[]);
    console.log('FLOW returns browser PASS: genuine CAS and manual progress, one consent-epoch cohort, repeat/day dedupe, no invented history, pending D1/D7/D30, opt-out/re-consent, live admin, 390/1440');
  } finally {
    if(browser) await browser.close();
    if(users.length) sql(`DELETE FROM account_roles WHERE user_id IN(${users.join(',')})`);
    if(books.length) {
      const ids=books.join(',');
      sql(`BEGIN; DELETE FROM reader_book_positions WHERE book_id IN(${ids});
        DELETE FROM reader_chapter_starts WHERE book_id IN(${ids}); DELETE FROM reader_chapter_completions WHERE book_id IN(${ids});
        DELETE FROM chapter_publications WHERE chapter_id IN(SELECT id FROM chapters WHERE book_id IN(${ids}));
        DELETE FROM book_members WHERE book_id IN(${ids}); DELETE FROM books WHERE id IN(${ids});
        DELETE FROM rec_quality_daily WHERE book_id IN(${ids}); DELETE FROM organic_discovery_quality_daily WHERE book_id IN(${ids});
        DELETE FROM rec_dirty_buckets WHERE book_id IN(${ids}); DELETE FROM organic_discovery_dirty WHERE book_id IN(${ids}); COMMIT`);
    }
    sql(`DELETE FROM users WHERE email_normalized IN(${emails.map(email=>`'${email}'`).join(',')})`);
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
