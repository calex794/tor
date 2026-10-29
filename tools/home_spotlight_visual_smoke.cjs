// H-10: real PostgreSQL public home spotlight, after the author browser
// publishes two books. No demo markup or private test routes.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const base = 'http://127.0.0.1:3098';
const shots = process.env.TOR_HOME_SHOTS || 'home-shots';
fs.mkdirSync(shots, { recursive: true });

function assert(ok, why) {
  if (!ok) throw new Error(why);
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [name, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const page = await browser.newPage({
        viewport: { width, height }, deviceScaleFactor: 1,
      });
      const response = await page.goto(base + '/', { waitUntil: 'networkidle' });
      assert(response && response.status() === 200, name + ': home page unavailable');
      const carousel = page.locator('.home-special-carousel__viewport');
      assert(await carousel.count() === 1, name + ': original NEwBOO spotlight viewport missing');
      const cards = page.locator('.home-special-carousel .home-spotlight-card');
      const count = await cards.count();
      assert(count >= 2 && count <= 5,
        name + ': expected recent public books from PostgreSQL, got ' + count);
      assert(await page.locator('.home-special-carousel a[href="/books/2"]').count() === 0,
        name + ': fixture draft book leaked into home showcase');
      assert(await page.getByText('Топ дня', { exact: true }).count() === 0,
        name + ': falsely advertises daily rating');
      assert(await page.getByText('Рекомендации для вас', { exact: true }).count() === 0,
        name + ': falsely advertises personalized recommendations');
      const metrics = await carousel.evaluate(node => {
        const first = node.querySelector('.home-spotlight-card');
        return {
          scroll: node.scrollWidth, visible: node.clientWidth,
          cover: first.querySelector('.home-spotlight-card__cover') !== null,
          snap: getComputedStyle(node).scrollSnapType,
          doc: document.documentElement.scrollWidth,
          viewport: document.documentElement.clientWidth,
          body: document.body.scrollWidth,
        };
      });
      assert(metrics.scroll > metrics.visible * 1.5,
        name + ': carousel cannot scroll: ' + JSON.stringify(metrics));
      assert(metrics.snap.includes('mandatory') && metrics.snap.includes('x'),
        name + ': mobile/desktop native scroll snapping missing');
      assert(metrics.cover, name + ': foreground book cover placeholder missing');
      assert(metrics.doc <= metrics.viewport + 1 && metrics.body <= metrics.viewport + 1,
        name + ': horizontal page overflow ' + JSON.stringify(metrics));
      await page.screenshot({
        path: path.join(shots, 'home-real-carousel-' + name + '-first.png'),
        fullPage: true,
      });
      await carousel.focus();
      assert(await carousel.evaluate(node => document.activeElement === node),
        name + ': keyboard cannot focus spotlight');
      await carousel.evaluate(node => node.scrollTo({ left: node.clientWidth, behavior: 'instant' }));
      await page.waitForFunction(() => {
        const node = document.querySelector('.home-special-carousel__viewport');
        return node && node.scrollLeft > node.clientWidth / 2;
      }, null, { timeout: 5000 });
      await page.screenshot({
        path: path.join(shots, 'home-real-carousel-' + name + '-second.png'),
        fullPage: true,
      });
      await page.close();
    }
    console.log('H-10 PostgreSQL home spotlight PASS: 390/1440, public-only, keyboard and scroll');
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
