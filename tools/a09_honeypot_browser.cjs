// A-09: exercise real HTML decoys without changing shared-IP/account state.
const assert = require('node:assert/strict');

async function assertDecoy(page, action) {
  const field = page.locator(`form[action="${action}"] input[name="website"]`);
  assert.equal(await field.count(), 1);
  assert.equal(await field.getAttribute('tabindex'), '-1');
  assert.equal(await field.getAttribute('autocomplete'), 'off');
  assert.equal(await field.locator('..').getAttribute('aria-hidden'), 'true');
  assert.equal(await field.inputValue(), '');
  // Keyboard users must reach the real fields without entering the decoy.
  await page.locator(`form[action="${action}"] [name="email"]`).focus();
  await page.keyboard.press('Tab');
  assert.equal(await page.evaluate(() => document.activeElement.name), 'password');
}

module.exports = async function honeypotBrowser(browser, base, email, password) {
  const context = await browser.newContext({viewport: {width: 390, height: 844}});
  try {
    const page = await context.newPage();
    await page.goto(`${base}/register`);
    await assertDecoy(page, '/register');
    await page.locator('[name=display_name]').fill('Rejected decoy');
    await page.locator('[name=email]').fill(email);
    await page.locator('[name=password]').fill(password);
    await page.locator('input[name=website]').evaluate(el => { el.value = 'bot-filled'; });
    const [rejected] = await Promise.all([
      page.waitForResponse(r => r.request().method() === 'POST' && r.url() === `${base}/register`),
      page.locator('form[action="/register"] button').click(),
    ]);
    assert.equal(rejected.status(), 400);
    assert.equal((await context.request.get(`${base}/api/v1/me`)).status(), 401);
    const trap = await context.request.get(`${base}/__bp_honeypot/admin/private`);
    assert.equal(trap.status(), 404);
    assert.equal(await trap.text(), '');
    assert.equal(trap.headers()['cache-control'], 'no-store');
    assert.equal(trap.headers()['x-robots-tag'], 'noindex, nofollow, noarchive');
    // Caller now registers this same email normally: rejection cannot reserve it.
  } finally {
    await context.close();
  }
};
module.exports.assertDecoy = assertDecoy;

module.exports.rejectedLogin = async function rejectedLogin(browser, base, email, password) {
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    await page.goto(`${base}/login`);
    await assertDecoy(page, '/login');
    await page.locator('[name=email]').fill(email);
    await page.locator('[name=password]').fill(password);
    await page.locator('input[name=website]').evaluate(el => { el.value = 'bot-filled'; });
    const [rejected] = await Promise.all([
      page.waitForResponse(r => r.request().method() === 'POST' && r.url() === `${base}/login`),
      page.locator('form[action="/login"] button').click(),
    ]);
    assert.equal(rejected.status(), 401);
    assert.equal((await context.request.get(`${base}/api/v1/me`)).status(), 401);
    // Caller logs in normally afterwards: no IP or account ban is permitted.
  } finally {
    await context.close();
  }
};
