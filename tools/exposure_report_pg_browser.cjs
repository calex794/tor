'use strict';
// Real PostgreSQL, production server, native forms without JavaScript.
const {chromium} = require('playwright');
const {execFileSync} = require('node:child_process');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const db = process.env.APP_URL || '';
const base = process.env.TOR_EXPOSURE_BASE || 'http://127.0.0.1:3113';
const shots = process.env.TOR_EXPOSURE_SHOTS || '/tmp/exposure-report-shots';
const parsed = new URL(db);
const port = process.env.FLOW_TEST_PG_PORT || '5432';
assert.match(port, /^[1-9][0-9]{0,4}$/);
assert.ok(Number(port) <= 65535);
assert.equal(parsed.protocol, 'postgresql:');
assert.equal(parsed.hostname, '127.0.0.1');
assert.equal(parsed.port, port);
assert.equal(parsed.pathname, '/tor_ci');
assert.equal(parsed.username, 'tor_app');
assert.match(base, /^http:\/\/127\.0\.0\.1:\d+$/);
fs.mkdirSync(shots, {recursive: true});
function sql(statement) {
  return execFileSync('psql', ['-XAtq', '-v', 'ON_ERROR_STOP=1', '--dbname', db, '-c', statement],
    {encoding: 'utf8', env: {...process.env, PGOPTIONS: '-c statement_timeout=5000 -c lock_timeout=2000'}}).trim();
}
async function privateResponse(response, expected = 200) {
  assert.equal(response.status(), expected);
  const headers = await response.allHeaders();
  assert.equal(headers['cache-control'], 'no-store');
  assert.match(headers['x-robots-tag'], /noindex/);
}
(async () => {
  const stamp = Date.now();
  const prefix = 'ci:flow-report-browser:' + stamp + ':';
  const label = 'FlowGenre' + stamp;
  let browser, actor;
  try {
    assert.equal(sql('SELECT count(*) FROM _sqlx_migrations WHERE version=202611294684 AND success'), '1');
    const low = label + 'zzz-low <script>';
    const unknown = label + 'zzzz-unknown';
    // Respect the production per-book genre limit: 101 ordinary books, one label each.
    const normal = sql("INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres)" +
      " SELECT '" + prefix + "normal-'||n,'ru','" + label + " normal '||n,'public','basic'," +
      "ARRAY['" + label + "a'||lpad(n::text,3,'0')] FROM generate_series(0,100) n RETURNING id")
      .split(/\s+/).map(Number);
    assert.equal(normal.length, 101);
    assert.ok(normal.every(id => Number.isSafeInteger(id) && id > 0));
    const first = normal[0];
    const lowBook = Number(sql("INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres) VALUES('" +
      prefix + "low','ru','" + label + " low <script>safe</script>','public','basic',ARRAY['" + low + "']) RETURNING id"));
    sql("INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres) VALUES('" +
      prefix + "unknown','ru','" + label + " unknown','public','basic',ARRAY['" + unknown + "']);" +
      "INSERT INTO flow_observation_hourly(bucket,book_id,surface,opportunities,rendered,views)" +
      " SELECT date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'-interval '2 hours',id,'home',20,20," +
      "CASE WHEN id=" + lowBook + " THEN 0 ELSE 6 END FROM books WHERE source_identity LIKE '" + prefix +
      "%' AND source_identity<>'" + prefix + "unknown'");
    sql("UPDATE books SET tags=ARRAY['" + label + "'] WHERE source_identity LIKE '" + prefix + "%'");
    sql("INSERT INTO flow_observation_hourly(bucket,book_id,surface,campaign_id,slot,opportunities,rendered,views,clicks)" +
      " VALUES(date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'-interval '2 hours'," +
      normal[1] + ",'home',891,'home_featured',3,2,1,1)");
    assert.ok(first > 0 && lowBook > 0);
    browser = await chromium.launch({headless: true});
    const context = await browser.newContext({javaScriptEnabled: false, viewport: {width: 1440, height: 900}});
    const page = await context.newPage();
    await privateResponse(await page.goto(base + '/admin/audience/exposure?window=closed'), 401);
    await page.goto(base + '/register');
    const email = 'flow-report-' + stamp + '@tor-ci.example';
    await page.getByLabel('Отображаемое имя').fill('Report operator');
    await page.getByLabel('Электронная почта').fill(email);
    await page.getByLabel('Пароль (12–128 символов)').fill('ReportBrowserPassword2026');
    await page.getByRole('button', {name: 'Создать учётную запись', exact: true}).click();
    await page.waitForURL('**/me');
    actor = Number(sql("SELECT id FROM users WHERE email_normalized='" + email + "'"));
    await privateResponse(await page.goto(base + '/admin/audience/exposure?window=closed'), 403);
    sql("INSERT INTO account_roles(user_id,role) VALUES(" + actor + ",'administrator')");
    await privateResponse(await page.goto(base + '/admin/audience/exposure?group=tag&label=' + encodeURIComponent(label)));
    assert.equal(await page.locator('[data-testid="exposure-health"]').count(), 0);
    await page.getByLabel('Граница периода').selectOption('closed');
    await page.getByLabel('Дней в периоде').selectOption('1');
    await page.getByRole('button', {name: 'Показать', exact: true}).click();
    await page.waitForURL(/window=closed/);
    assert.equal(await page.locator('[data-testid="exposure-closed-window"]').count(), 1);
    assert.equal(await page.locator('[data-testid="exposure-health-unknown"]').count(), 1);
    assert.equal(await page.locator('[data-testid="exposure-health-complete"]').count(), 0);
    const matrix = page.locator('[data-testid="attention-placements"]');
    const organicHome = matrix.locator('[data-attn-surface="home"][data-attn-source="organic"]');
    assert.equal(await organicHome.count(), 1, 'one aggregate-only home/organic row');
    assert.equal(await matrix.locator('[data-attn-surface="home"][data-attn-source="editorial"]').count(), 1);
    assert.match(await matrix.locator('[data-attn-source="editorial"]').innerText(), /кликов 1/);
    assert.match(await organicHome.innerText(), /показов/);
    assert.match(await organicHome.locator('a').getAttribute('href'), /source=organic/);
    const matrixText = await matrix.innerText();
    const summary = page.locator('[data-testid="exposure-genres"]');
    assert.equal(await summary.locator('[data-metric="genres"]').innerText(), '103');
    assert.equal(await summary.locator('[data-metric="low-genres"]').innerText(), '1');
    assert.equal(await summary.locator('[data-metric="unknown-genres"]').innerText(), '1');
    assert.equal(await summary.getByRole('link', {name: low, exact: true}).count(), 1);
    assert.ok(!(await page.content()).includes('<script>safe'));
    const summaryText = await summary.innerText();
    await page.locator('[data-testid="exposure-next"]').click();
    await page.waitForURL(/after=\d+/);
    assert.equal(new URL(page.url()).searchParams.get('window'), 'closed');
    assert.equal(await summary.innerText(), summaryText);
    assert.equal(await matrix.innerText(), matrixText, 'aggregate breakdown must not change with keyset pagination');
    await summary.getByRole('link', {name: low, exact: true}).click();
    await page.waitForURL(url => url.searchParams.get('label') === low);
    assert.equal(new URL(page.url()).searchParams.get('window'), 'closed');
    assert.equal(new URL(page.url()).searchParams.get('after'), null);
    assert.equal(await page.locator('[data-testid="exposure-row"]').count(), 1);
    assert.equal(await page.locator('[data-testid="exposure-class"]').innerText(), 'Мало подтверждённых показов');
    for (const width of [390, 1440]) {
      await page.setViewportSize({width, height: 900});
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
      await page.screenshot({path: path.join(shots, 'exposure-closed-genres-' + width + '.png'), fullPage: true, animations: 'disabled'});
      const previewName = 'exposure-closed-review-' + width + '.png';
      const preview = await page.screenshot({path: path.join(shots, previewName), fullPage: false, animations: 'disabled'});
      if (process.env.TOR_BROWSER_PROOF_IMAGES === '1') {
        console.log('FLOW_VISUAL_BEGIN ' + JSON.stringify({path: previewName, mime: 'image/png', bytes: preview.length, width, height: 900}));
        const encoded = preview.toString('base64');
        for (let offset = 0; offset < encoded.length; offset += 3000) console.log('FLOW_VISUAL_DATA ' + encoded.slice(offset, offset + 3000));
        console.log('FLOW_VISUAL_END');
      }
    }
    sql('UPDATE books SET admin_hold=true WHERE id=' + lowBook);
    await privateResponse(await page.reload());
    assert.equal(await page.locator('[data-testid="exposure-row"]').count(), 0);
    assert.equal(await summary.locator('[data-metric="genres"]').innerText(), '0');
    assert.equal(await page.locator('[data-testid="exposure-health-unknown"]').count(), 1);
    sql('DELETE FROM account_roles WHERE user_id=' + actor);
    await privateResponse(await page.reload(), 403);
    console.log('EXPOSURE_REPORT_BROWSER_PASS native no-JS form; closed/calendar; 103 genres; unknown health; preserved pagination/scope; XSS escaped; 390/1440; live withdrawal and role revocation');
  } finally {
    if (browser) await browser.close();
    sql("DELETE FROM flow_observation_hourly WHERE book_id IN (SELECT id FROM books WHERE source_identity LIKE '" + prefix +
      "%');DELETE FROM books WHERE source_identity LIKE '" + prefix + "%'");
    if (actor) sql('DELETE FROM account_roles WHERE user_id=' + actor + ';DELETE FROM users WHERE id=' + actor);
  }
})().catch(error => {console.error(error);process.exitCode = 1;});
