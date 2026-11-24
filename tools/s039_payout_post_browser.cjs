// S039: native author/admin forms against the real Axum server and disposable PG18.
// Internal wallet posting only; no provider, retained database or real funds.
const { chromium } = require('playwright');
const { execFileSync, spawn } = require('node:child_process');
const assert = require('node:assert/strict');
const { randomBytes } = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

const database = process.env.APP_URL || '';
const url = new URL(database);
assert.equal(url.protocol, 'postgresql:');
assert.equal(url.hostname, '127.0.0.1');
assert.equal(url.port, '5432');
assert.equal(url.pathname, '/tor_ci');
assert.equal(url.username, 'tor_app');
const temp = path.resolve(process.env.RUNNER_TEMP || '');
assert(process.env.RUNNER_TEMP, 'RUNNER_TEMP is required for isolated evidence');
const shots = path.resolve(process.env.TOR_READER_SHOTS || '');
assert(shots.startsWith(temp + path.sep), 'screenshots must remain in RUNNER_TEMP');
fs.mkdirSync(shots, { recursive: true });
const base = 'http://127.0.0.1:3097';
const password = 'S039-ci-internal-posting-password-2026';
const stamp = Date.now().toString(36) + randomBytes(4).toString('hex');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));

function pg(sql) {
  return execFileSync('psql', [
    '-X', '-q', '-A', '-t', '-v', 'ON_ERROR_STOP=1',
    '--dbname', database, '--command', sql,
  ], { encoding: 'utf8', timeout: 20000,
    env: { ...process.env, PGOPTIONS: '-c statement_timeout=15000 -c lock_timeout=5000' },
  }).trim();
}
function id(sql) {
  const result = pg(sql);
  assert.match(result, /^[1-9][0-9]*$/);
  const value = Number(result);
  assert(Number.isSafeInteger(value));
  return value;
}
async function register(page, device, role) {
  const email = 's039-browser-' + device + '-' + role + '-' + stamp + '@tor-ci.example';
  await page.goto(base + '/register');
  await page.getByLabel('Отображаемое имя').fill('S039 ' + role + ' ' + device);
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill(password);
  await page.getByRole('button', { name: 'Создать учётную запись' }).click();
  await page.waitForURL(base + '/me');
  return id("SELECT id FROM users WHERE email_normalized='" + email + "'");
}
function balance(owner) {
  return pg('SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=' + owner);
}
function reserved(owner) {
  return pg('SELECT COALESCE(sum(reservation_minor),0) FROM finance_creator_payout_requests ' +
    "WHERE user_id=" + owner + " AND state='pending'");
}
async function reserve(page, owner, amount, minor) {
  await page.goto(base + '/me/finance');
  await page.getByLabel('Сумма резерва, RUB (например, 100.50)').fill(amount);
  const response = page.waitForResponse(r =>
    r.url() === base + '/me/finance' && r.request().method() === 'POST');
  await page.getByRole('button', { name: 'Создать заявку на резерв' }).click();
  assert.equal((await response).status(), 303);
  await page.waitForURL(base + '/me/finance');
  return id('SELECT id FROM finance_creator_payout_requests WHERE user_id=' +
    owner + ' AND amount_minor=' + minor);
}
async function device(browser, label, width, height) {
  const authorContext = await browser.newContext({ viewport: { width, height } });
  const adminContext = await browser.newContext({ viewport: { width, height } });
  let admin;
  try {
    const authorPage = await authorContext.newPage();
    const adminPage = await adminContext.newPage();
    const owner = await register(authorPage, label, 'author');
    admin = await register(adminPage, label, 'administrator');
    pg("INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id) " +
      "VALUES('ci:s039:browser:" + label + ':' + stamp + "','ru','S039 test book','public'," + owner + ')');
    pg("INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) VALUES(" + owner + ",1000,'RUB')");
    pg("INSERT INTO account_roles(user_id,role,is_active) VALUES(" + admin + ",'administrator',true)");
    const first = await reserve(authorPage, owner, '6.00', 600);
    const second = await reserve(authorPage, owner, '3.00', 300);
    assert.notEqual(first, second);
    assert.equal(balance(owner), '1000', 'a pending request must not debit the wallet');
    assert.equal(reserved(owner), '900');

    const queue = await adminPage.goto(base + '/odmen/finance/payouts');
    assert.equal(queue.status(), 200);
    assert.equal(queue.headers()['cache-control'], 'no-store');
    const action = '/odmen/finance/payouts/' + first + '/post';
    const form = adminPage.locator('form[action="' + action + '"]');
    assert.equal(await form.count(), 1);
    assert.equal(await form.locator('[name="amount_minor"], [name="amount_rub"]').count(), 0);
    await form.getByLabel('Основание внутреннего списания').fill('S039 browser internal ' + label);
    const data = await form.evaluate(element => Object.fromEntries(new FormData(element)));
    data.current_password = password;
    const post = (payload, origin = base) => adminContext.request.post(base + action,
      { form: payload, headers: { origin } });
    assert.equal((await post(data, 'https://evil.invalid')).status(), 403);
    assert.equal((await post({ ...data, csrf_token: 'invalid' })).status(), 403);
    assert.equal((await post({ ...data, amount_minor: '1' })).status(), 422);
    // Use the author's own valid CSRF so this denial tests role, not cookie mismatch.
    const ownerCsrf = await authorPage.locator(
      'form[action="/me/finance"] input[name="csrf_token"]'
    ).inputValue();
    assert.equal((await authorContext.request.post(base + action, {
      form: { ...data, csrf_token: ownerCsrf }, headers: { origin: base },
    })).status(), 404);

    await form.getByLabel('Текущий пароль администратора').fill('incorrect-password');
    const rejected = adminPage.waitForResponse(r =>
      r.url() === base + action && r.request().method() === 'POST');
    await form.getByRole('button', { name: 'Создать внутреннюю проводку' }).click();
    assert.equal((await rejected).status(), 401);
    assert.equal(balance(owner), '1000');
    assert.equal(reserved(owner), '900');

    await adminPage.goto(base + '/odmen/finance/payouts');
    await form.getByLabel('Основание внутреннего списания').fill(data.reason);
    await form.getByLabel('Текущий пароль администратора').fill(password);
    const posted = adminPage.waitForResponse(r =>
      r.url() === base + action && r.request().method() === 'POST');
    await form.getByRole('button', { name: 'Создать внутреннюю проводку' }).click();
    const receipt = await posted;
    assert.equal(receipt.status(), 200);
    assert.equal(receipt.headers()['cache-control'], 'no-store');
    assert(await adminPage.getByRole('status').isVisible());
    assert((await adminPage.locator('main').innerText()).includes('Банковский перевод не выполнялся'));
    assert(!(await adminPage.content()).includes(password));
    assert.equal(balance(owner), '400');
    assert.equal(reserved(owner), '300', 'the other request must retain its reservation');
    const evidence = () => pg('SELECT (SELECT count(*) FROM finance_creator_payout_postings ' +
      'WHERE payout_request_id=' + first + ')::text || \':\' || ' +
      '(SELECT count(*) FROM reader_wallet_entries WHERE user_id=' + owner +
      " AND kind='creator_payout_debit')::text");
    assert.equal(evidence(), '1:1');
    const replay = await post(data);
    assert.equal(replay.status(), 200);
    assert((await replay.text()).includes('списание не повторялось'));
    assert.equal((await post({ ...data, reason: 'changed decision' })).status(), 409);
    assert.equal(balance(owner), '400');
    assert.equal(evidence(), '1:1');
    const geometry = await adminPage.evaluate(() => ({
      document: document.documentElement.scrollWidth,
      viewport: document.documentElement.clientWidth,
    }));
    assert(geometry.document <= geometry.viewport + 1, label + ': receipt overflows viewport');
    await adminPage.locator('main').screenshot({
      path: path.join(shots, 's039-internal-post-' + label + '.png'),
    });
    await authorPage.goto(base + '/me/finance?tab=payouts');
    assert((await authorPage.locator('main').innerText()).includes('банковский перевод не подтверждён'));
    pg("UPDATE account_roles SET is_active=false WHERE user_id=" + admin + " AND role='administrator'");
    assert.equal((await post(data)).status(), 404, 'revoked administrator must lose access');
    assert.equal(balance(owner), '400');
    assert.equal(evidence(), '1:1');
    console.log('S039 Chromium ' + label + ' PASS: reserve, password, CSRF, role, exact debit, replay, retained reserve');
  } finally {
    if (admin) pg("UPDATE account_roles SET is_active=false WHERE user_id=" + admin + " AND role='administrator'");
    await authorContext.close();
    await adminContext.close();
  }
}

