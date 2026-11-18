'use strict';
// RET-02: real browser evidence for private follow vs public FREE reading.
// The callback never sends a notification or fabricates a completion receipt.
const assert = require('node:assert/strict');
const { chromium } = require('playwright');

(async () => {
  const origin = process.env.RET02_ORIGIN;
  const chapter = Number(process.env.RET02_CHAPTER);
  const version = Number(process.env.RET02_VERSION);
  const csrf = process.env.RET02_CSRF;
  assert.equal(origin, 'http://127.0.0.1:3098');
  assert(Number.isSafeInteger(chapter) && chapter > 0);
  assert(Number.isSafeInteger(version) && version > 0);
  assert(csrf && process.env.RET02_SESSION_A && process.env.RET02_SESSION_B);

  const browser = await chromium.launch({ headless: true });
  try {
    const open = async (session, width, height) => {
      const context = await browser.newContext({ viewport: { width, height } });
      await context.addCookies([
        { name: 'tor-dev-session', value: session, url: origin },
        { name: 'tor-dev-csrf', value: csrf, url: origin }
      ]);
      const page = await context.newPage();
      const response = await page.goto(origin + '/read/' + chapter);
      assert.equal(response.status(), 200, 'public FREE does not require Follow');
      return { context, page };
    };

    const follower = await open(process.env.RET02_SESSION_A, 1440, 900);
    assert.match(await follower.page.locator('.reader-complete').innerText(),
      /Глава уже отмечена прочитанной/,
      'follower A returned and completed the real newly published chapter');
    assert.equal(await follower.page.locator('form[action="/read/' + chapter + '/complete"]').count(), 0);
    await follower.context.close();

    const observer = await open(process.env.RET02_SESSION_B, 390, 844);
    const form = observer.page.locator('form[action="/read/' + chapter + '/complete"]');
    assert.equal(await form.count(), 1,
      'B may complete public FREE without gaining a Follow notification');
    assert.equal(Number(await form.locator('[name="version_id"]').inputValue()), version);
    assert.equal(Number(await form.locator('[name="block_index"]').inputValue()), 1);
    assert.equal(Number(await form.locator('[name="char_offset"]').inputValue()),
      'C15 reader return'.length);
    const invalid = await observer.context.request.post(
      origin + '/read/' + chapter + '/complete',
      {
        headers: { Origin: origin },
        form: {
          csrf_token: 'invalid',
          version_id: String(version),
          block_index: '1',
          char_offset: String('C15 reader return'.length),
          expected_revision: '0'
        },
        maxRedirects: 0
      }
    );
    assert.equal(invalid.status(), 403, 'bad CSRF cannot fake a completion');
    await Promise.all([
      observer.page.waitForURL(url =>
        url.pathname === '/read/' + chapter && url.hash === '#continuation'),
      form.getByRole('button', { name: 'Глава прочитана' }).click()
    ]);
    assert.match(await observer.page.locator('.reader-complete').innerText(),
      /Глава уже отмечена прочитанной/,
      'mobile B has their own durable completion after real browser POST');
    assert.equal(await observer.page.locator('form[action="/read/' + chapter + '/complete"]').count(), 0);
    await observer.context.close();
    console.log('RET-02 Chromium PASS: desktop followed A, mobile unfollowed B, CSRF, real completion');
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error('RET-02 Chromium failed:', String(error.message).replace(/https?:\/\/\S+/g, '[url]'));
  process.exitCode = 1;
});
