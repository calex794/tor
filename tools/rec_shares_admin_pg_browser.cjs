'use strict';
// Actual no-JS browser acceptance for REC/ATTN share controls and rollback.
// Strictly isolated loopback tor_ci, no private-data export or live deployment.
const {chromium} = require('playwright');
const {execFileSync} = require('node:child_process');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const db = process.env.APP_URL || '';
const base = process.env.TOR_REC_BASE || 'http://127.0.0.1:3108';
const shots = process.env.TOR_REC_SHOTS || '/tmp/rec-shares-admin-shots';
const parsed = new URL(db);
assert.equal(parsed.protocol, 'postgresql:');
assert.equal(parsed.username, 'tor_app');
assert.equal(parsed.hostname, '127.0.0.1');
assert.equal(parsed.port, '5432');
assert.equal(parsed.pathname, '/tor_ci');
assert.match(base, /^http:\/\/127\.0\.0\.1:\d+$/);
fs.mkdirSync(shots, {recursive: true});
const PAGE = '/admin/recommendations';

function sql(query) {
  return execFileSync('psql',
    ['-XAtq', '-v', 'ON_ERROR_STOP=1', '--dbname', db, '-c', query],
    {encoding: 'utf8', env: {...process.env, PGOPTIONS: '-c statement_timeout=5000 -c lock_timeout=2000'}}).trim();
}
function snapshot() {
  const parts = sql("SELECT revision||'|'||author_pct||'|'||teammate_pct||'|'||" +
    "other_authors_pct||'|'||no_team_author_pct||'|'||no_team_other_authors_pct" +
    " FROM rec_shelf_share_policy WHERE singleton").split('|').map(Number);
  assert.equal(parts.length, 6);
  assert.ok(parts.every(Number.isSafeInteger));
  return {revision: parts[0], shares: parts.slice(1)};
}
async function privateResponse(response, status = 200) {
  assert.equal(response.status(), status);
  const headers = await response.allHeaders();
  assert.equal(headers['cache-control'], 'no-store');
  assert.match(headers['x-robots-tag'], /noindex/);
}
async function submit(page, button) {
  const [response] = await Promise.all([
    page.waitForResponse(r => r.url() === base + PAGE && r.request().method() === 'POST'),
    button.click()
  ]);
  await privateResponse(response, 303);
  await page.waitForURL(url => url.pathname === PAGE);
  await page.waitForLoadState('load');
}
(async () => {
  const stamp = Date.now();
  const email = 'rec-share-browser-' + stamp + '@example.invalid';
  let browser, actor;
  try {
    assert.equal(sql('SELECT count(*) FROM _sqlx_migrations WHERE version=202612079483 AND success'), '1');
    browser = await chromium.launch({headless: true});
    const guest = await browser.newContext({javaScriptEnabled: false, viewport: {width: 390, height: 900}});
    const guestPage = await guest.newPage();
    await privateResponse(await guestPage.goto(base + PAGE), 401);

    const ctx = await browser.newContext({javaScriptEnabled: false, viewport: {width: 390, height: 900}});
    const page = await ctx.newPage();
    await page.goto(base + '/register');
    await page.getByLabel('Отображаемое имя').fill('Администратор рекомендаций');
    await page.getByLabel('Электронная почта').fill(email);
    await page.getByLabel('Пароль (12–128 символов)').fill('RecommendationShareBrowser2026');
    await page.getByRole('button', {name: 'Создать учётную запись'}).click();
    await page.waitForURL('**/me');
    actor = Number(sql("SELECT id FROM users WHERE email_normalized='" + email + "'"));
    assert.ok(Number.isSafeInteger(actor) && actor > 0);
    await privateResponse(await page.goto(base + PAGE), 403);
    sql("INSERT INTO account_roles(user_id,role) VALUES(" + actor + ",'administrator')");
    await privateResponse(await page.goto(base + PAGE));
    const initial = snapshot();
    const first = page.locator('[data-testid="rec-share-policy"]');
    assert.equal(Number(await first.getAttribute('data-revision')), initial.revision);
    assert.equal(await page.locator('[data-testid="rec-policy-history"]').count(), 1);
    assert.equal(await page.locator('[data-flow-proof]').count(), 0);

    for (const width of [390, 1440]) {
      await page.setViewportSize({width, height: 900});
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
      await page.screenshot({path: path.join(shots, 'rec-share-admin-' + width + '.png'), animations: 'disabled', fullPage: true});
    }
    const alt = initial.shares.join(',') === '40,30,30,60,40' ?
      [50, 40, 10, 70, 30] : [40, 30, 30, 60, 40];
    const names = ['author_pct', 'teammate_pct', 'other_authors_pct',
      'no_team_author_pct', 'no_team_other_authors_pct'];
    const form = page.locator('[data-testid="rec-shares-form"]');
    for (let i = 0; i < names.length; i++) {
      await form.locator('input[name="' + names[i] + '"]').fill(String(alt[i]));
    }
    await submit(page, form.getByRole('button', {name: 'Сохранить доли'}));
    const changed = snapshot();
    assert.equal(changed.revision, initial.revision + 1);
    assert.deepEqual(changed.shares, alt);
    assert.equal(await page.locator('[data-flow-proof]').count(), 0);
    assert.equal(Number(await page.locator('[data-testid="rec-share-policy"]').getAttribute('data-revision')), changed.revision);

    const rollback = page.getByRole('button', {name: 'Вернуть ревизию ' + initial.revision, exact: true});
    assert.equal(await rollback.count(), 1);
    await submit(page, rollback);
    const restored = snapshot();
    assert.equal(restored.revision, initial.revision + 2);
    assert.deepEqual(restored.shares, initial.shares);
    assert.equal(sql('SELECT count(*) FROM rec_shelf_share_policy_audit WHERE revision IN (' +
      (initial.revision + 1) + ',' + (initial.revision + 2) + ')'), '2');

    sql("DELETE FROM account_roles WHERE user_id=" + actor);
    await privateResponse(await page.goto(base + PAGE), 403);
    console.log('REC_SHARES_ADMIN_BROWSER_PASS guest/reader denied; no-JS 390/1440; CAS save; audit rollback; ACL revoked; no FLOW proofs');
  } finally {
    if (browser) await browser.close();
    if (actor) {
      sql("DELETE FROM account_roles WHERE user_id=" + actor);
      sql("DELETE FROM sessions WHERE user_id=" + actor);
      sql("DELETE FROM users WHERE id=" + actor);
    }
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
