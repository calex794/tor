'use strict';
// FLOW retirement: real reading does not extend enabled legacy group observations.
// Historical fixtures separately verify grouping, public metadata and exact drill-down.
const {chromium} = require('playwright');
const {execFileSync} = require('node:child_process');
const {randomBytes} = require('node:crypto');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const db = require('./legacy_analytics_fixture.cjs')();
const base = process.env.TOR_AUDIENCE_BASE || 'http://127.0.0.1:3108';
const shots = process.env.TOR_AUDIENCE_SHOTS || '/tmp/audience-browser-shots';
const parsed = new URL(db);
if (parsed.protocol !== 'postgresql:' || parsed.hostname !== '127.0.0.1'
    || parsed.port !== '5432' || parsed.pathname !== '/tor_ci_legacy_analytics'
    || !/^http:\/\/127\.0\.0\.1:\d+$/.test(base)) {
  throw new Error('Group-return acceptance requires disposable loopback legacy analytics baseline 191');
}
fs.mkdirSync(shots, {recursive:true});
const legacy = require('./audience_retirement_fixtures.cjs')({sql,scalar,base});
const preview = require('./ci_browser_preview.cjs');

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
async function register(page, email) {
  await page.goto(base+'/register');
  await page.getByLabel('Отображаемое имя').fill('Проверка возвращений группы');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('AudienceGroupReturnsPassword2026');
  await page.getByRole('button',{name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
  return scalar(`SELECT id FROM users WHERE email_normalized='${email}'`);
}
async function retiredConsent(page, enabled) {
  await legacy.retiredConsent(page,enabled);
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
  assert.equal((await page.goto(base+`/read/${fixture.chapter}`)).status(),200);
  assert.equal((await page.goto(base+`/reader/${fixture.chapter}`)).status(),200);
  await page.waitForFunction(() => document.querySelector('[data-free-progress-chapter]')?.dataset.freeProgressReady==='1');
  assert.equal(page.url(),base+`/reader/${fixture.chapter}`,'progress runs from the actual AJAX reader with its live session');
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
function reportPath(kind,label,days=90) {
  const params=new URLSearchParams({kind,days:String(days)});
  params.set(label===null?'without_label':'label',label===null?'true':label);
  return '/admin/audience/returns?'+params;
}
async function snapshot(page,kind,label) {
  await legacy.labelled(page,'audience-group-returns');
  assert.equal(await page.getByRole('heading',{name:'Возвращения к книгам группы',exact:true}).count(),1);
  assert.equal(await page.getByTestId('audience-group-returns-title').innerText(),
    label===null ? (kind==='genres'?'Без жанра':'Без тега') : label);
  const form=page.locator('#audience-group-returns-filters');
  assert.equal(await form.locator('input[type="hidden"][name="kind"]').inputValue(),kind);
  if(label===null) {
    assert.equal(await form.locator('input[name="without_label"]').inputValue(),'true');
    assert.equal(await form.locator('input[name="label"]').count(),0);
  } else {
    assert.equal(await form.locator('input[name="label"]').inputValue(),label);
  }
  const result={books:Number(await page.getByTestId('audience-group-returns-book-count').innerText())};
  for(const period of ['current','previous']) {
    const section=page.getByTestId('audience-group-returns-'+period);
    const cohort=await metric(section,'cohort');
    assert.match(cohort,/^\d+$/);
    result[period]={cohort:Number(cohort),days:{}};
    assert.equal(await section.locator('article[data-return-day]').count(),2);
    for(const day of [7,30]) {
      const row=section.locator(`article[data-return-day='${day}']`), values={};
      for(const key of ['eligible','returned','pending','unknown','rate']) {
        const value=await metric(row,key);
        values[key]=/^\d+$/.test(value)?Number(value):value;
      }
      assert.equal(values.eligible+values.pending+values.unknown,Number(cohort));
      if(values.eligible===0) {
        assert.equal(values.returned,'—','waiting is not a measured zero');
        assert.equal(values.rate,'Пока не рассчитана');
      }
      result[period].days[day]=values;
    }
  }
  return result;
}
async function report(page,kind,label,days=90) {
  await privateResponse(await page.goto(base+reportPath(kind,label,days)));
  return snapshot(page,kind,label);
}
function pendingDelta(before,after,delta) {
  assert.equal(after.books,before.books,'reading does not change current group membership');
  assert.equal(after.current.cohort,before.current.cohort+delta,'one observation per account and book window');
  assert.deepEqual(after.previous,before.previous,'today cannot fill past cohorts');
  for(const day of [7,30]) {
    const a=before.current.days[day],b=after.current.days[day];
    assert.equal(b.pending,a.pending+delta);
    for(const key of ['eligible','returned','unknown','rate']) assert.equal(b[key],a[key],`unchanged mature D${day} ${key}`);
  }
}
async function select(page,kind,label,days) {
  const form=page.locator('#audience-group-returns-filters');
  await form.locator('select[name="days"]').selectOption(String(days));
  await form.getByRole('button',{name:'Показать',exact:true}).click();
  await page.waitForURL(url=>url.pathname==='/admin/audience/returns'
    && url.searchParams.get('kind')===kind && url.searchParams.get('days')===String(days)
    && (label===null ? url.searchParams.get('without_label')==='true' : url.searchParams.get('label')===label));
}
async function periods(page,days) {
  const bounds={};
  for(const period of ['current','previous']) {
    bounds[period]=(await page.getByTestId('audience-group-returns-'+period).innerText())
      .match(/\b\d{4}-\d{2}-\d{2}\b/g)||[];
    assert.equal(bounds[period].length,2);
    assert.equal(Date.parse(bounds[period][1])-Date.parse(bounds[period][0]),days*86400000);
  }
  assert.equal(bounds.previous[1],bounds.current[0]);
  assert.equal(bounds.current[1],sql("SELECT ((now() AT TIME ZONE 'UTC')::date+1)::text"));
}
function group(page,label) {
  return page.locator(`[data-testid='audience-group-row'][data-label=${JSON.stringify(label||'')}]`);
}
async function nextListPage(page,surface,visited) {
  const link=page.getByTestId(`audience-${surface}-next`);
  if(await link.count()===0) return false;
  assert.equal(await link.count(),1,'one next page link');
  const current=new URL(page.url()), next=new URL(await link.getAttribute('href'),base);
  const cursor=surface==='books'?'after_id':'after';
  const filters=url=>[...url.searchParams].filter(([key])=>key!==cursor)
    .sort(([left],[right])=>left.localeCompare(right));
  assert.equal(next.origin,current.origin);
  assert.equal(next.pathname,'/admin/audience/'+surface);
  assert.equal(next.pathname,current.pathname);
  assert.deepEqual(filters(next),filters(current),'pagination preserves every filter and the selected period');
  assert.ok(next.searchParams.get(cursor),'the next link has an explicit cursor');
  assert.notEqual(next.searchParams.get(cursor),current.searchParams.get(cursor));
  if(surface==='books') {
    assert.match(next.searchParams.get(cursor),/^[1-9]\d*$/);
    assert.ok(BigInt(next.searchParams.get(cursor))>BigInt(current.searchParams.get(cursor)||'0'),
      'the book cursor strictly advances');
  }
  assert.ok(!visited.has(next.href),'pagination must not loop');
  visited.add(next.href);
  await link.click();
  await page.waitForURL(next.href);
  return true;
}
async function findRow(page,surface,row,description) {
  const visited=new Set([page.url()]),seenBooks=new Set();
  for(;;) {
    if(surface==='books') {
      const after=BigInt(new URL(page.url()).searchParams.get('after_id')||'0');
      const ids=await page.getByTestId('audience-book-row').evaluateAll(rows=>rows.map(row=>row.dataset.bookId));
      for(const id of ids) {
        assert.ok(BigInt(id)>after,'each book follows the current cursor');
        assert.ok(!seenBooks.has(id),'books do not repeat while searching successive pages');
        seenBooks.add(id);
      }
    }
    if(await row.count()!==0) break;
    assert.ok(await nextListPage(page,surface,visited),description+' must exist in the complete filtered list');
  }
  assert.equal(await row.count(),1,description+' is unique on its page');
  return row;
}
async function findBook(page,book) {
  return findRow(page,'books',page.locator(`[data-testid='audience-book-row'][data-book-id='${book}']`),'selected book');
}
async function enterGroup(page,kind,label,days) {
  await privateResponse(await page.goto(base+`/admin/audience/groups?kind=${kind}&days=${days}`));
  const row=await findRow(page,'groups',group(page,label),'selected group');
  const link=row.getByTestId('audience-group-returns-link');
  assert.equal(await link.count(),1);
  const href=new URL(await link.getAttribute('href'),base);
  assert.equal(href.pathname,'/admin/audience/returns');
  assert.equal(href.searchParams.get('kind'),kind);
  assert.equal(href.searchParams.get('days'),String(days));
  assert.equal(href.searchParams.get(label===null?'without_label':'label'),label===null?'true':label);
  await link.click();
  await page.waitForURL(href.href);
  return snapshot(page,kind,label);
}
async function exactBooks(page,kind,label,days,bookIds) {
  const link=page.getByTestId('audience-group-returns-books');
  const href=new URL(await link.getAttribute('href'),base);
  const field=kind==='genres'?'genre':'tag';
  assert.equal(href.pathname,'/admin/audience/books');
  assert.equal(href.searchParams.get('days'),String(days));
  assert.equal(href.searchParams.get(label===null?'without_'+field:field),label===null?'true':label);
  await link.click();
  await page.waitForURL(href.href);
  if(bookIds!==null) {
    const actual=[],visited=new Set([page.url()]);
    do {
      actual.push(...await page.getByTestId('audience-book-row').evaluateAll(rows=>rows.map(row=>Number(row.dataset.bookId))));
    } while(await nextListPage(page,'books',visited));
    assert.equal(new Set(actual).size,actual.length,'books do not repeat across pages');
    assert.deepEqual(actual.sort((a,b)=>a-b),[...bookIds].sort((a,b)=>a-b),'all detail pages use exactly the same current books');
    if(visited.size>1) await privateResponse(await page.goto(href.href));
  }
}

(async()=>{
  const run=Date.now()+'-'+randomBytes(4).toString('hex');
  const genre='!возврат + "'+run+'"', overlap='!другой '+run;
  const tag='!метка &<'+run, otherTag='!путь '+run, moved='!перенос '+run;
  assert.ok([genre,overlap,tag,otherTag,moved].every(label=>[...label].length<40));
  const emails=['admin','api-reader','manual-reader'].map(role=>`flow-group-returns-${role}-${run}@tor-ci.example`);
  const books=[],users=[],errors=[],titles=[];
  const array=labels=>labels.length?'ARRAY['+labels.map(value=>"'"+value.replaceAll("'","''")+"'").join(',')+']':"'{}'::text[]";
  let browser,coverage;
  function createBook(key,genres,tags) {
    const title='Книга группы '+key+' '+run;
    titles.push(title);
    const book=scalar(`INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres,tags)
      VALUES('ci:flow:group-returns:${run}:${key}','ru','${title}','public','basic',${array(genres)},${array(tags)}) RETURNING id`);
    books.push(book);
    return book;
  }
  function create(key,genres,tags) {
    return chapter(createBook(key,genres,tags),1);
  }
  try {
    const first=create('first',[genre,overlap,genre],[tag,otherTag,tag]);
    const later=chapter(first.book,2),manual=chapter(first.book,3);
    const second=create('second',[genre],[tag]);
    const different=create('different',[overlap],[otherTag]);
    // Catalogue-only fixtures make the real next-page path mandatory even in
    // an otherwise empty database. They contain no chapter or analytics fact.
    for(let index=0;index<21;index++) createBook('pagination-'+index,[],[]);
    const missing=create('missing',[],[]);
    browser=await chromium.launch({headless:true});
    const pages=[];
    for(const [index,email] of emails.entries()) {
      const context=await browser.newContext({viewport:{width:index===0?1440:390,height:900}});
      context.setDefaultTimeout(15000);
      const page=await context.newPage();
      page.on('pageerror',error=>errors.push(error.message));
      users.push(await register(page,email)); pages.push(page);
    }
    const [adminPage,readerPage,manualPage]=pages;
    sql(`INSERT INTO account_roles(user_id,role) VALUES(${users[0]},'administrator')`);
    coverage=legacy.coverage('book');
    await privateResponse(await readerPage.goto(base+'/admin/audience/returns?kind=bad&label=&days=bad'),403);
    assert.equal(await readerPage.getByTestId('audience-group-returns-current').count(),0);
    await adminPage.goto(base+'/me');
    await adminPage.locator('a[href="/admin/audience"]').click();
    await adminPage.waitForURL('**/admin/audience');
    await adminPage.getByTestId('audience-groups-link').click();
    await adminPage.waitForURL(url=>url.pathname==='/admin/audience/groups');
    await adminPage.locator('#groups-days').selectOption('7');
    await adminPage.locator('#audience-groups-filters').getByRole('button',{name:'Показать',exact:true}).click();
    await adminPage.waitForURL(url=>url.searchParams.get('days')==='7');
    const selectedGroup=await findRow(adminPage,'groups',group(adminPage,genre),'selected group');
    const entry=selectedGroup.getByTestId('audience-group-returns-link');
    await entry.click();
    await adminPage.waitForURL(url=>url.pathname==='/admin/audience/returns'
      && url.searchParams.get('kind')==='genres' && url.searchParams.get('label')===genre && url.searchParams.get('days')==='7');
    const entered=await snapshot(adminPage,'genres',genre);
    assert.equal(entered.books,2);
    await select(adminPage,'genres',genre,90); await periods(adminPage,90);
    const before=await snapshot(adminPage,'genres',genre);
    assert.equal(before.current.cohort,0); assert.equal(before.previous.cohort,0);
    const tagBefore=await report(adminPage,'tags',tag);
    const overlapBefore=await report(adminPage,'genres',overlap);
    const missingBefore={};
    for(const kind of ['genres','tags']) missingBefore[kind]=await report(adminPage,kind,null);

    // Canonical progress without consent is still reading, but does not enroll
    // a book observation. Zero and rejected repeated positions do not enroll it.
    await progress(readerPage,first,3);
    assert.equal(observationsFor(users[1]),0);
    assert.deepEqual(await report(adminPage,'genres',genre),before);
    legacy.subject(users[1]);
    const activeLegacy=legacy.snapshot(users[1]);
    await retiredConsent(readerPage,true);
    assert.deepEqual(legacy.snapshot(users[1]),activeLegacy);
    await progress(readerPage,second,0);
    await progress(readerPage,first,3,409);
    assert.equal(observationsFor(users[1]),0);
    await progress(readerPage,first,6);
    assert.equal(observationsFor(users[1]),0);
    assert.deepEqual(legacy.snapshot(users[1]),activeLegacy);
    // Each report row below is an explicit historical fixture, not C capture.
    legacy.book(users[1],first.book);
    const one=await report(adminPage,'genres',genre); pendingDelta(before,one,1);
    await progress(readerPage,first,9); await progress(readerPage,first,9,409);
    await progress(readerPage,later,3);
    assert.deepEqual(await report(adminPage,'genres',genre),one);
    const oneFrozen=legacy.snapshot(users[1]);
    await progress(readerPage,second,3);
    assert.deepEqual(legacy.snapshot(users[1]),oneFrozen);
    legacy.book(users[1],second.book);
    assert.equal(observationsFor(users[1]),2,'two explicit legacy fixtures share the group');
    pendingDelta(before,await report(adminPage,'genres',genre),2);
    pendingDelta(tagBefore,await report(adminPage,'tags',tag),2);
    const twoFrozen=legacy.snapshot(users[1]);
    await progress(readerPage,different,3);
    assert.deepEqual(legacy.snapshot(users[1]),twoFrozen);
    legacy.book(users[1],different.book);
    pendingDelta(overlapBefore,await report(adminPage,'genres',overlap),2);
    pendingDelta(before,await report(adminPage,'genres',genre),2,'a book outside this group cannot enter its sum');
    const threeFrozen=legacy.snapshot(users[1]);
    await progress(readerPage,missing,3);
    assert.deepEqual(legacy.snapshot(users[1]),threeFrozen);
    legacy.book(users[1],missing.book);
    for(const kind of ['genres','tags']) pendingDelta(missingBefore[kind],await report(adminPage,kind,null),1);
    // A second real device must leave every historical observation unchanged.
    const deviceFrozen=legacy.snapshot(users[1]);
    const deviceContext=await browser.newContext({viewport:{width:1440,height:900}});
    deviceContext.setDefaultTimeout(15000);
    await deviceContext.addCookies(await readerPage.context().cookies());
    const devicePage=await deviceContext.newPage();
    devicePage.on('pageerror',error=>errors.push(error.message));
    const deviceUrl=base+`/read/${later.chapter}`;
    assert.equal((await devicePage.goto(deviceUrl)).status(),200);
    assert.equal(devicePage.url(),deviceUrl);
    assert.equal(await devicePage.locator(`form[action='/read/${later.chapter}/place'] input[name='version_id']`).first().inputValue(),String(later.version));
    await progress(devicePage,later,6);
    assert.equal(observationsFor(users[1],first.book),1);
    assert.deepEqual(legacy.snapshot(users[1]),deviceFrozen);
    await deviceContext.close();
    // A second account confirms a real visible manual save in the selected book.
    legacy.subject(users[2]);
    const manualFrozen=legacy.snapshot(users[2]);
    await retiredConsent(manualPage,true);
    assert.equal((await manualPage.goto(base+`/read/${manual.chapter}`)).status(),200);
    await manualPage.getByRole('button',{name:'Сохранить после фрагмента 2',exact:true}).click();
    await manualPage.waitForURL(base+`/read/${manual.chapter}#p-2`);
    assert.equal(observationsFor(users[2],first.book),0);
    // The native position form calls PgReadingProgress::save_position. It
    // persists the cursor without manufacturing a canonical chapter start.
    assert.equal(scalar(`SELECT count(*) FROM reader_book_positions
      WHERE user_id=${users[2]} AND book_id=${manual.book}
        AND chapter_id=${manual.chapter} AND version_id=${manual.version}
        AND block_index=2 AND char_offset=0`),1);
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_starts WHERE user_id=${users[2]}`),0);
    assert.deepEqual(legacy.snapshot(users[2]),manualFrozen);
    legacy.book(users[2],first.book);
    const enrolled=await report(adminPage,'genres',genre); pendingDelta(before,enrolled,3);
    pendingDelta(tagBefore,await report(adminPage,'tags',tag),3);
    pendingDelta(overlapBefore,await report(adminPage,'genres',overlap),3);
    assert.equal(scalar(`SELECT count(*) FROM reader_book_return_subjects WHERE book_id IN(${books.join(',')})
      AND (cohort_day<>(now() AT TIME ZONE 'UTC')::date OR returned_d7 OR returned_d30)`),0,
    'explicit historical fixtures do not assert a mature return');
    assert.equal(scalar(`SELECT count(*) FROM reader_book_return_daily WHERE book_id IN(${books.join(',')})`),0);
    const subjects=sql(`SELECT string_agg(subject_id::text,',') FROM rec_subjects WHERE user_id IN(${users.slice(1).join(',')})`).split(',');
    for(const width of [390,1440]) {
      await adminPage.setViewportSize({width,height:width===390?844:900});
      assert.deepEqual(await report(adminPage,'genres',genre),enrolled,'repeated GET does not collect');
      const html=await adminPage.content();
      for(const secret of [...emails,...subjects,...titles,'Readable first paragraph.']) assert.ok(!html.includes(secret),'no people, identities, book titles or chapter text');
      assert.match(await adminPage.locator('body').innerText(),/наблюдени/);
      assert.ok(await adminPage.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'report fits viewport');
      await adminPage.screenshot({path:path.join(shots,`audience-group-returns-pending-${width}.png`),fullPage:true});
      await preview(adminPage,`audience-group-returns-retired-${width}`);
    }
    for(const days of [1,7,30,90]) {
      await select(adminPage,'genres',genre,days); await periods(adminPage,days);
      const shown=await snapshot(adminPage,'genres',genre);
      assert.equal(shown.current.cohort,3);
      assert.equal(shown.previous.cohort,0);
    }
    // Real return drill-down: group -> exactly its books -> one selected book.
    await exactBooks(adminPage,'genres',genre,90,[first.book,second.book]);
    const selectedBook=await findBook(adminPage,first.book);
    await selectedBook.getByTestId('audience-book-returns-link').click();
    await adminPage.waitForURL(url=>url.pathname==='/admin/audience/returns'
      && url.searchParams.get('book_id')===String(first.book) && url.searchParams.get('days')==='90');
    assert.equal(await metric(adminPage.getByTestId('audience-book-returns-current'),'cohort'),'2');
    assert.equal(await metric(adminPage.getByTestId('audience-book-returns-current').locator('[data-return-day="7"]'),'returned'),'—');
    await enterGroup(adminPage,'tags',tag,7);
    await exactBooks(adminPage,'tags',tag,7,[first.book,second.book]);
    for(const kind of ['genres','tags']) {
      await enterGroup(adminPage,kind,null,7);
      await select(adminPage,kind,null,30); await periods(adminPage,30);
      await snapshot(adminPage,kind,null);
      await exactBooks(adminPage,kind,null,30,null);
      assert.equal(await adminPage.locator(`[data-testid='audience-book-row'][data-book-id='${missing.book}']`).count(),0,
        'the selected unlabelled book is deliberately beyond the first page');
      await findBook(adminPage,missing.book);
      assert.ok(new URL(adminPage.url()).searchParams.get('after_id'),'the missing-label drill-down really used pagination');
    }
    const empty=await report(adminPage,'genres','!пусто '+run);
    assert.equal(empty.books,0); assert.equal(empty.current.cohort,0);
    assert.equal(await adminPage.getByTestId('audience-group-returns-no-books').count(),1);

    // Current metadata moves all existing book observations, with no new events.
    sql(`UPDATE books SET genres=${array([moved])},tags=${array([otherTag])} WHERE id=${second.book}`);
    const movedFrom=await report(adminPage,'genres',genre);
    assert.equal(movedFrom.books,1); assert.equal(movedFrom.current.cohort,2);
    const movedTo=await report(adminPage,'genres',moved);
    assert.equal(movedTo.books,1); assert.equal(movedTo.current.cohort,1);
    assert.equal((await report(adminPage,'tags',tag)).current.cohort,2);
    assert.equal((await report(adminPage,'tags',otherTag)).current.cohort,4);
    sql(`UPDATE books SET genres=${array([genre])},tags=${array([tag])} WHERE id=${second.book}`);
    assert.deepEqual(await report(adminPage,'genres',genre),enrolled);
    sql(`UPDATE books SET admin_hold=true WHERE id=${first.book}`);
    const hidden=await report(adminPage,'genres',genre);
    assert.equal(hidden.books,1); assert.equal(hidden.current.cohort,1);
    await exactBooks(adminPage,'genres',genre,90,[second.book]);
    await privateResponse(await adminPage.goto(base+`/admin/audience/returns?book_id=${first.book}`),404);
    assert.equal(await adminPage.getByTestId('audience-book-returns-current').count(),0);
    sql(`UPDATE books SET admin_hold=false WHERE id=${first.book}`);
    assert.deepEqual(await report(adminPage,'genres',genre),enrolled);

    // Old consent commands cannot rotate, erase or resume the legacy model.
    // Fresh progress and replay remain real C operations, without enrichment.
    const frozen=legacy.snapshot(users[1]);
    await retiredConsent(readerPage,false);
    await progress(readerPage,later,9); await progress(readerPage,second,6);
    assert.deepEqual(legacy.snapshot(users[1]),frozen);
    assert.deepEqual(await report(adminPage,'genres',genre),enrolled);
    await retiredConsent(readerPage,true);
    await progress(readerPage,later,9,409);
    await progress(readerPage,later,12); await progress(readerPage,second,9);
    await progress(readerPage,different,6); await progress(readerPage,missing,6);
    assert.deepEqual(legacy.snapshot(users[1]),frozen);
    assert.equal(observationsFor(users[1]),4);
    assert.deepEqual(await report(adminPage,'genres',genre),enrolled);
    for(const kind of ['genres','tags']) pendingDelta(missingBefore[kind],await report(adminPage,kind,null),1);
    await privateResponse(await adminPage.goto(base+reportPath('genres',genre)+'&book_id='+first.book),400);
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${users[0]} AND role='administrator'`);
    await privateResponse(await adminPage.goto(base+reportPath('genres',genre)+'&days=bad'),403);
    assert.equal(await adminPage.getByTestId('audience-group-returns-current').count(),0);
    const anonymous=await browser.newPage();
    await privateResponse(await anonymous.goto(base+reportPath('genres',genre)),401);
    sql(`UPDATE account_roles SET is_active=true WHERE user_id=${users[0]} AND role='administrator'`);
    assert.deepEqual(await report(adminPage,'genres',genre),enrolled,'restored role works on the same session');
    assert.deepEqual(errors,[]);
    console.log('FLOW group returns browser PASS: labelled legacy groups -> exact books -> book history, real CAS/manual/device progress without capture, duplicate/overlapping/missing labels, retired consent410/no writes, periods, current metadata/hide, live admin, 390/1440');
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
    legacy.removeCoverage(coverage);
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
