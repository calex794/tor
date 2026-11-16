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
    } else if (config.mode === 'edit') {
      const context = await browser.newContext();
      await context.addCookies([{ name: 'tor-dev-session', value: config.session, url: config.origin }]);
      const page = await context.newPage();
      const url = `${config.origin}/me/books/${config.book}/chapters/${config.chapter}/edit`;
      await page.goto(url);
      const form = page.locator(`form[action="/me/books/${config.book}/chapters/${config.chapter}/edit"]`);
      assert.equal(await form.locator('[name="body"]').inputValue(), config.previous);
      await form.locator('[name="body"]').fill(config.markdown);
      const fields = await form.evaluate(form => Object.fromEntries(new FormData(form)));
      const [response] = await Promise.all([
        page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname.endsWith('/edit')),
        form.evaluate(form => form.requestSubmit()),
      ]);
      assert.equal(response.status(), 303);
      const replay = await context.request.post(url, { form: fields, headers: { Origin: config.origin }, maxRedirects: 0 });
      assert.equal(replay.status(), 303);
      process.stdout.write(JSON.stringify({ edited: true, replay: true }));
      await context.close();
    } else {
      const context = await browser.newContext({ viewport: { width: config.width || 390, height: 844 } });
      if (config.session) await context.addCookies([{ name: 'tor-dev-session', value: config.session, url: config.origin }]);
      const page = await context.newPage();
      const checks = [];
      let directGets = 0;
      page.on('response', response => {
        const url = new URL(response.url());
        if (url.origin === config.storage_origin && response.request().method() === 'GET') directGets++;
        if (url.origin === site.origin && (url.pathname.startsWith('/reader/') || url.pathname.startsWith('/read/') || url.pathname.startsWith('/me/library/saved/') || url.pathname.includes('free-grant'))) {
          checks.push(response.body().then(bytes => {
            assert(!bytes.toString('utf8').includes(config.marker), 'TOR response must never carry chapter text');
          }));
        }
      });
      const html = await page.goto(config.saved
        ? `${config.origin}/me/library/saved/${config.chapter}` : `${config.origin}/reader/${config.chapter}`);
      assert.equal(html.status(), 200);
      assert(!(await html.text()).includes(config.marker));
      if (config.mode === 'read') {
        await page.locator('#chapter-text').filter({ hasText: config.marker }).waitFor({ timeout: 20000 });
        const text = await page.locator('#chapter-text').innerText();
        assert(text.includes(config.markdown.split('\n\n')[0]));
        if (config.absent_marker) assert(!text.includes(config.absent_marker));
        if (config.version) assert.equal(await page.locator('#chapter-text').getAttribute('data-free-s3-version'), String(config.version));

        assert.equal(await page.locator('#chapter-text script').count(), 0);
        if (config.progress) {
          const article = page.locator('#chapter-text');
          const csrf = await article.getAttribute('data-free-s3-csrf');
          const version = await article.getAttribute('data-free-s3-version');
          assert.match(csrf, /^[A-Za-z0-9_-]{43}$/);
          const saved = await context.request.put(config.origin + '/api/v1/me/progress/free', {
            headers: { Origin: config.origin, 'X-CSRF-Token': csrf },
            data: { chapter_id: String(config.chapter), version_id: version, block_id: 1, char_offset: 1, expected_revision: 0 },
          });
          assert.equal(saved.status(), 200, 'S3-only source must support canonical progress');
          const invalid = await context.request.put(config.origin + '/api/v1/me/progress/free', {
            headers: { Origin: config.origin, 'X-CSRF-Token': csrf },
            data: { chapter_id: String(config.chapter), version_id: version, block_id: 1, char_offset: 100000, expected_revision: 1 },
          });
          assert.equal(invalid.status(), 422, 'a forged S3 source boundary must be rejected');
          await page.goto(config.origin + '/read/' + config.chapter);
          await page.locator('#chapter-text').filter({ hasText: config.marker }).waitFor();
          const finish = page.locator(`form[action="/read/${config.chapter}/complete"]`);
          await finish.waitFor();
          const [response] = await Promise.all([
            page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname.endsWith('/complete')),
            finish.evaluate(form => form.requestSubmit()),
          ]);
          assert.equal(response.status(), 303, 'completion must use B source metadata without PG plaintext');
        }
        if (process.env.TOR_PAID_READER_SHOTS) {
          fs.mkdirSync(process.env.TOR_PAID_READER_SHOTS, { recursive: true });
          await page.screenshot({ path: path.join(process.env.TOR_PAID_READER_SHOTS, `s349-free-${config.width || 390}.png`), fullPage: true });
        }

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
