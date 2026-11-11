'use strict';
// Real opt-in Chromium proof for bounded, non-monetary gifts.
// Runs only against disposable tor_ci under GitHub Actions.
const assert = require('node:assert/strict');
const {execFileSync} = require('node:child_process');
const {randomBytes, createHash} = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const {chromium} = require('playwright');

const db = process.env.APP_URL || '';
const base = process.env.TOR_SURPRISE_BASE || 'http://127.0.0.1:3108';
const shots = process.env.TOR_SURPRISE_SHOTS || '/tmp/surprise-browser-shots';
const target = new URL(db);
if (process.env.GITHUB_ACTIONS !== 'true'
    || target.protocol !== 'postgresql:'
    || target.hostname !== '127.0.0.1'
    || target.port !== '5432'
    || target.pathname !== '/tor_ci'
    || !/^http:\/\/127\.0\.0\.1:\d+$/.test(base)) {
  throw new Error('F-SURPRISE Chromium acceptance requires isolated CI tor_ci');
}
fs.mkdirSync(shots, {recursive: true});
function sql(statement) {
  return execFileSync('psql',
    ['-X', '-A', '-t', '-q', '-v', 'ON_ERROR_STOP=1', '--dbname', db, '--command', statement],
    {encoding: 'utf8',
      env: {...process.env, PGOPTIONS: '-c statement_timeout=5000 -c lock_timeout=2000'}}).trim();
}
function scalar(statement) {
  const value = sql(statement);
  assert.match(value, /^\d+$/);
  return Number(value);
}

