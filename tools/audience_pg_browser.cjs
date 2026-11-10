'use strict';
// FLOW: actual account forms, C progress, administrator page and two viewports.
// Reuses the already running isolated PostgreSQL/Chromium acceptance server.
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
  throw new Error('Audience acceptance requires disposable loopback tor_ci');
}
fs.mkdirSync(shots, {recursive:true});

function sql(statement) {
  return execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1',
    '--dbname',db,'--command',statement], {encoding:'utf8'}).trim();
}
function scalar(statement) {
  const raw = sql(statement);
  assert.match(raw, /^\d+$/, 'expected scalar database fact');
  return Number(raw);
}
async function register(page, email) {
  await page.goto(base+'/register');
  await page.getByLabel('Отображаемое имя').fill('Проверка аудитории');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('AudienceBrowserPassword2026');
  await page.getByRole('button', {name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
}
async function privateResponse(response, status) {
  assert.equal(response.status(), status);
  const headers = await response.allHeaders();
  assert.equal(headers['cache-control'], 'no-store');
  assert.match(headers['x-robots-tag'], /noindex/);
  assert.match(headers['x-robots-tag'], /nofollow/);
}
async function numbers(page, period='current') {
  return page.getByTestId('audience-'+period).evaluate(section => {
    const value = metric => {
      const element = section.querySelector(`[data-metric="${metric}"]`);
      // Missing measurements stay nonnumeric in the page; only this test's
      // before/after arithmetic needs a zero baseline for an empty source.
      return element ? Number(element.textContent.trim()) : 0;
    };
    return {starts:value('starts'), completions:value('completions'), readers:value('readers')};
  });
}
async function periodBoundaries(page, days) {
  const result = await page.evaluate(() => {
    const times = period => Array.from(document.querySelectorAll(
      `[data-testid="audience-${period}"] time[datetime]`
    ), node => Date.parse(node.getAttribute('datetime')));
    return {current:times('current'), previous:times('previous')};
  });
  assert.equal(result.current.length, 2);
  assert.equal(result.previous.length, 2);
  assert.equal(result.current[0], result.previous[1], 'comparison intervals must touch');
  for (const pair of [result.current, result.previous]) {
    assert.equal(pair[1]-pair[0], days*86400000, 'period must have exact duration');
  }
}

(async () => {
  const run = Date.now()+'-'+randomBytes(4).toString('hex');
  const adminEmail = `audience-admin-${run}@tor-ci.example`;
  const readerEmail = `audience-reader-${run}@tor-ci.example`;
  let browser, book, administrator, reader;
  try {
    browser = await chromium.launch({headless:true});
    const adminContext = await browser.newContext({viewport:{width:1440,height:900}});
    const readerContext = await browser.newContext({viewport:{width:390,height:844}});
    const guestContext = await browser.newContext({viewport:{width:390,height:844}});
    const adminPage = await adminContext.newPage();
    const readerPage = await readerContext.newPage();
    const guestPage = await guestContext.newPage();
    const errors = [];
    adminPage.on('pageerror', error => errors.push(error.message));
    await register(adminPage, adminEmail);
    await register(readerPage, readerEmail);
    administrator = scalar(`SELECT id FROM users WHERE email_normalized='${adminEmail}'`);
    reader = scalar(`SELECT id FROM users WHERE email_normalized='${readerEmail}'`);
    sql(`INSERT INTO account_roles(user_id,role) VALUES(${administrator},'administrator')`);

    assert.equal((await adminPage.goto(base+'/me')).status(), 200);
    const audienceLink = adminPage.getByRole('link', {name:'Аудитория',exact:true});
    assert.ok(await audienceLink.isVisible(), 'administrator must discover the report in their account');
    await audienceLink.click();
    await adminPage.waitForURL(base+'/admin/audience');
    assert.equal((await readerPage.goto(base+'/me')).status(), 200);
    assert.equal(await readerPage.locator('a[href="/admin/audience"]').count(), 0,
      'reader account must not advertise the administrator report');

    await privateResponse(await guestPage.goto(base+'/admin/audience'), 401);
    assert.equal(await guestPage.locator('[data-metric]').count(), 0);
    await privateResponse(await readerPage.goto(base+'/admin/audience'), 403);
    assert.equal(await readerPage.locator('[data-metric]').count(), 0);
    await privateResponse(await adminPage.goto(base+'/admin/audience?days=7'), 200);
    const before = await numbers(adminPage);

    // Public reading does not need an owner or unrelated immutable share history.
    book = scalar(`INSERT INTO books(source_identity,target_language,title,visibility,access_level)
      VALUES('ci:audience:browser:${run}','ru','AUDIENCE-PRIVATE-SENTINEL-${run}','public','basic') RETURNING id`);
    const chapter = scalar(`INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
      VALUES(${book},1,'Audience browser chapter','public','free',0) RETURNING id`);
    const version = scalar(`INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
      VALUES(${chapter},1,'Audience text') RETURNING id`);
    sql(`INSERT INTO chapter_publications(chapter_id,version_id) VALUES(${chapter},${version})`);
    assert.equal((await readerPage.goto(base+`/reader/${chapter}`)).status(), 200);
    const csrf = (await readerContext.cookies()).find(cookie => cookie.name === 'tor-dev-csrf')?.value;
    assert.ok(csrf, 'normal account UI must issue the CSRF cookie');
    await readerPage.evaluate(async ({chapter,version,csrf}) => {
      async function save(offset, revision, expected=200) {
        const response = await fetch('/api/v1/me/progress/free', {
          method:'PUT', credentials:'same-origin',
          headers:{'Content-Type':'application/json','X-CSRF-Token':csrf},
          body:JSON.stringify({chapter_id:String(chapter), version_id:String(version),
            block_id:1, char_offset:offset, expected_revision:revision}),
        });
        if (response.status !== expected) throw new Error('C progress status: '+response.status+', expected '+expected);
        return expected === 200 ? (await response.json()).revision : null;
      }
      const first = await save(3, 0);
      const finished = await save(13, first);
      await save(13, finished, 409);
    }, {chapter,version,csrf});
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_starts WHERE user_id=${reader} AND chapter_id=${chapter}`), 1);
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_completions WHERE user_id=${reader} AND chapter_id=${chapter}`), 1);
    const event = sql(`SELECT source_event_id FROM reader_chapter_completions WHERE user_id=${reader} AND chapter_id=${chapter}`);
    sql(`UPDATE books SET admin_hold=true WHERE id=${book}`);

    for (const width of [390,1440]) {
      await adminPage.setViewportSize({width,height:width === 390 ? 844 : 900});
      await privateResponse(await adminPage.goto(base+'/admin/audience?days=7'), 200);
      const after = await numbers(adminPage);
      for (const field of ['starts','completions','readers']) {
        assert.equal(after[field], before[field]+1, `C ${field} must count the real fact once`);
      }
      await periodBoundaries(adminPage, 7);
      const html = await adminPage.content();
      for (const forbidden of ['AUDIENCE-PRIVATE-SENTINEL', readerEmail, 'Audience text', event]) {
        assert.ok(!html.includes(forbidden), 'aggregate page exposed a source identity or content');
      }
      assert.ok(await adminPage.evaluate(() => document.documentElement.scrollWidth <= innerWidth+1),
        'audience page has horizontal overflow');
      await adminPage.screenshot({path:path.join(shots,`audience-${width}.png`),fullPage:true});
      await adminPage.locator('select[name="days"]').selectOption('30');
      await adminPage.locator('form[action="/admin/audience"]').getByRole('button').click();
      await adminPage.waitForURL(url => url.pathname==='/admin/audience' && url.searchParams.get('days')==='30');
      await periodBoundaries(adminPage, 30);
    }
    for (const days of [1,90]) {
      await privateResponse(await adminPage.goto(base+`/admin/audience?days=${days}`), 200);
      await periodBoundaries(adminPage, days);
      assert.equal(await adminPage.locator('select[name="days"]').inputValue(), String(days));
    }
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${administrator} AND role='administrator'`);
    await privateResponse(await adminPage.goto(base+'/admin/audience'), 403);
    assert.equal(await adminPage.locator('[data-metric]').count(), 0,
      'revoked administrator must not receive audience data');
    assert.equal((await adminPage.goto(base+'/me')).status(), 200);
    assert.equal(await adminPage.locator('a[href="/admin/audience"]').count(), 0,
      'revoked administrator must lose the report link without losing their session');
    assert.deepEqual(errors, []);
    console.log('FLOW audience browser PASS: actual C progress/replay, live admin role/revocation, private aggregates, 1/7/30/90 periods, 390/1440 px');
  } finally {
    if (browser) await browser.close();
    if (administrator) sql(`DELETE FROM account_roles WHERE user_id=${administrator}`);
    if (book) sql(`BEGIN;
      DELETE FROM reader_book_positions WHERE book_id=${book};
      DELETE FROM reader_chapter_starts WHERE book_id=${book};
      DELETE FROM reader_chapter_completions WHERE book_id=${book};
      DELETE FROM chapter_publications WHERE chapter_id IN(SELECT id FROM chapters WHERE book_id=${book});
      DELETE FROM book_members WHERE book_id=${book};
      DELETE FROM books WHERE id=${book};
      COMMIT`);
    sql(`DELETE FROM users WHERE email_normalized IN('${adminEmail}','${readerEmail}')`);
  }
})().catch(error => {console.error(error);process.exitCode=1;});
