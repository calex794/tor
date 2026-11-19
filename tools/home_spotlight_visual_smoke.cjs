// UI-06 / H-10: real PostgreSQL homepage, content first. The former
// non-advertising spotlight duplicated these books and has been retired.
// This test deliberately keeps the same CI entrypoint for browser acceptance.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');

const base = 'http://127.0.0.1:3098';
const shots = process.env.TOR_HOME_SHOTS || 'home-shots';
fs.mkdirSync(shots, { recursive: true });

function assert(ok, why) {
  if (!ok) throw new Error(why);
}
async function noOverflow(page, description) {
  const metrics = await page.evaluate(() => ({
    root: document.documentElement.scrollWidth,
    body: document.body.scrollWidth,
    viewport: document.documentElement.clientWidth,
  }));
  assert(metrics.root <= metrics.viewport + 1 && metrics.body <= metrics.viewport + 1,
    description + ': horizontal overflow ' + JSON.stringify(metrics));
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [name, width, height] of [
      ['small', 320, 740], ['mobile', 390, 844],
      ['tablet', 768, 900], ['desktop', 1440, 900],
    ]) {
      const page = await browser.newPage({
        viewport: { width, height }, deviceScaleFactor: 1,
      });
      const response = await page.goto(base + '/', { waitUntil: 'networkidle' });
      assert(response && response.status() === 200, name + ': home page unavailable');

      const firstShelf = page.locator('[data-tor-home-block="latest"]');
      assert(await firstShelf.count() === 1,
        name + ': exactly one canonical newest-books shelf is required');
      const cards = firstShelf.locator('.book-mini-card__link[href^="/books/"]');
      const count = await cards.count();
      assert(count >= 2, name + ': real public PostgreSQL books are missing');
      assert(await firstShelf.locator('a[href="/books/2"]').count() === 0,
        name + ': fixture draft book leaked into canonical public shelf');
      assert(await page.locator('.home-special-carousel, .home-spotlight-card, .home-showcase').count() === 0,
        name + ': obsolete duplicate/decorative hero was rendered');
      assert(await page.getByText('Топ дня', { exact: true }).count() === 0,
        name + ': fake daily ranking');
      assert(await page.getByText('Рекомендации для вас', { exact: true }).count() === 0,
        name + ': fake personalized recommendations');

      const search = page.getByRole('search', { name: 'Поиск книг' });
      const input = search.getByLabel('Найти книгу или автора');
      assert(await input.count() === 1, name + ': accessible real GET search missing');
      const inputBox = await input.boundingBox();
      const cardBox = await cards.first().boundingBox();
      assert(inputBox && cardBox && inputBox.y < cardBox.y &&
        cardBox.y < height - 80,
      name + ': empty introductory chrome pushes actual books out of first viewport');
      const linksBefore = await cards.evaluateAll(nodes =>
        nodes.map(a => a.getAttribute('href')));
      assert(linksBefore.length === new Set(linksBefore).size,
        name + ': duplicate copies of the same book in canonical shelf');

      // 3 styles x 2 manual themes use the *same* SSR public cards and links.
      // No fake card is allowed to appear merely because the design changes.
      for (const style of ['showcase', 'library', 'minimal']) {
        for (const theme of ['light', 'dark']) {
          await page.evaluate(({ style, theme }) => {
            document.documentElement.dataset.torStyle = style;
            document.documentElement.dataset.torTheme = theme;
          }, { style, theme });
          const linksAfter = await cards.evaluateAll(nodes =>
            nodes.map(a => a.getAttribute('href')));
          assert(JSON.stringify(linksAfter) === JSON.stringify(linksBefore),
            name + ': switching appearance changed book links');
          const display = await firstShelf.locator('.home-shelf__track')
            .evaluate(node => getComputedStyle(node).display);
          assert(display === (style === 'showcase' ? 'flex' : 'grid'),
            name + ': ' + style + ' lost its intended book layout');
          await noOverflow(page, name + ': ' + style + '/' + theme);
          const box = await cards.first().boundingBox();
          assert(box && box.y < height - 80,
            name + ': ' + style + '/' + theme + ' pushes books below the fold');
          if (style === 'library') {
            const genres = page.locator('.home-genre-navigation__link');
            assert(await genres.count() <= 5, name + ': genre chip limit exceeded');
            if (await genres.count()) {
              assert(await genres.first().isVisible(),
                name + ': real genres hidden by mobile layout');
              assert((await genres.first().getAttribute('href')).startsWith('/catalog?genre='),
                name + ': non-catalog genre destination');
            }
          }
          if (theme === 'light' && (width === 390 || width === 1440)) {
            await page.screenshot({
              path: path.join(shots, 'home-content-first-' + style + '-' + name + '.png'),
              fullPage: true,
            });
          }
        }
      }

      // Existing GET catalog route must return the exact public book and
      // continue denying draft titles after the homepage UI refactor.
      const title = await cards.first().locator('h3').innerText();
      const href = await cards.first().getAttribute('href');
      await input.fill(title);
      await search.getByRole('button', { name: 'Найти', exact: true }).click();
      await page.waitForURL(url => url.pathname === '/catalog' &&
        url.searchParams.get('q') === title);
      assert(await page.locator('.book-mini-card a[href="' + href + '"]').count() === 1,
        name + ': GET search failed to find public PostgreSQL book');
      assert(await page.locator('.book-mini-card a[href="/books/2"]').count() === 0,
        name + ': hidden draft leaked through search');
      await page.close();
    }

    // The canonical server-rendered homepage must be useful without JS.
    const noJs = await browser.newContext({
      javaScriptEnabled: false, viewport: { width: 390, height: 844 },
    });
    const fallback = await noJs.newPage();
    const response = await fallback.goto(base + '/', { waitUntil: 'networkidle' });
    assert(response && response.status() === 200, 'no-JS homepage unavailable');
    assert(await fallback.locator('[data-tor-home-block="latest"] .book-mini-card__link').count() >= 2,
      'no-JS fallback lost public book cards');
    assert(await fallback.getByRole('search', { name: 'Поиск книг' }).count() === 1,
      'no-JS fallback lost GET search');
    await noOverflow(fallback, 'no-JS public home');
    await noJs.close();

    console.log('UI-06 PostgreSQL content-first home PASS: 24 style/theme/width combinations, real search, no-JS');
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
