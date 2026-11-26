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
    const desktop = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    const destination = base + '/books/1?dialog-test=1#main';
    await desktop.goto(destination);
    const dialog = await openDialog(desktop);
    await desktop.keyboard.press('Escape');
    await dialog.waitFor({ state: 'hidden' });
    assert.equal(desktop.url(), destination);
    await openDialog(desktop);
    if (shots) await desktop.screenshot({ path: path.join(shots, 'auth-dialog-desktop.png') });
    await dialog.getByRole('button', { name: 'Регистрация', exact: true }).click();
    await dialog.locator('form[action="/register"]').waitFor();
    await dialog.getByLabel('Отображаемое имя').fill('Auth Dialog Reader');
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
    await desktop.close();

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
    await mobile.locator('[data-chapter-comment-form]').waitFor();
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
