'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { createRequire } = require('node:module');
const requireRepo = createRequire(path.join(process.cwd(), 'package.json'));
const { chromium } = requireRepo('playwright');

(async () => {
  const config = JSON.parse(fs.readFileSync(0, 'utf8'));
  const site = new URL(config.origin);
  assert.equal(site.hostname, '127.0.0.1');
  const browser = await chromium.launch({ headless: true });
  try {
    if (config.mode === 'publish') {
      const context = await browser.newContext();
      await context.addCookies([{ name: 'tor-dev-session', value: config.session, url: config.origin }]);
      const page = await context.newPage();
      await page.goto(`${config.origin}/me/books/${config.book}/chapters/new`);
      const form = page.locator(`form[action="/me/books/${config.book}/chapters"]`);
      await form.locator('[name="title"]').fill('S349 browser chapter');
      await form.locator('[name="body"]').fill(config.markdown);
      const fields = await form.evaluate(element => Object.fromEntries(new FormData(element)));
      const [saved] = await Promise.all([
        page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname === `/me/books/${config.book}/chapters`),
        form.evaluate(element => element.requestSubmit()),
      ]);
      assert.equal(saved.status(), 303, 'author form must commit a verified S3 publication');
      const replay = await context.request.post(`${config.origin}/me/books/${config.book}/chapters`, {
        form: fields, headers: { Origin: config.origin }, maxRedirects: 0,
      });
      assert.equal(replay.status(), 303, 'an identical retry must not create another chapter');
      const denied = await context.request.post(`${config.origin}/me/books/${config.book}/chapters`, {
        form: { ...fields, csrf_token: 'x'.repeat(43) }, headers: { Origin: config.origin }, maxRedirects: 0,
      });
      assert.equal(denied.status(), 403, 'invalid CSRF must be rejected');
      const toc = await context.request.get(`${config.origin}/api/v1/books/${config.book}/toc`);
      assert.equal(toc.status(), 200);
      const entries = (await toc.json()).items;
      assert.equal(entries.length, 1, 'save replay must leave exactly one chapter');
      process.stdout.write(JSON.stringify({ chapter_id: entries[0].chapter_id, published: true, replay: true, csrf_denied: true }));
      await context.close();
    } else {
      const context = await browser.newContext({ viewport: { width: 390, height: 844 } });
      const page = await context.newPage();
      const checks = [];
      let directGets = 0;
      page.on('response', response => {
        const url = new URL(response.url());
        if (url.origin === config.storage_origin && response.request().method() === 'GET') directGets++;
        if (url.origin === site.origin && (url.pathname.startsWith('/reader/') || url.pathname.includes('free-grant'))) {
          checks.push(response.body().then(bytes => {
            assert(!bytes.toString('utf8').includes(config.marker), 'TOR response must never carry chapter text');
          }));
        }
      });
      const html = await page.goto(`${config.origin}/reader/${config.chapter}`);
      assert.equal(html.status(), 200);
      assert(!(await html.text()).includes(config.marker));
      if (config.mode === 'read') {
        await page.locator('#chapter-text').filter({ hasText: config.marker }).waitFor({ timeout: 20000 });
        const text = await page.locator('#chapter-text').innerText();
        assert(text.includes(config.markdown.split('\n\n')[0]));
        assert.equal(await page.locator('#chapter-text script').count(), 0);
      } else {
        await page.locator('#chapter-text [role="alert"]').waitFor({ timeout: 20000 });
        assert(!(await page.locator('#chapter-text').innerText()).includes(config.marker));
      }
      await Promise.all(checks);
      assert(directGets >= 1, 'browser must download directly from the storage origin');
      process.stdout.write(JSON.stringify({ checked_site_responses: checks.length, direct_s3_gets: directGets, corrupt_rejected: config.mode === 'corrupt', read: config.mode === 'read' }));
      await context.close();
    }
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error('S349 real browser scenario failed:', String(error.message).replace(/https?:\/\/\S+/g, '[url]'));
  process.exitCode = 1;
});
