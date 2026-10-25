const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
(async () => {
  const browser = await chromium.launch({ headless: true });
  const out = process.env.TOR_SCREENSHOT_DIR || 'screenshots';
  fs.mkdirSync(out, { recursive: true });
  try {
    for (const [name, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 1 });
      for (const [label, url] of [['home','/'], ['catalog','/catalog'], ['book','/books/1'], ['login','/login']]) {
        const response = await page.goto('http://127.0.0.1:3099' + url, { waitUntil: 'networkidle' });
        if (!response || response.status() !== 200) throw new Error(name + ': ' + url + ' HTTP != 200');
        if (!(await page.locator('h1').isVisible())) throw new Error(name + ': ' + url + ' missing visible h1');
        const dims = await page.evaluate(() => ({
          scroll: document.documentElement.scrollWidth,
          viewport: document.documentElement.clientWidth,
        }));
        if (dims.scroll > dims.viewport + 1) throw new Error(name + ': ' + url + ' horizontal overflow: ' + JSON.stringify(dims));
        await page.screenshot({ path: path.join(out, name + '-' + label + '.png'), fullPage: true });
      }
      if (name === 'mobile') {
        await page.goto('http://127.0.0.1:3099/catalog');
        if (!(await page.getByRole('link', { name: 'Все книги' }).isVisible())) throw new Error('Mobile navigation hidden');
      }
      await page.close();
    }
    const page = await browser.newPage();
    await page.goto('http://127.0.0.1:3099/catalog');
    if (await page.getByText('Закрытый черновик').count()) throw new Error('Draft leaked');
    await page.close();
    console.log('Visual smoke PASS: desktop + mobile, 8 screenshots, no horizontal overflow');
  } finally {
    await browser.close();
  }
})().catch(err => { console.error(err); process.exitCode = 1; });
