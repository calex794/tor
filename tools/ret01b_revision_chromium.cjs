'use strict';
// RET-01b: real Chromium acceptance for the live actor-scoped revision shelf.
const assert = require('node:assert/strict');
const { chromium } = require('playwright');

(async () => {
  const origin = process.env.RET01_BROWSER_ORIGIN;
  const session = process.env.RET01_BROWSER_SESSION;
  const csrf = process.env.RET01_BROWSER_CSRF;
  const chapter = Number(process.env.RET01_BROWSER_CHAPTER);
  const notice = Number(process.env.RET01_BROWSER_NOTICE);
  assert.match(origin || '', /^http:\/\/127\.0\.0\.1:3098$/);
  assert(session && csrf && Number.isSafeInteger(chapter) && chapter > 0);
  assert(Number.isSafeInteger(notice) && notice > 0);
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [width, height] of [[1440, 900], [390, 844]]) {
      const context = await browser.newContext({ viewport: { width, height } });
      await context.addCookies([
        { name: 'tor-dev-session', value: session, url: origin },
        { name: 'tor-dev-csrf', value: csrf, url: origin },
      ]);
      const page = await context.newPage();
      const response = await page.goto(origin + '/me/library?view=updates');
      assert.equal(response.status(), 200, 'private Library must be rendered');
      await page.getByRole('heading', { name: 'Обновились', exact: true }).waitFor();
      const revised = page.locator('[data-revision-notice-id="' + notice + '"]');
      assert.equal(await revised.count(), 1, 'one revision event, not a duplicate first release');
      const form = revised.locator('form[action="/notifications/' + notice + '/read"]');
      assert.equal(await form.count(), 1);
      assert.equal(await form.locator('[name="next"]').inputValue(), '/read/' + chapter);
      assert.equal(await form.locator('[name="csrf_token"]').inputValue(), csrf);
      assert(!(await revised.innerText()).includes('RET01 revision body V2'),
        'never place free chapter body in a notification');
      if (width === 390) {
        assert.match(await revised.innerText(), /Просмотрено/,
          'mobile reflects the exact event acknowledged by desktop');
      }

      const bell = await page.goto(origin + '/notifications?tab=revisions');
      assert.equal(bell.status(), 200);
      assert.equal(await page.locator('[data-notification-kind="chapter_updated"]').count(), 1);
      assert.equal(await page.locator('[data-notification-kind="chapter_created"]').count(), 0);

      if (width === 1440) {
        const bad = await context.request.post(origin + '/notifications/' + notice + '/read', {
          headers: { Origin: origin },
          form: { csrf_token: 'invalid', next: '/read/' + chapter },
          maxRedirects: 0,
        });
        assert.equal(bad.status(), 403, 'invalid CSRF never acknowledges revision');
        await page.goto(origin + '/me/library?view=updates');
        const click = page.locator('[data-revision-notice-id="' + notice + '"] button[type="submit"]');
        await Promise.all([
          page.waitForURL(url => url.pathname === '/read/' + chapter),
          click.click(),
        ]);
        assert.equal(new URL(page.url()).pathname, '/read/' + chapter);
      }
      await context.close();
    }
    console.log('RET-01b real Chromium PASS: 1440/390, CSRF, single V2, private reader');
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error('RET-01b Chromium failed:', String(error.message).replace(/https?:\/\/\S+/g, '[url]'));
  process.exitCode = 1;
});
