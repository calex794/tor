// UI-06: real PostgreSQL-backed content-first homepage. Replaces the old
// carousel smoke; the redundant, non-ad spotlight was intentionally removed.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const base = 'http://127.0.0.1:3098';
const shots = process.env.TOR_HOME_SHOTS || 'home-shots';
fs.mkdirSync(shots, { recursive: true });

function assert(ok, why) {
  if (!ok) throw new Error(why);
}

async function checkMode(browser, style, width, height) {
  const name = style + '-' + width;
  const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 1 });
  try {
    // Exercise the persisted appearance setting before the first page paint.
    await page.addInitScript(mode => {
      try {
        localStorage.setItem('torAppearance.v1', JSON.stringify({
          style: mode, theme: 'system', density: 'normal', catalog: 'grid',
        }));
      } catch (_) { /* storage denial tested in UI-01 */ }
    }, style);
    const response = await page.goto(base + '/', { waitUntil: 'networkidle' });
    assert(response && response.status() === 200, name + ': homepage unavailable');
    assert(await page.locator('.home-special-carousel, .home-special').count() === 0,
      name + ': duplicate hero returned');
    assert(await page.locator('.home-hero h1').count() === 1,
      name + ': semantic H1 missing');
    assert(await page.locator('.home-hero + [data-tor-home-blocks]').count() === 1,
      name + ': shelves do not directly follow the search');

    const search = page.getByRole('search', { name: 'Поиск книг' });
    const input = search.getByRole('searchbox', { name: 'Найти книгу или автора' });
    const submit = search.getByRole('button', { name: 'Найти', exact: true });
    assert(await input.count() === 1 && await submit.count() === 1,
      name + ': native accessible GET search missing');
    const latest = page.locator('[data-tor-home-blocks] > [data-tor-home-block="latest"]');
    const cards = latest.locator('.book-mini-card__link[href^="/books/"]');
    const count = await cards.count();
    assert(count >= 2, name + ': expected public PostgreSQL books, got ' + count);
    assert(await latest.locator('.book-mini-card__link[href="/books/2"]').count() === 0,
      name + ': fixture draft leaked into homepage');

    const metrics = await page.evaluate(() => {
      const hero = document.querySelector('.home-hero');
      const shelf = document.querySelector('[data-tor-home-block="latest"]');
      const first = shelf.querySelector('.book-mini-card__link');
      const h1 = hero.querySelector('h1');
      return {
        mode: document.documentElement.dataset.torStyle,
        viewport: document.documentElement.clientWidth,
        doc: document.documentElement.scrollWidth,
        body: document.body.scrollWidth,
        heroBottom: hero.getBoundingClientRect().bottom,
        shelfTop: shelf.getBoundingClientRect().top,
        firstTop: first.getBoundingClientRect().top,
        h1Width: h1.getBoundingClientRect().width,
        h1Height: h1.getBoundingClientRect().height,
      };
    });
    assert(metrics.mode === style, name + ': saved style not applied');
    assert(metrics.doc <= metrics.viewport + 1 && metrics.body <= metrics.viewport + 1,
      name + ': horizontal page overflow ' + JSON.stringify(metrics));
    assert(metrics.shelfTop - metrics.heroBottom < 80,
      name + ': excessive gap before books ' + JSON.stringify(metrics));
    assert(metrics.shelfTop < height - 80 && metrics.firstTop < height,
      name + ': books pushed below first screen ' + JSON.stringify(metrics));
    assert(metrics.h1Width <= 2 && metrics.h1Height <= 2,
      name + ': decorative heading occupies screen area');

    const genre = page.locator('.home-genre-navigation');
    if (await genre.count()) {
      assert(await genre.isVisible() === (style === 'library'),
        name + ': genre shortcuts must display only in library design');
      if (style === 'library') {
        const links = genre.locator('a[href^="/catalog?genre="]');
        const n = await links.count();
        assert(n >= 1 && n <= 5, name + ': unbounded genre shortcuts');
        await links.first().focus();
        assert(await links.first().evaluate(node => document.activeElement === node),
          name + ': genre link is not keyboard focusable');
      }
    }
    await input.focus();
    await page.keyboard.press('Tab');
    assert(await submit.evaluate(node => document.activeElement === node),
      name + ': Tab order from search to submit broken');
    await cards.first().focus();
    assert(await cards.first().evaluate(node => document.activeElement === node),
      name + ': real book link is not keyboard focusable');

    if (width === 390 || width === 1440) {
      await page.screenshot({
        path: path.join(shots, 'home-content-' + name + '.png'), fullPage: true,
      });
    }
    // Verify a real catalog query for one published PostgreSQL book.
    if (style === 'showcase' && width === 390) {
      const first = cards.first();
      const href = await first.getAttribute('href');
      const title = await first.locator('h3').innerText();
      await input.fill(title);
      await submit.click();
      await page.waitForURL(url =>
        url.pathname === '/catalog' && url.searchParams.get('q') === title
      );
      assert(await page.locator('.book-mini-card a[href="' + href + '"]').count() >= 1,
        name + ': catalog did not find the actual published book');
      assert(await page.locator('.book-mini-card a[href="/books/2"]').count() === 0,
        name + ': draft leaked into catalog search');
    }
  } finally {
    await page.close();
  }
}

async function checkNoJs(browser) {
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 }, javaScriptEnabled: false,
  });
  try {
    const page = await context.newPage();
    const response = await page.goto(base + '/', { waitUntil: 'load' });
    assert(response && response.status() === 200, 'no-JS: homepage unavailable');
    assert(await page.getByRole('searchbox', { name: 'Найти книгу или автора' }).count() === 1,
      'no-JS: server search missing');
    assert(await page.locator('[data-tor-home-block="latest"] .book-mini-card__link').count() >= 2,
      'no-JS: real book cards missing');
  } finally {
    await context.close();
  }
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const style of ['showcase', 'library', 'minimal']) {
      for (const [width, height] of [[320, 720], [390, 844], [768, 960], [1440, 900]]) {
        await checkMode(browser, style, width, height);
      }
    }
    await checkNoJs(browser);
    console.log('UI-06 PG home PASS: 3 styles, 320/390/768/1440, keyboard, GET search, no-JS');
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
