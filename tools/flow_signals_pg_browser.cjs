'use strict';
// Saved journal/probe UI acceptance, not a replacement for evaluator or real HEAD tests.
// Only a disposable loopback tor_ci is accepted. Background leases are fenced while
// deterministic journal facts are shown; GET must leave those facts unchanged.
const {chromium} = require('playwright');
const {execFileSync} = require('node:child_process');
const {createHash} = require('node:crypto');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const db = process.env.APP_URL || '';
const base = process.env.TOR_SIGNALS_BASE || 'http://127.0.0.1:3114';
const shots = process.env.TOR_SIGNALS_SHOTS || '/tmp/flow-signals-shots';
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
const rules = ['html_quality', 'html_change', 'reading_starts', 'reading_completions',
  'google_quality', 'google_clicks', 'yandex_quality', 'yandex_clicks',
  'exposure_quality', 'exposure_change', 'underexposed_books', 'underexposed_genres', 'public_page_loss'];
const columns = ('rule,days,scope,status,reason,last_epoch,checked_at_ms,until_epoch,window_label,' +
  'enabled,available,current_value,previous_value,current_units,previous_units,expected_units,' +
  'known_discards,source_failed,limited_units,omitted_rows,current_ack_per_mille,previous_ack_per_mille,' +
  'current_sample,previous_sample,current_opportunities,previous_opportunities,unknown_entities,' +
  'episode_seq,episode_reason,episode_opened_ms,episode_emitted,last_notice_ms').split(',');
const marker = 'FlowSignalsBrowser' + Date.now();
const scopes = new Set();
let lastEpoch;
function sql(statement) {
  return execFileSync('psql', ['-XAtq', '-v', 'ON_ERROR_STOP=1', '--dbname', db, '-c', statement],
    {encoding: 'utf8', env: {...process.env, PGOPTIONS: '-c statement_timeout=5000 -c lock_timeout=2000'}}).trim();
}
function quote(value) {
  if (value === null) return 'NULL';
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  if (typeof value === 'number') { assert.ok(Number.isSafeInteger(value)); return String(value); }
  assert.equal(typeof value, 'string');
  return "'" + value.replaceAll("'", "''") + "'";
}
function scope(name) {
  const value = createHash('sha256').update(marker + ':' + name).digest('hex');
  scopes.add(value);
  return value;
}
function now() { return Number(sql("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")); }
function row(rule, days, overrides = {}) {
  const at = now();
  return {rule, days, scope: scope(rule + ':' + days), status: 'unknown', reason: 'missing_measurement',
    last_epoch: lastEpoch, checked_at_ms: at, until_epoch: days === 0 ? null : Math.floor(at / 3600000) * 3600,
    window_label: marker + (days === 0 ? ' current saved sample' : ' closed UTC fixture'), enabled: true, available: false,
    current_value: null, previous_value: null, current_units: 0, previous_units: 0,
    expected_units: days === 0 ? 4 : days * 24, known_discards: 0, source_failed: false,
    limited_units: 0, omitted_rows: 0, current_ack_per_mille: null, previous_ack_per_mille: null,
    current_sample: 0, previous_sample: 0, current_opportunities: 0, previous_opportunities: 0,
    unknown_entities: 0, episode_seq: 0, episode_reason: null, episode_opened_ms: null,
    episode_emitted: false, last_notice_ms: null, ...overrides};
}
function values(record) { return '(' + columns.map(key => quote(record[key])).join(',') + ')'; }
function save(records) {
  sql('INSERT INTO flow_signal_states(' + columns.join(',') + ') VALUES' + records.map(values).join(',') +
    ' ON CONFLICT(rule,days,scope) DO UPDATE SET ' + columns.slice(3).map(key => key + '=EXCLUDED.' + key).join(','));
}
function event(record, kind) {
  sql('INSERT INTO flow_signal_events(' + columns.join(',') + ',kind) VALUES' +
    values(record).slice(0, -1) + ',' + quote(kind) + ')');
}
function fingerprint() {
  return sql("SELECT md5(coalesce(string_agg(row_to_json(s)::text,'|' ORDER BY rule,days,scope),'')) FROM flow_signal_states s;" +
    "SELECT md5(coalesce(string_agg(row_to_json(e)::text,'|' ORDER BY id),'')) FROM flow_signal_events e;" +
    "SELECT md5(coalesce(string_agg(row_to_json(r)::text,'|' ORDER BY days),'')) FROM flow_signal_rounds r;" +
    "SELECT md5(coalesce(string_agg(row_to_json(p)::text,'|'),'')) FROM flow_public_page_probe p;" +
    "SELECT md5(coalesce(string_agg(row_to_json(p)::text,'|' ORDER BY slot),'')) FROM flow_public_page_results p;");
}
async function response(response, status = 200) {
  assert.equal(response.status(), status);
  const headers = await response.allHeaders();
  assert.equal(headers['cache-control'], 'no-store');
  assert.match(headers['x-robots-tag'], /noindex/);
}
async function readonlyGet(page, route) {
  const before = fingerprint();
  await response(await page.goto(base + route));
  assert.equal(fingerprint(), before, 'administrative GET must not evaluate, probe, or change stored evidence');
}
async function screenshots(page, name) {
  for (const width of [390, 1440]) {
    await page.setViewportSize({width, height: 900});
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
    await page.screenshot({path: path.join(shots, name + '-' + width + '.png'), fullPage: true, animations: 'disabled'});
    if (name === 'signals-unknown-open') {
      await page.screenshot({path: path.join(shots, 'signals-review-' + width + '.png'), fullPage: false, animations: 'disabled'});
    }
  }
}
const eligible = "b.visibility='public' AND b.visibility_status='active' AND NOT b.admin_hold " +
  "AND NOT b.is_deleted AND b.access_level='basic' AND b.lifecycle_status<>'pending_deletion'";