(async () => {
  // A separate loopback process keeps opt-in financial flags away from the
  // reader-browser server. A pre-existing listener must not be reused.
  const probe = () => fetch(base + '/readyz', { signal: AbortSignal.timeout(1500) });
  const occupied = await probe().then(() => true, () => false);
  assert(!occupied, 'dedicated S039 CI port is already occupied');
  const fd = fs.openSync(path.join(temp, 's039-internal-post-server.log'), 'w', 0o600);
  const server = spawn('./target/debug/tor-web', [], {
    env: { ...process.env, DATABASE_URL: database, TOR_AUTH_ENABLED: '1',
      TOR_CREATOR_PAYOUT_REQUESTS_ENABLED: '1', TOR_S039_PAYOUT_POST_HTTP_ENABLED: '1',
      TOR_PUBLIC_ORIGIN: base, TOR_LISTEN_ADDR: '127.0.0.1:3097' },
    stdio: ['ignore', fd, fd],
  });
  fs.closeSync(fd);
  let startupError;
  server.once('error', error => { startupError = error; });
  const exited = new Promise(resolve => server.once('exit', resolve));
  let browser;
  try {
    let ready = false;
    for (let attempt = 0; attempt < 45; attempt++) {
      if (startupError) throw startupError;
      assert.equal(server.exitCode, null, 'isolated S039 server exited before readiness');
      ready = await probe().then(r => r.ok, () => false);
      if (ready) break;
      await delay(1000);
    }
    assert(ready, 'isolated S039 server not ready');
    browser = await chromium.launch({ headless: true });
    await device(browser, 'desktop', 1440, 900);
    await device(browser, 'mobile', 390, 844);
  } finally {
    if (browser) await browser.close();
    server.kill('SIGTERM');
    await Promise.race([exited, delay(3000)]);
    if (server.exitCode === null) server.kill('SIGKILL');
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
