// Real Chromium + PostgreSQL, using the ordinary account routes.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const path = require('node:path');
const base = 'http://127.0.0.1:3098';
const password = 'Dialog-Test-Password-2026';
const email = 'auth-dialog.' + Date.now() + '@example.test';
const shots = process.env.TOR_READER_SHOTS;

async function openDialog(page, mobile = false) {
  if (mobile) {
    await page.locator('.site-mobile-menu > summary').click();
    await page.locator('.site-mobile-nav a[href="/login"]').click();
  } else {
    await page.locator('.site-header__auth a[href="/login"]').click();
  }
  const dialog = page.locator('#tor-auth-dialog');
  await dialog.locator('form[action="/login"]').waitFor();
  assert.equal(await dialog.isVisible(), true);
  assert.equal(await page.locator('.site-header a[href="/register"]').count(), 0);
  return dialog;
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const desktopContext = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    const desktop = await desktopContext.newPage();
    // Chromium without the newer timeout helper must still use the modal.
    await desktop.addInitScript(() => {
      Object.defineProperty(AbortSignal, 'timeout', { value: undefined, configurable: true });
    });
    const destination = base + '/books/1?dialog-test=1#main';
    const bookResponse = await desktop.goto(destination);
    const integrity = await desktop.locator('script[src^="/assets/tor-auth-dialog.js"]').getAttribute('integrity');
    assert.match(integrity, /^sha256-[A-Za-z0-9+/]{43}=$/);
    const bookCsp = bookResponse.headers()['content-security-policy'];
    assert.ok(bookCsp.includes("script-src '" + integrity + "';"));
    assert.ok(bookCsp.includes("connect-src 'self';"));
    assert.ok(!bookCsp.includes("script-src 'self'"));
    // A standalone form in another tab must remain usable after opening
    // and switching modal forms in this tab.
    const native = await desktopContext.newPage();
    await native.goto(base + '/login');
    const csrf = await native.locator('input[name="csrf_token"]').inputValue();
    const dialog = await openDialog(desktop);
    assert.ok(await dialog.locator('input[name="csrf_token"]').inputValue() === csrf,
      'opening modal invalidated an existing standalone CSRF token');
    await desktop.keyboard.press('Escape');
    await dialog.waitFor({ state: 'hidden' });
    assert.equal(desktop.url(), destination);

    // Hold the first GET until after dismissal/reopening. A stale canceled
    // request must not navigate away, replace the new form or show an error.
    let held = false;
    let release;
    let completed;
    const gate = new Promise(resolve => { release = resolve; });
    const released = new Promise(resolve => { completed = resolve; });
    const holdFirst = async route => {
      const request = route.request();
      if (!held && request.method() === 'GET' && request.headers()['x-tor-auth-dialog'] === '1') {
        held = true;
        await gate;
        try { await route.abort(); } catch (_) { /* already canceled by the client */ }
        finally { completed(); }
      } else {
        await route.continue();
      }
    };
    await desktop.route(base + '/login', holdFirst);
    try {
      const loading = desktop.waitForRequest(r =>
        r.url() === base + '/login' && r.method() === 'GET' &&
        r.headers()['x-tor-auth-dialog'] === '1');
      await desktop.locator('.site-header__auth a[href="/login"]').click();
      await loading;
      await dialog.getByRole('button', { name: 'Закрыть' }).click();
      await dialog.waitFor({ state: 'hidden' });
      await openDialog(desktop);
      assert.equal(desktop.url(), destination);
    } finally {
      release();
      if (held) await released;
      await desktop.unroute(base + '/login', holdFirst);
    }
    assert.equal(await dialog.locator('[data-auth-error]').isVisible(), false);
    assert.ok(await dialog.locator('input[name="csrf_token"]').inputValue() === csrf,
      'reopening modal invalidated an existing CSRF token');
    if (shots) await desktop.screenshot({ path: path.join(shots, 'auth-dialog-desktop.png') });
    await dialog.getByRole('button', { name: 'Регистрация', exact: true }).click();
    await dialog.locator('form[action="/register"]').waitFor();
    assert.ok(await dialog.locator('input[name="csrf_token"]').inputValue() === csrf,
      'switching modal forms invalidated an existing CSRF token');
    assert.ok((await desktop.context().cookies(base)).some(c =>
      c.name === 'tor-dev-csrf' && c.value === csrf), 'CSRF cookie changed across tabs');
    await native.close();
    assert.equal(await dialog.locator('[name="display_name"]').count(), 0);
    assert.equal(await dialog.locator('input[required]').count(), 2);
    await dialog.getByLabel('Электронная почта').fill(email);
    await dialog.getByLabel('Пароль (12–128 символов)').fill('short');
    const weak = desktop.waitForResponse(r => r.url() === base + '/register' && r.request().method() === 'POST');
    await dialog.getByRole('button', { name: 'Создать учётную запись' }).click();
    assert.equal((await weak).status(), 400);
    await dialog.locator('[data-auth-error]').waitFor({ state: 'visible' });
    assert.equal(desktop.url(), destination);
    assert.equal(await dialog.getByLabel('Электронная почта').inputValue(), email);
    assert.equal(await dialog.getByLabel('Пароль (12–128 символов)').inputValue(), '');
    await dialog.getByLabel('Пароль (12–128 символов)').fill(password);
    const registration = desktop.waitForResponse(r => r.url() === base + '/register' && r.request().method() === 'POST');
    await dialog.getByRole('button', { name: 'Создать учётную запись' }).click();
    assert.equal((await registration).status(), 204);
    await desktop.locator('.site-header__auth a[href="/me"]').waitFor();
    assert.equal(desktop.url(), destination);
    assert.equal(await desktop.locator('#tor-auth-dialog').count(), 0);
    const signedBook = await desktop.reload();
    assert.ok(signedBook.headers()['content-security-policy'].includes("script-src 'none'"));
    await desktopContext.close();

    const paywall = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    const locked = base + '/reader/103#main';
    const lockedResponse = await paywall.goto(locked);
    assert.ok(lockedResponse.headers()['content-security-policy'].includes("script-src '" + integrity + "';"));
    await paywall.locator('.reader-access-page a[href="/login"]').click();
    const paywallDialog = paywall.locator('#tor-auth-dialog');
    await paywallDialog.locator('form[action="/login"]').waitFor();
    assert.equal(paywall.url(), locked);
    assert.ok(!(await paywall.content()).includes('PRIVATE-PAID-MARKER'));
    await paywallDialog.getByLabel('Электронная почта').fill(email);
    await paywallDialog.getByLabel('Пароль', { exact: true }).fill(password);
    const lockedLogin = paywall.waitForResponse(r => r.url() === base + '/login' && r.request().method() === 'POST');
    await paywallDialog.getByRole('button', { name: 'Войти', exact: true }).click();
    assert.equal((await lockedLogin).status(), 204);
    await paywall.locator('.site-header__auth a[href="/me"]').waitFor();
    assert.equal(paywall.url(), locked);
    assert.ok(!(await paywall.content()).includes('PRIVATE-PAID-MARKER'));
    const signedLocked = await paywall.reload();
    assert.ok(signedLocked.headers()['content-security-policy'].includes("script-src 'none'"));
    assert.equal(await paywall.locator('#tor-auth-dialog').count(), 0);
    await paywall.close();

    const mobile = await browser.newPage({ viewport: { width: 390, height: 844 } });
    const reader = base + '/reader/101?dialog-test=1#main';
    await mobile.goto(reader);
    const login = await openDialog(mobile, true);
    const dimensions = await login.evaluate(node => {
      const bounds = node.getBoundingClientRect();
      return { width: bounds.width, height: bounds.height, viewport: innerWidth, screen: innerHeight };
    });
    assert.ok(dimensions.width <= dimensions.viewport && dimensions.height <= dimensions.screen);
    if (shots) await mobile.screenshot({ path: path.join(shots, 'auth-dialog-mobile.png') });
    await login.getByLabel('Электронная почта').fill(email);
    await login.getByLabel('Пароль', { exact: true }).fill('wrong-password');
    const bad = mobile.waitForResponse(r => r.url() === base + '/login' && r.request().method() === 'POST');
    await login.getByRole('button', { name: 'Войти', exact: true }).click();
    assert.equal((await bad).status(), 401);
    await login.locator('[data-auth-error]').waitFor({ state: 'visible' });
    assert.equal(mobile.url(), reader);
    await login.getByLabel('Пароль', { exact: true }).fill(password);
    const signedIn = mobile.waitForResponse(r => r.url() === base + '/login' && r.request().method() === 'POST');
    await login.getByRole('button', { name: 'Войти', exact: true }).click();
    assert.equal((await signedIn).status(), 204);
    await mobile.locator('[data-chapter-comment-form]').filter({
      has: mobile.getByText('Комментарий к главе', { exact: true })
    }).waitFor();
    assert.equal(mobile.url(), reader);
    assert.equal(await mobile.locator('#tor-auth-dialog').count(), 0);
    await mobile.close();

    const noJs = await browser.newPage({ javaScriptEnabled: false, viewport: { width: 1440, height: 900 } });
    await noJs.goto(base + '/');
    await noJs.locator('.site-header__auth a[href="/login"]').click();
    await noJs.waitForURL('**/login');
    await noJs.locator('form[action="/login"]').waitFor();
    await noJs.getByRole('link', { name: 'Зарегистрироваться', exact: true }).click();
    await noJs.waitForURL('**/register');
    await noJs.locator('form[action="/register"]').waitFor();
    await noJs.close();
    console.log('TOR_AUTH_DIALOG_BROWSER_OK');
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
