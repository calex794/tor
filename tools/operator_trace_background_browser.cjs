'use strict';
// Existing S059 acceptance: real operator pages on the disposable HTTP server.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require('playwright');

async function main() {
  assert.equal(process.env.GITHUB_ACTIONS, 'true');
  const origin = new URL(process.env.TOR_S059_ORIGIN);
  assert.equal(origin.protocol, 'http:');
  assert.equal(origin.hostname, '127.0.0.1');
  assert.match(origin.port, /^[0-9]+$/);
  assert.equal(origin.pathname, '/');
  assert.equal(origin.username + origin.password + origin.search + origin.hash, '');
  const output = path.resolve(process.env.TOR_S059_OUTPUT);
  assert.equal(output, path.join(path.resolve(process.env.RUNNER_TEMP), 's059-pg'));
  const metadata = fs.lstatSync(output);
  assert(metadata.isDirectory() && !metadata.isSymbolicLink());
  const password = process.env.TOR_TEST_OPERATOR_PASSWORD;
  assert(password && password.length >= 32 && password.length <= 256);
  const cases = JSON.parse(process.env.TOR_S059_BACKGROUND_CASES);
  assert(Array.isArray(cases) && cases.length === 3);
  for (const value of cases) {
    assert.deepEqual(Object.keys(value).sort(), ['outcomes', 'trace']);
    assert.match(value.trace, /^[0-9a-f]{32}$/);
    assert(Array.isArray(value.outcomes) && value.outcomes.length >= 1);
    assert(value.outcomes.every(item => ['retried', 'completed', 'interrupted'].includes(item)));
  }
  const denied = await fetch(`${origin.origin}/ops/traces?id=${cases[0].trace}`,
    { signal: AbortSignal.timeout(5000) });
  assert.equal(denied.status, 401);
  assert.equal(denied.headers.get('cache-control'), 'no-store');
  const browser = await chromium.launch({ headless: true });
  try {
    for (const width of [1440, 390]) {
      const context = await browser.newContext({
        viewport: { width, height: 900 },
        httpCredentials: { username: 'operator', password },
      });
      try {
        await context.route('**/*', route => {
          const requested = new URL(route.request().url());
          return requested.origin === origin.origin ? route.continue() : route.abort();
        });
        const page = await context.newPage();
        page.setDefaultTimeout(10000);
        for (let index = 0; index < cases.length; index += 1) {
          const value = cases[index];
          const response = await page.goto(`${origin.origin}/ops/traces?id=${value.trace}`);
          assert(response && response.status() === 200);
          assert.equal(response.headers()['cache-control'], 'no-store');
          assert.equal(response.headers()['x-frame-options'], 'DENY');
          const outcomes = await page.locator('[data-background-outcome]').evaluateAll(
            nodes => nodes.map(node => node.getAttribute('data-background-outcome')));
          assert.deepEqual(outcomes.sort(), [...value.outcomes].sort());
          assert.equal(await page.locator('[data-background-matches]').getAttribute(
            'data-background-matches'), String(value.outcomes.length));
          const content = await page.locator('body').innerText();
          assert(content.includes(value.trace));
          for (const secret of ['s059-private-source.txt', 'S059_PRIVATE_IMPORT_BODY',
            's059-private-owner', 'staging/chapters/']) {
            assert(!content.includes(secret));
          }
          assert(await page.evaluate(() =>
            document.documentElement.scrollWidth <= window.innerWidth + 1));
          if (index === 0) {
            await page.screenshot({ path: path.join(output, `background-${width}.png`),
              fullPage: true });
          }
        }
      } finally {
        await context.close();
      }
    }
  } finally {
    await browser.close();
  }
  console.log('S059_BACKGROUND_OPERATOR_BROWSER_1440_390_OK');
}
main().catch(() => {
  // Do not serialize request headers, credentials or the whole browser context.
  console.error('S059 background operator browser acceptance failed');
  process.exitCode = 1;
});
