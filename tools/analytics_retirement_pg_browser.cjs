'use strict';
// Real HTTP and Chromium against the database upgraded by the migration acceptance target.
const {chromium} = require('playwright');
const {execFileSync} = require('node:child_process');
const {randomBytes} = require('node:crypto');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const preview = require('./ci_browser_preview.cjs');

const db = process.env.COLLECTION_RETIREMENT_APP_DATABASE_URL;
const base = process.env.TOR_AUDIENCE_BASE || 'http://127.0.0.1:3108';
const shots = process.env.TOR_AUDIENCE_SHOTS || '/tmp/audience-browser-shots';
assert.match(base, /^http:\/\/127\.0\.0\.1:\d+$/);
function sql(statement) {
  return execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1',
    '--dbname',db,'--command',statement], {
    encoding:'utf8',env:{...process.env,PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'},
  }).trim();
}
const fixture = require('./analytics_retirement_fixture.cjs')(sql);
const {linked,untouched,queued} = fixture.byKey;
const quote = value => "'"+value.replaceAll("'","''")+"'";
const array = values => 'ARRAY['+values.map(quote).join(',')+']::text[]';
const common = 'проверка истории';
const intact = 'сохранённая история';
const incomplete = 'неполная история';
const tag = 'после удаления';
const keys = ['issued','impressions','clicks','book-opens','starts'];
const retained = ['22','13','15','15','14'];
const unknown = keys.map(() => 'Нет данных');
const paths = ['/admin/audience?days=7','/admin/audience/books?days=7',
  '/admin/audience/groups?days=7&kind=genres','/admin/audience/visits?days=7'];
const booksPath = '/admin/audience/books?days=7&genre='+encodeURIComponent(common);
const groupsPath = '/admin/audience/groups?days=7&kind=genres';
const email = 'retirement-browser-'+randomBytes(8).toString('hex')+'@example.invalid';
let browser, user;
fs.mkdirSync(shots, {recursive:true});

