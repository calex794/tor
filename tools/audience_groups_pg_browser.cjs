'use strict';
// Retained genre/tag aggregates and drill-downs are explicitly historical.
// Real ordinary navigation and C progress must not create new optional observations.
const {chromium} = require('playwright');
const {execFileSync} = require('node:child_process');
const {randomBytes} = require('node:crypto');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const preview = require('./ci_browser_preview.cjs');

const db = process.env.APP_URL || '';
const base = process.env.TOR_AUDIENCE_BASE || 'http://127.0.0.1:3108';
const shots = process.env.TOR_AUDIENCE_SHOTS || '/tmp/audience-browser-shots';
const parsed = new URL(db);
if (parsed.protocol !== 'postgresql:' || parsed.hostname !== '127.0.0.1'
    || parsed.port !== '5432' || parsed.pathname !== '/tor_ci'
    || !/^http:\/\/127\.0\.0\.1:\d+$/.test(base)) {
  throw new Error('Group-report acceptance requires disposable loopback tor_ci');
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
async function register(page, email) {
  await page.goto(base+'/register');
  await page.getByLabel('Отображаемое имя').fill('Проверка направлений');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('AudienceGroupsPassword2026');
  await page.getByRole('button', {name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
  return scalar(`SELECT id FROM users WHERE email_normalized='${email}'`);
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
  assert.equal((await page.goto(base+`/reader/${fixture.chapter}`)).status(), 200);
  await page.waitForFunction(() => document.querySelector('[data-free-progress-chapter]')?.dataset.freeProgressReady === '1');
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
const retired = require('./audience_retired_pg_helpers.cjs')({sql,scalar,base});
const navigate = retired.navigate;

async function values(locator) {
  const result = {};
  for (const metric of metrics) {
    const text = (await locator.locator(`[data-metric='${metric}']`).innerText()).trim();
    assert.ok(/^\d+$/.test(text) || text === 'Нет данных',text);
    result[metric] = /^\d+$/.test(text) ? Number(text) : null;
  }
  return result;
}
function group(page, label) {
  // CSS escaping preserves literal &, +, quotes and Cyrillic metadata.
  const quoted = JSON.stringify(label);
  return page.locator(`[data-testid='audience-group-row'][data-label=${quoted}]`);
}
async function select(page, kind, days) {
  await page.locator('#groups-kind').selectOption(kind);
  await page.locator('#groups-days').selectOption(String(days));
  await page.locator('#audience-groups-filters').getByRole('button',{name:'Показать',exact:true}).click();
  await page.waitForURL(url => url.pathname === '/admin/audience/groups'
    && url.searchParams.get('kind') === kind && url.searchParams.get('days') === String(days));
}
async function periods(page, days) {
  const bounds = {};
  for (const period of ['current','previous']) {
    bounds[period] = (await page.getByTestId('audience-groups-'+period).innerText())
      .match(/\b\d{4}-\d{2}-\d{2}\b/g) || [];
    assert.equal(bounds[period].length,2);
    assert.equal(Date.parse(bounds[period][1])-Date.parse(bounds[period][0]),days*86400000);
  }
  assert.equal(bounds.previous[1],bounds.current[0]);
  assert.equal(bounds.current[1],sql("SELECT ((now() AT TIME ZONE 'UTC')::date+1)::text"));
}
async function detail(page, kind, label, days) {
  const row = group(page,label);
  assert.equal(await row.count(),1,'group is available');
  const bookCount=await row.getByTestId('audience-group-book-count').innerText();
  const before = {};
  const states = {};
  for (const p of ['current','previous']) {
    before[p]=await values(row.locator(`[data-period='${p}']`));
    states[p]=await row.locator(`[data-period='${p}'] [data-state]`).allTextContents();
  }
  await row.getByTestId('audience-group-books').click();
  await page.waitForURL(url => url.pathname === '/admin/audience/books');
  const url = new URL(page.url());
  assert.equal(url.searchParams.get('days'),String(days));
  const name = kind === 'genres' ? 'genre' : 'tag';
  assert.equal(url.searchParams.get(label ? name : 'without_'+name),label || 'true');
  assert.equal(await page.getByTestId('audience-books-count').innerText(),bookCount,'same complete public book count');
  for (const p of ['current','previous']) {
    assert.deepEqual(await values(page.getByTestId('audience-books-'+p)),before[p],kind+' detail '+label);
    assert.deepEqual(await page.getByTestId('audience-books-'+p).locator('[data-state]').allTextContents(),states[p],'same book states');
  }
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth+1),'detail fits viewport');
  return before;
}

(async () => {
  const run = Date.now()+'-'+randomBytes(4).toString('hex');
  const genreA = '!жанр '+run, genreB = '!повесть '+run;
  const tagA = '!метка + "'+run+'"', tagB = '!дорога '+run;
  const secret = '!скрытый '+run;
  const emails = ['admin','reader'].map(role => `flow-groups-${role}-${run}@tor-ci.example`);
  const fixtures = [], users = [], legacyTokens = [], errors = [];
  let browser;
  const array = labels => labels.length ? 'ARRAY['+labels.map(value => "'"+value.replaceAll("'","''")+"'").join(',')+']' : "'{}'::text[]";
  function create(key,title,genres,tags) {
    const book = scalar(`INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres,tags)
      VALUES('ci:flow:groups:${run}:${key}','ru','${title}','public','basic',${array(genres)},${array(tags)}) RETURNING id`);
    const fixture={key,book,title}; fixtures.push(fixture);
    Object.assign(fixture,chapter(book,1));
    return fixture;
  }
  try {
    const search = create('search','Книга из поиска '+run,[genreA,genreB,genreA],[tagA,tagB,tagA]);
    const recommended = create('recommendations','Книга из рекомендации '+run,[genreA],[tagA]);
    const direct = create('direct','Книга без жанра '+run,[],[tagB]);
    const unread = create('unread','Книга без тега '+run,[genreB],[]);
    const hidden = create('hidden','HIDDEN-GROUP-SENTINEL-'+run,[secret],[secret]);
    sql(`UPDATE books SET admin_hold=true WHERE id=${hidden.book}`);
    browser = await chromium.launch({headless:true});
    const pages = [];
    for (const [index,email] of emails.entries()) {
      const context = await browser.newContext({viewport:{width:index === 0 ? 1440 : 390,height:900}});
      const page = await context.newPage();
      page.on('pageerror',error => errors.push(error.message));
      users.push(await register(page,email)); pages.push(page);
    }
    const [adminPage,readerPage]=pages;
    sql(`INSERT INTO account_roles(user_id,role) VALUES(${users[0]},'administrator')`);
    await privateResponse(await readerPage.goto(base+'/admin/audience/groups?kind=bad'),403);
    assert.equal(await readerPage.getByTestId('audience-group-row').count(),0);
    retired.oldScopes(users.slice(1));
    const scope = retired.scopeSnapshot(users);
    await retired.settings(readerPage);
    legacyTokens.push(await retired.rejectedLegacyCommands(readerPage));
    retired.history([
      {book:search.book,source:'search',issued:2,clicks:1,opens:1,starts:1,completions:1},
      {book:recommended.book,source:'recommendations',impressions:2,clicks:1,opens:1,starts:1,completions:1},
      {book:direct.book,source:'unattributed',starts:1,completions:1},
      {book:unread.book,source:'catalog',issued:1},
    ]);
    retired.noCapture(users,fixtures.map(item=>item.book));
    await navigate(readerPage,search,'search'); await read(readerPage,search);
    await navigate(readerPage,recommended,'recommendations'); await read(readerPage,recommended);
    await readerPage.goto(base+`/books/${direct.book}`); await read(readerPage,direct);
    await navigate(readerPage,unread,'catalog');
    await readerPage.goto(base+'/me'); await readerPage.waitForLoadState('networkidle');
    await retired.settled();
    retired.noCapture(users,fixtures.map(item=>item.book));
    const zero = create('zero','Книга без наблюдений '+run,[genreA],[tagA]);
    await adminPage.goto(base+'/me');
    await adminPage.locator('a[href="/admin/audience"]').click();
    await adminPage.waitForURL('**/admin/audience');
    await adminPage.getByTestId('audience-groups-link').click();
    await adminPage.waitForURL(url => url.pathname === '/admin/audience/groups');
    await select(adminPage,'genres',7); await periods(adminPage,7);
    await retired.marked(adminPage,['audience-groups-current','audience-groups-previous']);
    const baseline = {};
    for (const [kind,labels] of [['genres',[genreA,genreB,'']],['tags',[tagA,tagB,'']]]) {
      await select(adminPage,kind,7);
      baseline[kind]={};
      for (const label of labels) {
        baseline[kind][label]=await values(group(adminPage,label).locator('[data-period="current"]'));
      }
    }
    assert.equal(baseline.genres[genreA].starts,2,'duplicate labels and tags do not multiply first readings');
    assert.equal(baseline.genres[genreB].starts,1);
    assert.equal(baseline.tags[tagA].starts,2);
    assert.equal(baseline.tags[tagB].starts,2);
    assert.ok(baseline.genres[genreA].impressions>0,'observed recommendation retained');
    assert.ok(baseline.genres[genreA].issued>0,'issued search links retained');
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_starts WHERE user_id=${users[1]}`),3);
    await select(adminPage,'genres',7);
    assert.equal(await group(adminPage,genreA).locator('[data-period="current"] [data-state="with-starts"]').innerText(),'2');
    assert.equal(await group(adminPage,genreA).locator('[data-period="current"] [data-state="no-observations"]').innerText(),'1');
    assert.equal(await group(adminPage,genreB).locator('[data-period="current"] [data-state="observed-without-starts"]').innerText(),'1');
    for (const width of [390,1440]) {
      await adminPage.setViewportSize({width,height:width===390 ? 844 : 900});
      for (const [kind,labels] of [['genres',[genreA,genreB,'']],['tags',[tagA,tagB,'']]]) {
        await privateResponse(await adminPage.goto(base+`/admin/audience/groups?kind=${kind}&days=7`));
        const html=await adminPage.content();
        for (const forbidden of [secret,'HIDDEN-GROUP-SENTINEL','Readable text',...emails,...legacyTokens]) assert.ok(!html.includes(forbidden),'no private metadata or identity');
        assert.ok(await adminPage.evaluate(() => document.documentElement.scrollWidth <= innerWidth+1),'groups fit viewport');
        await retired.marked(adminPage,['audience-groups-current','audience-groups-previous']);
        await adminPage.screenshot({path:path.join(shots,`audience-groups-${kind}-${width}.png`),fullPage:true});
        await preview(adminPage,`audience-groups-${kind}-${width}`);
        for (const label of labels) {
          const groupsUrl=adminPage.url();
          assert.deepEqual(await values(group(adminPage,label).locator('[data-period="current"]')),baseline[kind][label],'repeat comparison is read-only');
          const shown=await detail(adminPage,kind,label,7);
          assert.deepEqual(shown.current,baseline[kind][label]);
          await privateResponse(await adminPage.goto(groupsUrl));
        }
      }
    }
    for (const days of [1,30,90]) {
      await select(adminPage,'genres',days); await periods(adminPage,days);
      assert.equal((await detail(adminPage,'genres',genreA,days)).current.starts,2);
      await adminPage.goto(base+'/admin/audience/groups?kind=genres&days='+days);
    }
    // Moving the current tag moves existing anonymous facts; hiding removes them.
    sql(`UPDATE books SET tags=ARRAY['${tagB}'] WHERE id=${recommended.book}`);
    await privateResponse(await adminPage.goto(base+'/admin/audience/groups?kind=tags&days=7'));
    assert.equal((await values(group(adminPage,tagA).locator('[data-period="current"]'))).starts,1);
    assert.equal((await values(group(adminPage,tagB).locator('[data-period="current"]'))).starts,3);
    await detail(adminPage,'tags',tagB,7);
    sql(`UPDATE books SET admin_hold=true WHERE id=${recommended.book}`);
    await privateResponse(await adminPage.goto(base+'/admin/audience/groups?kind=tags&days=7'));
    assert.equal((await values(group(adminPage,tagB).locator('[data-period="current"]'))).starts,2);
    // Untouched book remains visible without any invented measured zero.
    await adminPage.goto(base+'/admin/audience/books?genre='+encodeURIComponent(genreA)+'&days=7');
    assert.ok(Object.values(await values(adminPage.locator(`[data-book-id='${zero.book}'] [data-period='current']`))).every(value=>value===null));
    await retired.settled();
    retired.noCapture(users,fixtures.map(item=>item.book));
    assert.equal(retired.scopeSnapshot(users),scope,'ordinary activity does not mutate old scopes');
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${users[0]} AND role='administrator'`);
    await privateResponse(await adminPage.goto(base+'/admin/audience/groups?kind=genres&days=7'),403);
    assert.equal(await adminPage.getByTestId('audience-group-row').count(),0);
    await privateResponse(await adminPage.goto(base+'/admin/audience/books?without_genre=true&days=7'),403);
    assert.deepEqual(errors,[]);
    console.log('FLOW groups browser PASS: labelled historical fixtures, real ordinary C reads without capture, overlapping and missing labels, exact details, periods, current visibility, live admin, 390/1440');
  } finally {
    if(browser) await browser.close();
    if(users.length) sql(`DELETE FROM account_roles WHERE user_id IN(${users.join(',')})`);
    if(fixtures.length) {
      const ids=fixtures.map(item=>item.book).join(',');
      sql(`BEGIN;
        DELETE FROM reader_book_positions WHERE book_id IN(${ids}); DELETE FROM reader_chapter_starts WHERE book_id IN(${ids});
        DELETE FROM reader_chapter_completions WHERE book_id IN(${ids});
        DELETE FROM chapter_publications WHERE chapter_id IN(SELECT id FROM chapters WHERE book_id IN(${ids}));
        DELETE FROM book_members WHERE book_id IN(${ids}); DELETE FROM books WHERE id IN(${ids});
        DELETE FROM rec_quality_daily WHERE book_id IN(${ids}); DELETE FROM organic_discovery_quality_daily WHERE book_id IN(${ids});
        DELETE FROM rec_dirty_buckets WHERE book_id IN(${ids}); DELETE FROM organic_discovery_dirty WHERE book_id IN(${ids}); COMMIT`);
    }
    sql(`DELETE FROM users WHERE email_normalized IN(${emails.map(email=>`'${email}'`).join(',')})`);
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