(async () => {
  const nonce = Date.now() + '-' + randomBytes(5).toString('hex');
  const token = randomBytes(32).toString('base64url');
  const csrf = randomBytes(32).toString('base64url');
  const digest = createHash('sha256').update(token).digest('hex');
  const title = 'Surprise-browser-' + nonce;
  let user = 0, item = 0, browser;
  try {
    user = scalar(`INSERT INTO users(email_normalized,display_name,password_hash)
      VALUES('surprise-browser-${nonce}@tor-ci.example','Surprise browser',
             '$argon2id$fixture') RETURNING id`);
    sql(`INSERT INTO sessions(token_hash,user_id,expires_at)
      VALUES(decode('${digest}','hex'),${user},now()+interval '1 hour')`);
    item = scalar(`INSERT INTO surprise_content(kind,title,body,editorial,state)
      VALUES('joke','${title}','Reader-safe editorial browser text',true,'approved')
      RETURNING id`);
    sql("UPDATE surprise_runtime_policy SET enabled=true,probability_per_mille=1000 WHERE singleton");

    browser = await chromium.launch({headless: true});
    const context = await browser.newContext({viewport: {width: 1500, height: 900}});
    await context.addCookies([
      {name: 'tor-dev-session', value: token, url: base, httpOnly: true, sameSite: 'Strict'},
      {name: 'tor-dev-csrf', value: csrf, url: base, httpOnly: true, sameSite: 'Strict'},
    ]);
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    await page.clock.install();
    const [response] = await Promise.all([
      page.waitForResponse(r => r.url() === base + '/api/v1/me/surprises/plan'),
      page.goto(base + '/'),
    ]);
    assert.equal(response.status(), 200);
    const plan = await response.json();
    assert.equal(plan.state, 'planned');
    assert.ok(Number.isInteger(plan.utc_day));
    assert.ok(plan.active_after_seconds >= 120 && plan.active_after_seconds <= 1200);
    assert.equal(plan.csrf_token, csrf);
    assert.equal(await page.locator('script[src="/assets/surprise.js"]').count(), 1);
    assert.equal(await page.locator('link[href="/assets/surprise.css"]').count(), 1);
    assert.equal(await page.locator('.tor-surprise').count(), 0, 'no surprise before threshold');
    assert.equal(await page.evaluate(() => document.visibilityState), 'visible');
    await page.bringToFront();
    assert.equal(await page.evaluate(() => document.hasFocus()), true);

    const savedPlan = JSON.parse(sql(`SELECT json_build_object(
      'state',state,'delay',active_after_seconds) FROM surprise_daily_decisions
      WHERE user_id=${user} AND utc_day=${plan.utc_day}`));
    assert.equal(savedPlan.state, 'planned');
    assert.equal(savedPlan.delay, plan.active_after_seconds);
    // Only the disposable fixture receipt is backdated. Browser still has
    // to accrue real JS active seconds; this is not a production bypass.
    sql(`UPDATE surprise_daily_decisions SET created_at=now()-interval '30 minutes'
      WHERE user_id=${user} AND utc_day=${plan.utc_day}`);

    // The UI uses active foreground seconds, not Date.now elapsed alone.
    // Advance the *browser's* clock in <=60 s slices and emit a normal
    // scroll signal to keep the synthetic reader active.
    let remaining = plan.active_after_seconds * 1000 + 1000;
    while (remaining > 0) {
      const period = Math.min(60000, remaining);
      await page.clock.runFor(period);
      remaining -= period;
      await page.evaluate(() => document.dispatchEvent(new Event('scroll')));
    }

    const trigger = page.getByRole('button', {name: 'Открыть случайную находку'});
    await trigger.waitFor({state: 'visible', timeout: 12000});
    const mode = await trigger.evaluate(node => node.closest('.tor-surprise')?.className || '');
    assert.match(mode, /tor-surprise--(?:left|right|inline)/);
    const overlaps = await trigger.evaluate(node => {
      const anchor = node.closest('.tor-surprise');
      if (anchor.classList.contains('tor-surprise--inline')) return false;
      const card = document.querySelector('#main .container');
      if (!card) return false;
      const content = card.getBoundingClientRect(), icon = node.getBoundingClientRect();
      return icon.left < content.right && icon.right > content.left;
    });
    assert.equal(overlaps, false, 'desktop gift cannot cover book content');
    await trigger.click();
    await page.getByRole('heading', {name: title}).waitFor({state: 'visible', timeout: 9000});
    assert.equal(await page.locator('.tor-surprise__card').count(), 1);
    assert.ok(await page.getByText('Reader-safe editorial browser text').isVisible());
    assert.ok(await page.getByText('Выбор редакции').isVisible());
    await page.screenshot({path: path.join(shots, 'surprise-open-desktop.png'),
      fullPage: false, animations: 'disabled'});

    const [disabled] = await Promise.all([
      page.waitForResponse(r => r.url() === base + '/api/v1/me/surprises/disable'),
      page.getByRole('button', {name: 'Больше не показывать'}).click(),
    ]);
    assert.equal(disabled.status(), 204);
    assert.equal(scalar(`SELECT disabled::int FROM surprise_user_preferences WHERE user_id=${user}`), 1);
    assert.equal(await page.locator('.tor-surprise').count(), 0);
    const off = await page.evaluate(async () => {
      const r = await fetch('/api/v1/me/surprises/plan', {credentials: 'same-origin'});
      return {status: r.status, data: await r.json()};
    });
    assert.equal(off.status, 200);
    assert.equal(off.data.state, 'off');

    const guest = await browser.newContext({viewport: {width: 390, height: 844}});
    const anonymous = await guest.newPage();
    const answer = await anonymous.goto(base + '/');
    assert.equal(answer.status(), 200);
    assert.equal(await anonymous.locator('.tor-surprise').count(), 0);
    assert.deepEqual(errors, []);
    assert.equal(scalar(`SELECT count(*) FROM surprise_daily_decisions WHERE user_id=${user}`), 1);
    await guest.close();
    await context.close();
    console.log('F-SURPRISE Chromium PASS: real active-time gift, safe gutter/inline anchor, admin-approved text, no duplicate chance, opt-out, anonymous and 390/1500 viewports');
  } finally {
    if (browser) await browser.close();
    sql("UPDATE surprise_runtime_policy SET enabled=false WHERE singleton");
    if (item) sql(`DELETE FROM surprise_content WHERE id=${item}`);
    if (user) sql(`DELETE FROM users WHERE id=${user}`);
  }
})().catch(err => {console.error(err); process.exitCode = 1;});
