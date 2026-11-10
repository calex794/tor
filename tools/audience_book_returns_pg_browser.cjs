'use strict';
// FLOW: accepted reading today creates bounded observations for a selected book.
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
  throw new Error('Book-returns acceptance requires disposable loopback tor_ci');
}
fs.mkdirSync(shots, {recursive:true});

function sql(statement) {
  return execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1',
    '--dbname',db,'--command',statement], {
    encoding:'utf8', timeout:15000, env:{...process.env, PGCONNECT_TIMEOUT:'5',
      PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'},
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
  await page.getByLabel('Отображаемое имя').fill('Проверка возвращения к книге');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('AudienceBookReturnsPassword2026');
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
function observationsFor(user,book) {
  return scalar(`SELECT count(*) FROM reader_book_return_subjects
    WHERE subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id=${user})
    ${book === undefined ? '' : 'AND book_id='+book}`);
}
async function metric(locator,key) {
  return (await locator.locator(`[data-metric='${key}']`).innerText()).trim();
}
async function report(page,book,days=90) {
  await privateResponse(await page.goto(base+`/admin/audience/returns?book_id=${book}&days=${days}`));
  assert.equal(await page.getByRole('heading',{name:'Возвращаются ли к этой книге',exact:true}).count(),1);
  assert.equal(await page.locator('input[type="hidden"][name="book_id"]').inputValue(),String(book));
  const result = {};
  for (const period of ['current','previous']) {
    const section = page.getByTestId('audience-book-returns-'+period);
    const cohort = await metric(section,'cohort');
    assert.match(cohort,/^\d+$/,'observed episodes have an explicit count');
    result[period]={cohort:Number(cohort),days:{}};
    assert.equal(await section.locator('article[data-return-day]').count(),2);
    for (const day of [7,30]) {
      const row = section.locator(`article[data-return-day='${day}']`);
      const values = {};
      for (const key of ['eligible','returned','pending','unknown','rate']) {
        const value = await metric(row,key);
        values[key] = /^\d+$/.test(value) ? Number(value) : value;
      }
      assert.equal(values.eligible+values.pending+values.unknown,Number(cohort));
      if (values.eligible === 0) {
        assert.equal(values.returned,'—','no observed denominator is not a measured zero');
        assert.equal(values.rate,'Пока не рассчитана');
      }
      result[period].days[day]=values;
    }
  }
  return result;
}
function pendingDelta(before,after,delta) {
  assert.equal(after.current.cohort,before.current.cohort+delta,'one observation per book and current window');
  assert.deepEqual(after.previous,before.previous,'today cannot fill past observation groups');
  for (const day of [7,30]) {
    const a=before.current.days[day], b=after.current.days[day];
    assert.equal(b.pending,a.pending+delta,'the exact complete target day is still awaited');
    for (const key of ['eligible','returned','unknown','rate']) {
      assert.equal(b[key],a[key],'today cannot change mature '+day+' '+key);
    }
  }
}
async function select(page,book,days) {
  await page.locator('select[name="days"]').selectOption(String(days));
  await page.getByRole('button',{name:'Показать',exact:true}).click();
  await page.waitForURL(url => url.pathname === '/admin/audience/returns'
    && url.searchParams.get('days') === String(days)
    && url.searchParams.get('book_id') === String(book));
}
async function periods(page,days) {
  const bounds = {};
  for (const period of ['current','previous']) {
    bounds[period]=(await page.getByTestId('audience-book-returns-'+period).innerText())
      .match(/\b\d{4}-\d{2}-\d{2}\b/g) || [];
    assert.equal(bounds[period].length,2);
    assert.equal(Date.parse(bounds[period][1])-Date.parse(bounds[period][0]),days*86400000);
  }
  assert.equal(bounds.previous[1],bounds.current[0]);
  assert.equal(bounds.current[1],sql("SELECT ((now() AT TIME ZONE 'UTC')::date+1)::text"));
}

(async () => {
  const run=Date.now()+'-'+randomBytes(4).toString('hex');
  const genre='book-returns-'+run;
  const title='Книга для возвращения '+run;
  const otherTitle='Другая книга '+run;
  const emails=['admin','api-reader','manual-reader'].map(role=>`flow-book-returns-${role}-${run}@tor-ci.example`);
  const books=[], users=[], errors=[];
  let browser;
  function book(key,title) {
    const id=scalar(`INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres)
      VALUES('ci:flow:book-returns:${run}:${key}','ru','${title}','public','basic',ARRAY['${genre}']) RETURNING id`);
    books.push(id); return id;
  }
  try {
    const first=chapter(book('selected',title),1), later=chapter(first.book,2);
    const manual=chapter(first.book,3);
    const other=chapter(book('other',otherTitle),1);
    browser=await chromium.launch({headless:true});
    const pages=[];
    for (const [index,email] of emails.entries()) {
      const context=await browser.newContext({viewport:{width:index===0?1440:390,height:900}});
      context.setDefaultTimeout(15000);
      const page=await context.newPage();
      page.on('pageerror',error=>errors.push(error.message));
      users.push(await register(page,email)); pages.push(page);
    }
    const [adminPage,readerPage,manualPage]=pages;
    sql(`INSERT INTO account_roles(user_id,role) VALUES(${users[0]},'administrator')`);
    await waitFor(()=>sql(`SELECT EXISTS(SELECT 1 FROM reader_book_return_coverage
      WHERE ended_at IS NULL AND last_heartbeat_at>now()-interval '30 seconds')`)==='t',
    'the real worker confirms separate book coverage before progress');
    await privateResponse(await readerPage.goto(base+`/admin/audience/returns?book_id=${first.book}&days=bad`),403);
    assert.equal(await readerPage.getByTestId('audience-book-returns-current').count(),0);
    await adminPage.goto(base+'/me');
    await adminPage.locator('a[href="/admin/audience"]').click();
    await adminPage.waitForURL('**/admin/audience');
    await adminPage.locator('a[href^="/admin/audience/books"]').click();
    await adminPage.waitForURL(url=>url.pathname==='/admin/audience/books');
    await adminPage.locator('#books-days').selectOption('7');
    await adminPage.locator('#books-genre').fill(genre);
    await adminPage.locator('#audience-books-filters').getByRole('button',{name:'Показать',exact:true}).click();
    await adminPage.waitForURL(url=>url.searchParams.get('genre')===genre && url.searchParams.get('days')==='7');
    const listUrl=adminPage.url();
    const row=adminPage.locator(`[data-testid='audience-book-row'][data-book-id='${first.book}']`);
    const link=row.getByTestId('audience-book-returns-link');
    assert.equal(await link.count(),1,'selected book has one administrative return link');
    const href=new URL(await link.getAttribute('href'),base);
    assert.equal(href.pathname,'/admin/audience/returns');
    assert.equal(href.searchParams.get('book_id'),String(first.book));
    assert.equal(href.searchParams.get('days'),'7');
    await link.click();
    await adminPage.waitForURL(url=>url.pathname==='/admin/audience/returns'
      && url.searchParams.get('book_id')===String(first.book) && url.searchParams.get('days')==='7');
    assert.equal(await adminPage.getByTestId('audience-book-returns-title').innerText(),title);
    await select(adminPage,first.book,90); await periods(adminPage,90);
    const before=await report(adminPage,first.book);
    assert.equal(before.current.cohort,0);
    assert.equal(before.previous.cohort,0);
    const otherBefore=await report(adminPage,other.book);

    // A genuine first start before consent stays canonical reading, without
    // creating a book observation or recovering history when consent changes.
    assert.equal((await readerPage.goto(base+`/read/${first.chapter}`)).status(),200);
    await progress(readerPage,first,3);
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_starts WHERE user_id=${users[1]}`),1);
    assert.equal(observationsFor(users[1]),0);
    assert.deepEqual(await report(adminPage,first.book),before);
    await consent(readerPage,true);
    await progress(readerPage,other,0);
    await progress(readerPage,first,3,409);
    assert.equal(observationsFor(users[1]),0,'zero anchors and rejected retries never start observations');

    // A dateless canonical global completion marker is the only state fixture.
    // It supplies no date, positive book fact or invented historical return.
    // The next genuine C save must still create a new selected-book observation.
    sql(`INSERT INTO reader_return_subjects(subject_id)
      SELECT subject_id FROM rec_subjects WHERE user_id=${users[1]}`);
    await progress(readerPage,first,6);
    assert.equal(observationsFor(users[1],first.book),1,'book collection survives the completed global observation');
    assert.equal(scalar(`SELECT count(*) FROM reader_return_subjects WHERE cohort_day IS NULL
      AND subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id=${users[1]})`),1);
    assert.equal(scalar(`SELECT count(*) FROM reader_activity_days
      WHERE subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id=${users[1]})`),0,
    'finished global collection does not resume detailed book/day history');
    await progress(readerPage,first,9);
    await progress(readerPage,first,9,409);
    await progress(readerPage,later,3);
    assert.equal(observationsFor(users[1],first.book),1);
    const one=await report(adminPage,first.book); pendingDelta(before,one,1);
    await progress(readerPage,other,3);
    assert.equal(observationsFor(users[1],other.book),1);
    assert.deepEqual(await report(adminPage,first.book),one,'a different book cannot count as a return to this book');
    pendingDelta(otherBefore,await report(adminPage,other.book),1);

    // Another browser context uses the same live account and accepted progress.
    const deviceContext=await browser.newContext({viewport:{width:1440,height:900}});
    deviceContext.setDefaultTimeout(15000);
    await deviceContext.addCookies(await readerPage.context().cookies());
    const devicePage=await deviceContext.newPage();
    devicePage.on('pageerror',error=>errors.push(error.message));
    const deviceReaderUrl=base+`/read/${later.chapter}`;
    const deviceReader=await devicePage.goto(deviceReaderUrl);
    assert.equal(deviceReader.status(),200,'the second device can open the actual reading page');
    assert.equal(devicePage.url(),deviceReaderUrl,'the second device stays on the same-origin reader');
    assert.equal(await devicePage.locator(`form[action='/read/${later.chapter}/place'] input[name='version_id']`).first().inputValue(),String(later.version),
      'the server verifies the copied live session before exposing the exact-version save form');
    await progress(devicePage,later,6);
    assert.equal(observationsFor(users[1],first.book),1,'chapters and devices share the same book window');
    assert.deepEqual(await report(adminPage,first.book),one);
    await deviceContext.close();

    // A second real reader uses the visible manual action in the same book.
    await consent(manualPage,true);
    assert.equal((await manualPage.goto(base+`/read/${manual.chapter}`)).status(),200);
    await manualPage.getByRole('button',{name:'Сохранить после фрагмента 2',exact:true}).click();
    await manualPage.waitForURL(base+`/read/${manual.chapter}#p-2`);
    assert.equal(observationsFor(users[2],first.book),1);
    const enrolled=await report(adminPage,first.book); pendingDelta(before,enrolled,2);
    assert.equal(scalar(`SELECT count(*) FROM reader_book_return_subjects WHERE book_id IN(${books.join(',')})
      AND (cohort_day<>(now() AT TIME ZONE 'UTC')::date OR returned_d7 OR returned_d30)`),0,
    'browser evidence contains only today and no fabricated mature return');
    assert.equal(scalar(`SELECT count(*) FROM reader_book_return_daily WHERE book_id IN(${books.join(',')})`),0);
    const subjects=sql(`SELECT string_agg(subject_id::text,',') FROM rec_subjects WHERE user_id IN(${users.slice(1).join(',')})`).split(',');
    for (const width of [390,1440]) {
      await adminPage.setViewportSize({width,height:width===390?844:900});
      assert.deepEqual(await report(adminPage,first.book),enrolled,'report requests have no collection side effects');
      assert.equal(await adminPage.getByTestId('audience-book-returns-title').innerText(),title);
      const html=await adminPage.content();
      for (const forbidden of [...emails,...subjects,'Readable first paragraph.',otherTitle]) {
        assert.ok(!html.includes(forbidden),'no personal identifiers, text or other-book metadata');
      }
      assert.match(await adminPage.locator('body').innerText(),/наблюдени/);
      assert.ok(await adminPage.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'report fits viewport');
      await adminPage.screenshot({path:path.join(shots,`audience-book-returns-pending-${width}.png`),fullPage:true});
    }
    for (const days of [1,7,30,90]) {
      await select(adminPage,first.book,days); await periods(adminPage,days);
      assert.equal(await adminPage.getByTestId('audience-book-returns-title').innerText(),title);
      assert.equal(await adminPage.getByTestId('audience-book-returns-current').locator('article[data-return-day]').count(),2);
      assert.equal(await metric(adminPage.getByTestId('audience-book-returns-current'),'cohort'),'2');
    }

    // Opt-out deletes both still-personal book episodes, while accepted reading
    // continues. A new consent identity cannot resurrect either old observation.
    const oldSubject=sql(`SELECT subject_id FROM rec_subjects WHERE user_id=${users[1]}`);
    await consent(readerPage,false);
    assert.equal(scalar(`SELECT count(*) FROM reader_book_return_subjects WHERE subject_id='${oldSubject}'::uuid`),0);
    assert.equal(scalar(`SELECT count(*) FROM reader_return_subjects WHERE subject_id='${oldSubject}'::uuid`),0);
    await progress(readerPage,later,9);
    await progress(readerPage,other,6);
    assert.equal(observationsFor(users[1]),0);
    pendingDelta(before,await report(adminPage,first.book),1);
    assert.deepEqual(await report(adminPage,other.book),otherBefore);
    await consent(readerPage,true);
    assert.notEqual(sql(`SELECT subject_id FROM rec_subjects WHERE user_id=${users[1]}`),oldSubject);
    await progress(readerPage,later,9,409);
    assert.equal(observationsFor(users[1]),0);
    await progress(readerPage,later,12);
    assert.equal(observationsFor(users[1],first.book),1);
    pendingDelta(before,await report(adminPage,first.book),2);
    await progress(readerPage,other,9);
    assert.equal(observationsFor(users[1],other.book),1);
    assert.deepEqual(await report(adminPage,first.book),enrolled);
    pendingDelta(otherBefore,await report(adminPage,other.book),1);

    sql(`UPDATE books SET admin_hold=true WHERE id=${first.book}`);
    await privateResponse(await adminPage.goto(listUrl));
    assert.equal(await adminPage.locator(`[data-book-id='${first.book}']`).count(),0);
    await privateResponse(await adminPage.goto(base+`/admin/audience/returns?book_id=${first.book}&days=90`),404);
    assert.equal(await adminPage.getByTestId('audience-book-returns-current').count(),0);
    assert.ok(!(await adminPage.content()).includes(title),'current hidden metadata cannot leak');
    sql(`UPDATE books SET admin_hold=false WHERE id=${first.book}`);
    assert.deepEqual(await report(adminPage,first.book),enrolled);
    await privateResponse(await adminPage.goto(base+`/admin/audience/returns?book_id=${first.book}&days=2`),400);
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${users[0]} AND role='administrator'`);
    await privateResponse(await adminPage.goto(base+`/admin/audience/returns?book_id=${first.book}&days=90`),403);
    assert.equal(await adminPage.getByTestId('audience-book-returns-current').count(),0);
    const anonymous=await browser.newPage();
    await privateResponse(await anonymous.goto(base+`/admin/audience/returns?book_id=${first.book}`),401);
    assert.deepEqual(errors,[]);
    console.log('FLOW book returns browser PASS: books-to-selected-book, real CAS/manual progress, completed global marker, same-book isolation, chapter/device/retry dedupe, pending D7/D30, consent rotation, hidden book, live admin, 390/1440');
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
