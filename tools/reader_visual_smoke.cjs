const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');

const base = 'http://127.0.0.1:3098';
const out = process.env.TOR_READER_SHOTS || 'reader-shots';
fs.mkdirSync(out, { recursive: true });

function assert(ok, why) {
  if (!ok) throw new Error(why);
}

async function inspect(page, name, where) {
  const metrics = await page.evaluate(() => ({
    content: document.documentElement.scrollWidth,
    viewport: document.documentElement.clientWidth,
    body: document.body.scrollWidth
  }));
  assert(metrics.content <= metrics.viewport + 1 && metrics.body <= metrics.viewport + 1,
    name + ': ' + where + ' horizontal overflow ' + JSON.stringify(metrics));
  assert(await page.locator('h1').isVisible(), name + ': missing h1');
  await page.screenshot({ path: path.join(out, name + '-' + where + '.png'), fullPage: true });
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [name, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 1 });
      const book = await page.goto(base + '/books/1', { waitUntil: 'networkidle' });
      assert(book && book.status() === 200, name + ': public book missing');
      assert(await page.getByText('Глава 1. Начало истории').isVisible(), name + ': TOC invisible');
      assert(await page.getByText('Платная · пока недоступна').isVisible(), name + ': paywall label missing');
      assert(!(await page.content()).includes('PRIVATE-PAID-MARKER'), name + ': paid text in TOC');
      await inspect(page, name, 'toc');
      await page.getByRole('link', { name: /Глава 1\. Начало истории/ }).click();
      await page.waitForURL('**/reader/101');
      assert(await page.getByText('Первая строка').count() > 0, name + ': no actual text');
      const html = await page.content();
      assert(!html.includes('<script>alert(1)</script>'), name + ': unsafe author HTML');
      assert(html.includes('&lt;script&gt;'), name + ': escaped author HTML not displayed');
      assert(!html.includes('PRIVATE-PAID-MARKER'), name + ': paid body leaked');
      await inspect(page, name, 'reader');
      for (const denied of ['/reader/102', '/reader/103', '/books/2']) {
        const response = await page.goto(base + denied);
        assert(response && response.status() === 404, name + ': denied URL ' + denied);
      }
      await page.close();
    }
    console.log('TOR PG reader visual PASS: two viewports, four screenshots, ACL and no overflow');
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