function savedPages() {
  // Exact current target/revision fixture; the owner PG suite separately exercises
  // selection, real network, pre/post policy checks, lease fencing and timeout.
  sql('BEGIN;DELETE FROM flow_public_page_results;' +
    "UPDATE flow_public_page_probe SET epoch=epoch+1,origin=" + quote(base) + ',finished_origin=' + quote(base) +
    ",finished_at=now(),started_at=now(),lease_until=now()+interval '10 minutes';" +
    "INSERT INTO flow_public_page_results(slot,http_status,baseline_known,last_success_at,checked_at)" +
    " VALUES(1,200,true,now(),now()),(2,200,true,now(),now());" +
    "INSERT INTO flow_public_page_results(slot,book_id,book_revision,http_status,baseline_known,last_success_at,checked_at)" +
    ' SELECT 3,b.id,b.xmin::text,200,true,now(),now() FROM books b WHERE ' + eligible + ' ORDER BY b.id LIMIT 1;' +
    "INSERT INTO flow_public_page_results(slot,book_id,chapter_id,book_revision,chapter_revision,publication_revision,version_id,http_status,baseline_known,last_success_at,checked_at)" +
    " SELECT 4,b.id,c.id,b.xmin::text,c.xmin::text,p.xmin::text,p.version_id,200,true,now(),now()" +
    " FROM chapters c JOIN books b ON b.id=c.book_id JOIN chapter_publications p ON p.chapter_id=c.id" +
    " JOIN chapter_versions v ON v.id=p.version_id AND v.chapter_id=c.id WHERE " + eligible +
    " AND c.visibility='public' AND c.access_level='free' AND NOT c.admin_hold AND NOT c.is_deleted" +
    " AND (c.reader_released_at IS NULL OR c.reader_released_at<=now()) ORDER BY c.id LIMIT 1;COMMIT;");
}
(async () => {
  let browser, actor, book, backupProbe, backupPages, fenced = false;
  const disclaimer = 'Восстановление ранее проверенных URL не утверждается';
  try {
    assert.equal(sql('SELECT count(*) FROM _sqlx_migrations WHERE version IN(202611294683,202611294686) AND success'), '2');
    backupProbe = sql('SELECT row_to_json(p)::text FROM flow_public_page_probe p WHERE singleton');
    backupPages = sql("SELECT coalesce(json_agg(p ORDER BY slot),'[]'::json)::text FROM flow_public_page_results p");
    sql("BEGIN;UPDATE flow_signal_rounds SET epoch=epoch+1,started_ms=floor(extract(epoch FROM clock_timestamp())*1000)::bigint," +
      "lease_until_ms=floor(extract(epoch FROM clock_timestamp())*1000)::bigint+600000,committed=false,payload_digest=NULL;" +
      "UPDATE flow_public_page_probe SET epoch=epoch+1,started_at=now(),lease_until=now()+interval '10 minutes';COMMIT;");
    fenced = true;
    lastEpoch = Number(sql('SELECT coalesce(max(last_epoch),0)+1000000 FROM flow_signal_states'));
    const initial = [];
    for (const days of [1, 7, 30, 90]) for (const rule of rules.filter(rule => rule !== 'public_page_loss')) initial.push(row(rule, days));
    initial.push(row('public_page_loss', 0));
    save(initial);
    const sample = JSON.parse(sql("WITH b AS (INSERT INTO books(source_identity,target_language,title,visibility,access_level)" +
      " VALUES(" + quote('ci:' + marker) + ",'ru','Public sample','public','basic') RETURNING id)," +
      "c AS (INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)" +
      " SELECT id,1,'Public free chapter','public','free',0 FROM b RETURNING id,book_id)," +
      "v AS (INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) SELECT id,1,'Public free text' FROM c RETURNING id,chapter_id)," +
      "p AS (INSERT INTO chapter_publications(chapter_id,version_id) SELECT chapter_id,id FROM v RETURNING chapter_id)" +
      " SELECT json_build_object('book',c.book_id,'chapter',c.id) FROM c JOIN p ON p.chapter_id=c.id"));
    book = sample.book;
    assert.ok(Number.isSafeInteger(book) && book > 0);
    sql("DELETE FROM flow_public_page_results;UPDATE flow_public_page_probe SET finished_origin=" + quote(base) + ',finished_at=now();');
    browser = await chromium.launch({headless: true});
    const context = await browser.newContext({javaScriptEnabled: false, viewport: {width: 1440, height: 900}});
    const page = await context.newPage();
    for (const route of ['/admin/audience/signals', '/admin/audience/pages']) await response(await page.goto(base + route), 401);
    await page.goto(base + '/register');
    const email = marker.toLowerCase() + '@tor-ci.example';
    await page.getByLabel('Отображаемое имя').fill('Signal report operator');
    await page.getByLabel('Электронная почта').fill(email);
    await page.getByLabel('Пароль (12–128 символов)').fill('SignalsBrowserPassword2026');
    await page.getByRole('button', {name: 'Создать учётную запись', exact: true}).click();
    await page.waitForURL('**/me');
    actor = Number(sql('SELECT id FROM users WHERE email_normalized=' + quote(email)));
    for (const route of ['/admin/audience/signals', '/admin/audience/pages']) await response(await page.goto(base + route), 403);
    sql("INSERT INTO account_roles(user_id,role) VALUES(" + actor + ",'administrator')");
    await readonlyGet(page, '/admin/audience/signals');
    assert.equal(await page.locator('[data-testid="signal-state"]').count(), 13);
    assert.equal(await page.locator('[data-testid="signal-state"][data-state="unknown"]').count(), 13);
    assert.equal(await page.locator('[data-testid="signal-state"][data-state="healthy"]').count(), 0);
    for (const days of [1, 7, 30, 90]) {
      const before = fingerprint();
      await page.getByLabel('Длительность периода').selectOption(String(days));
      await page.getByRole('button', {name: 'Показать', exact: true}).click();
      await page.waitForURL(url => url.searchParams.get('days') === String(days));
      assert.equal(await page.getByLabel('Длительность периода').inputValue(), String(days));
      assert.equal(await page.locator('[data-testid="signal-state"]').count(), 13);
      assert.equal(fingerprint(), before);
    }
    const injection = '<script>flowFixtureBad</script><img src=x onerror=flowFixtureBad>';
    const active = row('underexposed_genres', 7, {
      status: 'active', reason: 'low_exposure', available: true, current_value: 1, previous_value: 0,
      current_units: 168, previous_units: 168, expected_units: 168,
      current_ack_per_mille: 1000, previous_ack_per_mille: 1000,
      current_sample: 20, previous_sample: 20, current_opportunities: 20, previous_opportunities: 20,
      episode_seq: 1, episode_reason: 'low_exposure', episode_opened_ms: now(), episode_emitted: true,
      last_notice_ms: now(), window_label: injection,
    });
    save([active]); event(active, 'opened');
    await readonlyGet(page, '/admin/audience/signals?days=7');
    const genre = page.locator('[data-testid="signal-state"][data-rule="underexposed_genres"]');
    assert.equal(await genre.getAttribute('data-state'), 'active');
    assert.equal(await genre.locator('[data-testid="signal-open-episode"]').count(), 1);
    assert.ok((await genre.innerText()).includes(injection));
    assert.equal(await genre.locator('script,img,iframe').count(), 0);
    save([{...active, status: 'unknown', reason: 'missing_measurement', available: false,
      current_value: null, previous_value: null, checked_at_ms: now(), unknown_entities: 1}]);
    await readonlyGet(page, '/admin/audience/signals?days=7');
    assert.equal(await genre.getAttribute('data-state'), 'unknown');
    assert.equal(await genre.locator('[data-testid="signal-open-episode"]').count(), 1);
    assert.equal(await page.locator('[data-testid="signal-event"][data-kind="recovered"]').count(), 0);
    await screenshots(page, 'signals-unknown-open');
    const healthy = {...active, status: 'healthy', reason: 'within_threshold', current_value: 0,
      checked_at_ms: now(), unknown_entities: 0, episode_reason: null, episode_opened_ms: null, episode_emitted: false};
    save([healthy]); event(healthy, 'recovered');
    const publicActive = row('public_page_loss', 0, {status: 'active', reason: 'public_page_loss', available: true,
      current_value: 1, previous_value: null, current_units: 4, previous_units: 0, expected_units: 4,
      episode_seq: 1, episode_reason: 'public_page_loss', episode_opened_ms: now(), episode_emitted: true, last_notice_ms: now()});
    event(publicActive, 'opened');
    const publicHealthy = {...publicActive, status: 'healthy', reason: 'within_threshold', current_value: 0,
      checked_at_ms: now(), episode_reason: null, episode_opened_ms: null, episode_emitted: false};
    save([publicHealthy]); event(publicHealthy, 'recovered');
    await readonlyGet(page, '/admin/audience/signals?days=7');
    assert.equal(await genre.getAttribute('data-state'), 'healthy');
    assert.equal(await genre.locator('[data-testid="signal-open-episode"]').count(), 0);
    const publicState = page.locator('[data-testid="signal-state"][data-rule="public_page_loss"]');
    assert.ok((await publicState.innerText()).includes(disclaimer));
    const publicRecovery = page.locator('[data-testid="signal-event"][data-kind="recovered"]').filter({hasText: 'Недоступные публичные страницы'});
    assert.equal(await publicRecovery.count(), 1);
    assert.ok((await publicRecovery.innerText()).includes(disclaimer));
    assert.ok(!(await publicRecovery.innerText()).includes('Подтверждено восстановление'));
    assert.equal(await page.locator('[data-testid="signal-event"][data-kind="recovered"]').count(), 2);
    save([{...healthy, checked_at_ms: now() - 301000}]);
    await readonlyGet(page, '/admin/audience/signals?days=7');
    assert.equal(await genre.getAttribute('data-state'), 'unknown', 'stale healthy evidence is not a current recovery');
    save([{...healthy, checked_at_ms: now()}]);
    for (let n = 0; n < 55; n++) {
      event(row('reading_starts', 7, {scope: scope('history:' + n), status: 'active', reason: 'recorded_drop',
        available: true, current_value: 50, previous_value: 100, current_units: 168, previous_units: 168, expected_units: 168,
        episode_seq: 1, episode_reason: 'recorded_drop', episode_opened_ms: now(), episode_emitted: true, last_notice_ms: now(),
        window_label: marker + ' history ' + n}), 'opened');
    }
    await readonlyGet(page, '/admin/audience/signals?days=7');
    const first = await page.locator('[data-testid="signal-event"]').allTextContents();
    assert.equal(first.length, 50);
    const beforePage = fingerprint();
    await page.locator('[data-testid="signal-next"]').click();
    await page.waitForURL(url => Number(url.searchParams.get('before')) > 0);
    assert.equal(new URL(page.url()).searchParams.get('days'), '7');
    const second = await page.locator('[data-testid="signal-event"]').allTextContents();
    assert.ok(second.length > 0 && second.length <= 50);
    assert.ok(second.every(value => !first.includes(value)), 'keyset pages must not repeat rows');
    assert.equal(fingerprint(), beforePage);
    await readonlyGet(page, '/admin/audience/pages');
    assert.equal(await page.locator('[data-testid="public-page-state"]').count(), 4);
    assert.deepEqual(await page.locator('[data-testid="public-page-state"]').evaluateAll(rows => rows.map(row => row.dataset.slot)), ['1','2','3','4']);
    assert.equal(await page.locator('[data-testid="pages-unknown"]').count(), 1);
    assert.equal(await page.locator('[data-testid="pages-current-healthy"]').count(), 0);
    savedPages();
    await readonlyGet(page, '/admin/audience/pages');
    assert.equal(await page.locator('[data-testid="public-page-state"][data-state="healthy"]').count(), 4);
    assert.ok((await page.locator('[data-testid="pages-current-healthy"]').innerText()).includes(disclaimer));
    await screenshots(page, 'public-pages-current-sample');
    sql('UPDATE flow_public_page_results SET http_status=503 WHERE slot=3');
    await readonlyGet(page, '/admin/audience/pages');
    assert.equal(await page.locator('[data-testid="pages-loss"]').count(), 1);
    sql('UPDATE flow_public_page_results SET baseline_known=false,last_success_at=NULL WHERE slot=3');
    await readonlyGet(page, '/admin/audience/pages');
    assert.equal(await page.locator('[data-testid="public-page-state"][data-slot="3"]').getAttribute('data-state'), 'unknown');
    assert.equal(await page.locator('[data-testid="pages-loss"]').count(), 0);
    assert.equal(await page.locator('[data-testid="pages-current-healthy"]').count(), 0);
    savedPages();
    sql("UPDATE flow_public_page_probe SET finished_at=now()-interval '181 seconds'");
    await readonlyGet(page, '/admin/audience/pages');
    assert.equal(await page.locator('[data-testid="public-page-state"][data-state="unknown"]').count(), 4);
    sql('DELETE FROM account_roles WHERE user_id=' + actor);
    for (const route of ['/admin/audience/signals', '/admin/audience/pages']) await response(await page.goto(base + route), 403);
    console.log('FLOW_SIGNALS_BROWSER_PASS saved journal fixtures; native no-JS 1/7/30/90; 13 rules; unknown/open/recovered/stale; keyset; XSS; current-sample wording; 4 page slots; GET read-only; 390/1440; live role');
  } finally {
    if (browser) await browser.close();
    if (scopes.size) sql('DELETE FROM flow_signal_events WHERE scope IN(' + [...scopes].map(quote).join(',') + ');' +
      'DELETE FROM flow_signal_states WHERE scope IN(' + [...scopes].map(quote).join(',') + ');');
    if (book) sql('DELETE FROM books WHERE id=' + book);
    if (actor) sql('DELETE FROM account_roles WHERE user_id=' + actor + ';DELETE FROM users WHERE id=' + actor);
    if (fenced) {
      sql('BEGIN;DELETE FROM flow_public_page_results;INSERT INTO flow_public_page_results SELECT * FROM json_populate_recordset(NULL::flow_public_page_results,' + quote(backupPages) + ');' +
        'UPDATE flow_public_page_probe p SET epoch=p.epoch+1,origin=b.origin,finished_origin=b.finished_origin,' +
        'started_at=b.started_at,finished_at=b.finished_at,lease_until=NULL FROM json_populate_record(NULL::flow_public_page_probe,' + quote(backupProbe) + ') b WHERE p.singleton;' +
        'UPDATE flow_signal_rounds SET epoch=epoch+1,lease_until_ms=0,committed=false,payload_digest=NULL;COMMIT;');
    }
  }
})().catch(error => {console.error(error);process.exitCode = 1;});
