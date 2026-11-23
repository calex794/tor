'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { createRequire } = require('node:module');
const requireRepo = createRequire(path.join(process.cwd(), 'package.json'));
const { chromium } = requireRepo('playwright');
let stage = 'launch';

(async () => {
  const config = JSON.parse(fs.readFileSync(0, 'utf8'));
  stage = `mode=${config.mode} admin=${Boolean(config.admin)} saved=${Boolean(config.saved)}`;
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
      const context = await browser.newContext({ viewport: { width: config.width || 390, height: 844 }, javaScriptEnabled: config.mode !== 'no-js' });
      if (config.session) await context.addCookies([{ name: 'tor-dev-session', value: config.session, url: config.origin }]);
      const page = await context.newPage();
      let directGets = 0, storageBytes = 0;
      let signalFirstS3;
      const firstS3Finished = new Promise(resolve => { signalFirstS3 = resolve; });
      const s3Requests = new Map(), networkErrors = [];
      let canceledGets = 0, recoveredExactAborts = 0;
      let cdReceived = 0, cdFinished = 0, pwFinished = 0, pwUninspectable = 0;
      const provedUrls = new Set();
      const matchesChapterGet = (href, method) => {
        if (method !== 'GET') return false;
        const url = new URL(href);
        return url.origin === config.storage_origin &&
          url.pathname.startsWith('/tor-s349-new-free-ci/private/free-chapters/' + config.chapter + '/');
      };
      const acceptCompletedGet = (url, bytes) => {
        if (bytes <= 0 || provedUrls.has(url)) return;
        provedUrls.add(url);
        directGets++;
        storageBytes += bytes;
        signalFirstS3();
      };
      // Instrument two independent byte-counted Network witnesses. Chromium
      // CDP dataReceived is preferred; Playwright request.sizes() is the
      // fallback when a fast or cached resource lacks CDP completion data.
      // A disposed Playwright request is ignored, never counted as proof.
      page.on('requestfinished', request => {
        if (!matchesChapterGet(request.url(), request.method())) return;
        pwFinished++;
        void Promise.all([request.response(), request.sizes()])
          .then(([response, size]) => {
            if (response && response.status() === 200 &&
                size.responseBodySize > 0) {
              acceptCompletedGet(request.url(), size.responseBodySize);
            }
          })
          .catch(() => { pwUninspectable++; });
      });
      const cdp = await context.newCDPSession(page);
      await cdp.send('Network.enable');
      cdp.on('Network.requestWillBeSent', event => {
        if (!matchesChapterGet(event.request.url, event.request.method)) return;
        s3Requests.set(event.requestId, {
          url: event.request.url, status: null, bodyBytes: 0, expectedLength: null,
        });
      });
      cdp.on('Network.responseReceived', event => {
        const record = s3Requests.get(event.requestId);
        if (record) {
          record.status = event.response.status;
          const lengthHeader = Object.entries(event.response.headers || {})
            .find(([name]) => name.toLowerCase() === 'content-length');
          const length = lengthHeader && Number(lengthHeader[1]);
          record.expectedLength = Number.isSafeInteger(length) && length > 0 ? length : null;
        }
      });
      cdp.on('Network.dataReceived', event => {
        const record = s3Requests.get(event.requestId);
        if (record) {
          record.bodyBytes += event.dataLength;
          cdReceived++;
        }
      });
      cdp.on('Network.loadingFinished', event => {
        const record = s3Requests.get(event.requestId);
        if (!record) return;
        cdFinished++;
        s3Requests.delete(event.requestId);
        if (record.status === 200) {
          // Require actual byte data, using independent Playwright sizes if
          // Chromium did not provide dataReceived for this short response.
          acceptCompletedGet(record.url, record.bodyBytes);
        } else {
          networkErrors.push('S3 GET status=' + record.status);
          signalFirstS3();
        }
      });
      cdp.on('Network.loadingFailed', event => {
        const record = s3Requests.get(event.requestId);
        if (!record) return;
        s3Requests.delete(event.requestId);
        // Navigating between public/private or editor pages aborts an
        // in-flight, now-obsolete GET. This is not a transfer failure of the
        // chapter already rendered. Ignore *only* Chrome's explicit cancel;
        // the test still requires another completed HTTP 200 GET with bytes.
        if (event.errorText === 'net::ERR_ABORTED') {
          canceledGets++;
          // The admin renderer may cancel an in-flight fetch as soon as it
          // has consumed all source bytes. A CDP loadingFinished event is
          // then absent despite a complete HTTP 200 body. Accept this ONLY
          // when Network.dataReceived proves an exact, positive length
          // matching S3's Content-Length; never accept a partial cancel.
          if (record.status === 200 && record.expectedLength !== null
              && record.bodyBytes === record.expectedLength) {
            recoveredExactAborts++;
            acceptCompletedGet(record.url, record.bodyBytes);
          }
          return;
        }
        networkErrors.push('S3 GET failed: ' + event.errorText);
        signalFirstS3();
      });
      const html = await page.goto(config.admin ? `${config.origin}/read/admin/${config.chapter}` : config.saved
        ? `${config.origin}/me/library/saved/${config.chapter}` : config.private
          ? `${config.origin}/read/${config.chapter}` : `${config.origin}/reader/${config.chapter}`);
      assert.equal(html.status(), 200);
      assert(!(await html.text()).includes(config.marker));
      if (config.admin) {
        const article = page.locator('[data-admin-content]');
        await article.filter({ hasText: config.marker }).waitFor({ timeout: 20000 });
        if (config.history) {
          const history = page.getByLabel('История редакций', { exact: true });
          await history.selectOption(String(config.history));
          await page.waitForFunction(marker => {
            const article = document.querySelector('[data-admin-content]');
            return article.textContent.includes('S349_NEW_BODY_ONLY_IN_S3_47b935') && !article.textContent.includes(marker);
          }, 'V2_ONLY_MARKER');
        }
        const root = page.locator('[data-admin-chapter-reader]');
        const csrf = await root.getAttribute('data-csrf-token');
        const denied = await context.request.post(`${config.origin}/api/v1/reader/chapters/${config.chapter}/admin-unsealed`, {
          headers: { Origin: config.origin, 'X-CSRF-Token': 'x'.repeat(43) }, maxRedirects: 0,
        });
        assert.equal(denied.status(), 403);
        assert.match(csrf, /^[A-Za-z0-9_-]{43}$/);
      } else if (config.mode === 'no-js') {
        stage = 'no-JavaScript same-origin source';
        await Promise.all([
          page.waitForURL(url => url.origin === config.origin &&
            url.pathname === '/api/v1/chapters/' + config.chapter + '/free-grant' &&
            url.searchParams.get('format') === 'source'),
          page.getByRole('link', { name: 'Открыть текст без JavaScript', exact: true }).click(),
        ]);
        assert((await page.locator('body').innerText()).includes(config.marker));
        assert.equal(directGets, 0);
        await cdp.detach();
        process.stdout.write(JSON.stringify({ read: true, direct_s3_gets: directGets, storage_bytes: storageBytes }));
        await context.close();
        return;
      } else if (config.mode === 'read') {
        await page.locator('#chapter-text').filter({ hasText: config.marker }).waitFor({ timeout: 20000 });
        const text = await page.locator('#chapter-text').innerText();
        assert(text.includes(config.markdown.split('\n\n')[0]));
        if (config.absent_marker) assert(!text.includes(config.absent_marker));
        if (config.version) assert.equal(await page.locator('#chapter-text').getAttribute('data-free-s3-version'), String(config.version));
        if (config.verify_open) {
          await page.waitForFunction(() =>
            document.querySelector('#chapter-text')?.dataset.freeOpenConfirmed === '1',
            undefined, { timeout: 10000 });
        }
        if (config.archived) {
          const csrf = await page.locator('#chapter-text').getAttribute('data-free-s3-csrf');
          const endpoint = config.origin + '/api/v1/me/progress/archived-free';
          const current = await context.request.get(endpoint + `?chapter_id=${config.chapter}&version_id=${config.version}`);
          assert.equal(current.status(), 200, 'the saved free V1 remains a valid archive cursor after conversion');
          const state = await current.json();
          const saved = await context.request.put(endpoint, {
            headers: { Origin: config.origin, 'X-CSRF-Token': csrf },
            data: { chapter_id: String(config.chapter), version_id: String(config.version),
              block_id: 1, char_offset: 1, expected_revision: state.revision },
          });
          assert.equal(saved.status(), 200, 'only the exact pinned V1 cursor may advance');
          await page.reload();
          await page.locator('#chapter-text').filter({ hasText: config.marker }).waitFor();
          await page.waitForFunction(() => document.querySelector('#chapter-text')?.dataset.freeProgressRestored === '1');
          const notice = page.locator('[data-free-progress-notice]');
          assert(!(await notice.isVisible()), 'a valid archive must not display a progress failure');
        }

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
          assert.equal(await page.locator('#p-1').count(), 1, 'private resume links must target loaded source blocks');
          const [response] = await Promise.all([
            page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname.endsWith('/complete')),
            finish.evaluate(form => form.requestSubmit()),
          ]);
          assert.equal(response.status(), 303, 'completion must use B source metadata without PG plaintext');
          stage = 'public S3 selection tools';
          await page.goto(config.origin + '/reader/' + config.chapter);
          await page.waitForFunction(() => {
            const span = [...document.querySelectorAll('#chapter-text span[data-md-start]')]
              .find(node => node.textContent.startsWith('New chapter'));
            if (!span) return false;
            const range = document.createRange(); range.setStart(span.firstChild, 0); range.setEnd(span.firstChild, 11);
            const selection = window.getSelection(); selection.removeAllRanges(); selection.addRange(range);
            document.dispatchEvent(new Event('selectionchange'));
            return document.querySelector('[data-free-issue-quote]')?.value === 'New chapter' &&
              document.querySelector('[data-chapter-comment-quote]')?.value === 'New chapter';
          });
          const comment = page.locator('[data-chapter-comment-form]').first();
          assert.equal(await comment.locator('[name="anchor_block"]').inputValue(), '2');
          await comment.locator('[name="body"]').fill('S349 anchored comment.');
          const [commentResponse] = await Promise.all([
            page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname.endsWith('/comments')),
            comment.evaluate(form => form.requestSubmit()),
          ]);
          assert.equal(commentResponse.status(), 303, 'S3-only anchored comments must validate the actual source');
          stage = 'private S3 correction selection';
          await page.goto(config.origin + '/read/' + config.chapter);
          await page.waitForFunction(() => {
            const span = [...document.querySelectorAll('#chapter-text span[data-md-start]')]
              .find(node => node.textContent.startsWith('New chapter'));
            if (!span) return false;
            const range = document.createRange(); range.setStart(span.firstChild, 0); range.setEnd(span.firstChild, 11);
            const selection = window.getSelection(); selection.removeAllRanges(); selection.addRange(range);
            document.dispatchEvent(new Event('selectionchange'));
            return document.querySelector('#issue-quote')?.value === 'New chapter';
          });
          const issue = page.locator('.reader-issues form');
          assert.equal(await issue.locator('[name="block_index"]').inputValue(), '2');
          await issue.locator('[name="suggested_text"]').fill('Corrected chapter');
          const [issueResponse] = await Promise.all([
            page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname.endsWith('/text-issues')),
            issue.evaluate(form => form.requestSubmit()),
          ]);
          assert.equal(issueResponse.status(), 303, 'S3-only correction must validate the immutable source');
          await page.waitForURL(url => url.pathname === '/read/' + config.chapter && url.searchParams.get('issue') === 'sent');
          await page.locator('#chapter-text').filter({ hasText: config.marker }).waitFor();
        }
      } else {
        await page.locator('#chapter-text [role="alert"]').waitFor({ timeout: 20000 });
        assert(!(await page.locator('#chapter-text').innerText()).includes(config.marker));
      }
      if (process.env.TOR_PAID_READER_SHOTS && config.mode === 'read') {
        const surface = config.admin ? (config.history ? 'admin-v1' : 'admin-current')
          : config.archived ? 'archive' : config.saved ? 'saved-v1' : config.progress ? 'private' : 'public';
        fs.mkdirSync(process.env.TOR_PAID_READER_SHOTS, { recursive: true });
        await page.screenshot({ path: path.join(process.env.TOR_PAID_READER_SHOTS,
          `s349-free-${surface}-${config.width || 390}.png`), fullPage: true });
      }
      // The DOM may update before Chromium emits Network.loadingFinished.
      // Require the exact private chapter GET to finish, with real body bytes.
      let networkTimeout;
      try {
        await Promise.race([
          firstS3Finished,
          new Promise((_, reject) => {
            networkTimeout = setTimeout(() => reject(new Error(
              'real S3 GET never finished for ' + stage +
              ' [cdp_finished=' + cdFinished + ' cdp_data=' + cdReceived +
              ' pw_finished=' + pwFinished + ' pw_uninspectable=' + pwUninspectable +
              ' pending=' + s3Requests.size + ' aborted=' + canceledGets +
              ' exact_aborts=' + recoveredExactAborts + ']')), 15000);
          }),
        ]);
      } finally {
        clearTimeout(networkTimeout);
      }
      assert.deepEqual(networkErrors, [], 'all direct S3 GETs must finish successfully');
      assert(directGets >= 1,
        'browser must download the private chapter from the storage origin; canceled GETs=' + canceledGets);
      await cdp.detach();
      process.stdout.write(JSON.stringify({ storage_bytes: storageBytes,
        direct_s3_gets: directGets, corrupt_rejected: config.mode === 'corrupt', read: config.mode === 'read' || config.mode === 'no-js' }));
      await context.close();
    }
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error('S349 real browser scenario failed at ' + stage + ':', String(error.message).replace(/https?:\/\/\S+/g, '[url]'));
  process.exitCode = 1;
});