async function privateResponse(response, status=200) {
  assert.equal(response.status(), status);
  const headers = await response.allHeaders();
  assert.equal(headers['cache-control'], 'no-store');
  assert.match(headers['x-robots-tag'], /noindex/);
  assert.match(headers['x-robots-tag'], /nofollow/);
}
async function measurements(root, expected) {
  for (const [index,key] of keys.entries()) {
    assert.equal(await root.locator('[data-metric="'+key+'"]').innerText(), expected[index], key);
  }
}
function bookRow(page, book) {
  return page.locator('[data-testid="audience-book-row"][data-book-id="'+book.id+'"]');
}
function groupRow(page, label) {
  return page.getByTestId('audience-group-row').filter({
    has:page.locator('h3').filter({hasText:new RegExp('^'+label+'$')}),
  });
}
async function sourceUnknown(page) {
  await privateResponse(await page.goto(base+paths[0]));
  const current = page.getByTestId('audience-sources-current');
  for (const source of ['recommendations','catalog','search','unattributed']) {
    const card = current.locator('[data-source="'+source+'"]');
    assert.equal(await card.locator('[data-evidence="invalidated"]').count(), 1);
    for (const metric of ['clicks','book-opens','starts','completions']) {
      const expected = source==='unattributed' && ['clicks','book-opens'].includes(metric)
        ? 'Не измеряется' : 'Нет данных';
      assert.equal(await card.locator('[data-metric="'+metric+'"]').innerText(), expected);
    }
  }
}
async function proof(page, name, target) {
  await target.scrollIntoViewIfNeeded();
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth+1));
  assert.equal(await page.getByTestId('collection-retired').count(), 1);
  assert.equal(await page.locator('[data-current-measurement="unavailable"]').innerText(), 'Не измеряется');
  const html = await page.content();
  assert.ok(!html.includes('retirement-main@ci.invalid'));
  assert.ok(!html.includes(email));
  assert.ok(!html.includes('subject_id'));
  assert.ok(!html.includes('session_digest'));
  await page.screenshot({path:path.join(shots,name+'.png'),fullPage:true,animations:'disabled'});
  await preview(page,name);
}
async function denied(page, status) {
  for (const route of paths) {
    await privateResponse(await page.goto(base+route),status);
    assert.equal(await page.locator('[data-metric]').count(),0);
  }
}
(async () => {
  try {
    for (const [book,title,genres] of [
      [linked,'Книга с удалёнными исходными данными',[common,incomplete]],
      [untouched,'Книга с сохранёнными измерениями',[common,intact]],
      [queued,'Книга с неполной историей периода',[common,incomplete]],
    ]) {
      sql(`UPDATE books SET title=${quote(title)},genres=${array(genres)},tags=${array([tag])}
        WHERE id=${book.id}`);
    }
    browser = await chromium.launch({headless:true});
    const context = await browser.newContext({viewport:{width:1440,height:900},
      userAgent:'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36'});
    const page = await context.newPage();
    const errors = [], requests = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('request', request => {
      if (/\/api\/v1\/(?:recommendations\/(?:impressions|activate)|discovery\/activate)$/.test(new URL(request.url()).pathname)) {
        requests.push(request.url());
      }
    });
    await denied(page,401);
    await page.goto(base+'/register');
    await page.getByLabel('Отображаемое имя').fill('Проверка сохранённых измерений');
    await page.getByLabel('Электронная почта').fill(email);
    await page.getByLabel('Пароль (12–128 символов)').fill('RetirementBrowserPassword2026');
    await page.getByRole('button',{name:'Создать учётную запись'}).click();
    await page.waitForURL('**/me');
    user = Number(sql('SELECT id FROM users WHERE email_normalized='+quote(email)));
    assert.ok(Number.isSafeInteger(user) && user>0);
    await denied(page,403);
    sql(`INSERT INTO account_roles(user_id,role) VALUES(${user},'administrator')`);

    for (const width of [390,1440]) {
      await page.setViewportSize({width,height:900});
      await sourceUnknown(page);
      await proof(page,'analytics-retirement-sources-'+width,page.getByTestId('audience-sources-current'));

      await privateResponse(await page.goto(base+booksPath));
      assert.equal(await page.getByTestId('audience-books-count').innerText(),'3');
      const totals = page.getByTestId('audience-books-current');
      await measurements(totals,unknown);
      assert.equal(await totals.locator('[data-state="unknown-evidence"]').innerText(),'2');
      assert.equal(await totals.locator('[data-state="with-starts"]').innerText(),'1');
      for (const book of [linked,queued]) {
        const current = bookRow(page,book).locator('[data-period="current"]');
        assert.equal(await current.getAttribute('data-state'),'unknown-evidence');
        await measurements(current,unknown);
        assert.equal(await current.locator('[data-evidence="invalidated"]').count(),1);
      }
      const kept = bookRow(page,untouched).locator('[data-period="current"]');
      assert.equal(await kept.getAttribute('data-state'),'with-starts');
      await measurements(kept,retained);
      assert.equal(await kept.locator('[data-evidence="invalidated"]').count(),0);
      await proof(page,'analytics-retirement-books-'+width,totals);

      await privateResponse(await page.goto(base+groupsPath));
      await measurements(groupRow(page,common).locator('[data-period="current"]'),unknown);
      await measurements(groupRow(page,intact).locator('[data-period="current"]'),retained);
      assert.equal(await groupRow(page,incomplete).locator('[data-period="current"] [data-state="unknown-evidence"]').innerText(),'2');
      await proof(page,'analytics-retirement-groups-'+width,groupRow(page,common));
    }

    // A marker only invalidates the selected source/book/date window.
    await privateResponse(await page.goto(base+booksPath.replace('days=7','days=1')));
    assert.equal(await page.getByTestId('audience-books-current').locator('[data-evidence="invalidated"]').count(),0);
    for (const book of [linked,untouched,queued]) {
      assert.equal(await bookRow(page,book).locator('[data-period="current"]').getAttribute('data-state'),'no-observations');
    }
    await privateResponse(await page.goto(base+groupsPath));
    await groupRow(page,intact).getByTestId('audience-group-books').click();
    assert.equal(await page.getByTestId('audience-books-count').innerText(),'1');
    await measurements(page.getByTestId('audience-books-current'),retained);

    // Book/group reports apply current B visibility before sums and invalidation.
    // The global source report retains its historical, catalogue-independent scope.
    sql(`UPDATE books SET admin_hold=true WHERE id IN(${linked.id},${queued.id})`);
    await privateResponse(await page.goto(base+booksPath));
    assert.equal(await page.getByTestId('audience-books-count').innerText(),'1');
    await measurements(page.getByTestId('audience-books-current'),retained);
    assert.equal(await page.getByTestId('audience-books-current').locator('[data-evidence="invalidated"]').count(),0);
    assert.equal(await bookRow(page,linked).count(),0);
    assert.equal(await bookRow(page,queued).count(),0);
    await sourceUnknown(page);
    await privateResponse(await page.goto(base+paths[3]));
    const visitCurrent = page.getByTestId('flow-visits-current');
    for (const metric of ['unique-subjects','unattributed-pageviews']) {
      assert.equal(await visitCurrent.locator('[data-metric="'+metric+'"]').innerText(),'Нет измерения');
    }
    fixture.noRaw();
    assert.deepEqual(fixture.snapshot(),fixture.before,'report reads must not mutate retained evidence, C or P1');
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${user} AND role='administrator'`);
    await denied(page,403);
    assert.deepEqual(errors,[]);
    assert.deepEqual(requests,[]);
    console.log('Installed analytics retirement browser PASS: actual 190->193 evidence, per-period unknown/reason, untouched values, groups/detail, live visibility and role revocation, private401/403, no raw/C/P1 mutation, 390/1440');
  } finally {
    if (browser) await browser.close();
    for (const book of fixture.books) {
      sql(`UPDATE books SET title=${quote(book.title)},genres=${array(book.genres)},
        tags=${array(book.tags)},admin_hold=${book.admin_hold?'true':'false'} WHERE id=${book.id}`);
    }
    if (user) {
      sql(`DELETE FROM account_roles WHERE user_id=${user}; DELETE FROM users WHERE id=${user}`);
    }
  }
})().catch(error => {console.error(error);process.exitCode=1;});
