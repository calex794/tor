// F-03: real Chromium owner publication -> anonymous search -> PG revocation.
// This script intentionally refuses to modify anything except disposable tor_ci.
const { chromium } = require('playwright');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const base = 'http://127.0.0.1:3098';
const dbUrl = process.env.APP_URL;
const shots = process.env.TOR_SEARCH_SHOTS || 'search-shots';
fs.mkdirSync(shots, { recursive: true });

function assert(ok, reason) {
  if (!ok) throw new Error(reason);
}
function pg(sql) {
  assert(dbUrl, 'F-03 browser test needs isolated APP_URL');
  return execFileSync('psql', ['-X', '-v', 'ON_ERROR_STOP=1', '-At', '-d', dbUrl, '-c', sql], {
    encoding: 'utf8', timeout: 10000, stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();
}
function changeBook(id, phase) {
  assert(/^[1-9][0-9]*$/.test(id), 'invalid book ID');
  const changes = {
    genre: "genres=ARRAY['детектив']",
    hold: 'admin_hold=TRUE',
    draft: "admin_hold=FALSE,visibility='draft'",
    deleted: "visibility='public',is_deleted=TRUE",
  };
  assert(Object.hasOwn(changes, phase), 'invalid phase');
  const result = pg('UPDATE books SET ' + changes[phase] + ' WHERE id=' + id +
    " AND owner_user_id IS NOT NULL AND source_identity LIKE 'manual:%'");
  assert(result === 'UPDATE 1', phase + ': expected one test-owned book update');
}
function bookLink(page, id) {
  return page.locator('.book-mini-card__link[href="/books/' + id + '"]');
}
async function apiHas(page, id, query) {
  const res = await page.request.get(base + '/api/v1/search/books?' + new URLSearchParams(query));
  assert(res.status() === 200, 'search API HTTP ' + res.status());
  const body = await res.json();
  assert(Array.isArray(body.items) && Object.hasOwn(body, 'next_cursor'), 'invalid search JSON');
  return body.items.some(book => book.id === id);
}
// The NEwBOO UI exposes taxonomy as native selects. Values hidden from the
// public taxonomy list cannot be chosen in the form; test these via direct URL.
async function setTaxonomy(page, label, value) {
  const control = page.getByLabel(label, { exact: true });
  const isSelect = await control.evaluate(el => el.tagName === 'SELECT');
  if (!isSelect) {
    await control.fill(value);
    return true;
  }
  const hasValue = await control.evaluate((el, selected) =>
    Array.from(el.options).some(option => option.value === selected), value);
  if (!hasValue) return false;
  await control.selectOption(value);
  return true;
}
async function search(page, { q = '', genre = '', language = '', tag = '', fandom = '' }) {
  const res = await page.goto(base + '/catalog', { waitUntil: 'networkidle' });
  assert(res && res.status() === 200, 'catalog HTTP failed');
  await page.getByLabel('Название или автор').fill(q);
  const available = [
    await setTaxonomy(page, 'Жанр', genre),
    await setTaxonomy(page, 'Тег', tag),
    await setTaxonomy(page, 'Фэндом', fandom),
  ];
  await page.getByLabel('Язык перевода').fill(language);
  if (available.every(Boolean)) {
    await page.getByRole('button', { name: 'Найти' }).click();
    await page.waitForLoadState('networkidle');
  } else {
    // Test direct GET rejection for unpublished/nonexistent taxonomy values.
    // Do not add hidden/private options to a public dropdown just for CI.
    const params = new URLSearchParams({ q, genre, language, tag, fandom });
    const direct = await page.goto(base + '/catalog?' + params, {
      waitUntil: 'networkidle',
    });
    assert(direct && direct.status() === 200, 'direct catalog filter HTTP failed');
  }
  assert(new URL(page.url()).pathname === '/catalog', 'search navigation failed');
}
async function snapshot(page, name) {
  const x = await page.evaluate(() => ({
    viewport: document.documentElement.clientWidth,
    doc: document.documentElement.scrollWidth,
    body: document.body.scrollWidth,
  }));
  assert(x.doc <= x.viewport + 1 && x.body <= x.viewport + 1,
    name + ': horizontal overflow ' + JSON.stringify(x));
  assert(await page.getByRole('heading', { name: 'Каталог', exact: true }).isVisible(),
    name + ': missing catalog heading');
  await page.screenshot({ path: path.join(shots, 'search-' + name + '.png'), fullPage: true });
}
// H-10: inspect the real PostgreSQL-backed BookCard, not an injected HTML
// mock. Visual proportions and author metadata follow original NEwBOO.
async function verifyOriginalBookCard(page, id, viewport) {
  const card = bookLink(page, id);
  assert(await card.isVisible(), viewport + ': real published book card not visible');
  assert((await card.locator('.book-mini-card__meta').allTextContents())
    .some(text => text.trim() === 'АвторПоиска'),
    viewport + ': original public author missing');
  const text = await card.innerText();
  assert(!text.includes('Язык:') && !text.includes('Жанры:') &&
    !text.includes('Лайков:') && !text.includes('Комментарии:'),
    viewport + ': invented metadata is polluting NEwBOO book card');
  const dimensions = await card.evaluate(node => {
    const cover = node.querySelector('.book-mini-card__cover');
    const title = node.querySelector('.book-mini-card__title');
    const author = node.querySelector('.book-mini-card__meta');
    const c = cover.getBoundingClientRect();
    const titleCss = getComputedStyle(title);
    const authorCss = getComputedStyle(author);
    return {
      ratio: c.width / c.height,
      titleClamp: titleCss.webkitLineClamp,
      titleOverflow: titleCss.overflow,
      authorOverflow: authorCss.textOverflow,
      authorWhiteSpace: authorCss.whiteSpace,
      container: node.getBoundingClientRect().toJSON(),
      viewport: document.documentElement.clientWidth,
      cardColor: getComputedStyle(node).backgroundColor,
      coverColor: getComputedStyle(cover).backgroundColor,
    };
  });
  assert(Math.abs(dimensions.ratio - 2 / 3) < 0.035,
    viewport + ': original NEwBOO 2:3 book cover lost');
  assert(dimensions.titleClamp === '2' && dimensions.titleOverflow === 'hidden',
    viewport + ': original two-line BookCard title lost');
  assert(dimensions.authorOverflow === 'ellipsis' &&
    dimensions.authorWhiteSpace === 'nowrap',
    viewport + ': original author ellipsis lost');
  assert(dimensions.container.right <= dimensions.viewport + 1,
    viewport + ': BookCard extends outside viewport');

  await page.screenshot({
    path: path.join(shots, 'newboo-book-card-' + viewport + '-light.png'),
    fullPage: true,
  });
  await page.emulateMedia({ colorScheme: 'dark' });
  const dark = await card.evaluate(node => {
    const c = node.querySelector('.book-mini-card__cover');
    return getComputedStyle(c).backgroundColor;
  });
  assert(dark !== dimensions.coverColor,
    viewport + ': NEwBOO BookCard dark background tokens not applied');
  await page.screenshot({
    path: path.join(shots, 'newboo-book-card-' + viewport + '-dark.png'),
    fullPage: true,
  });
  await page.emulateMedia({ colorScheme: 'light' });
}

async function draftBook(page, viewport) {
  const title = 'Книга поиска F03 ' + viewport;
  await page.goto(base + '/register');
  await page.getByLabel('Отображаемое имя').fill('Автор поиска ' + viewport);
  await page.getByLabel('Электронная почта').fill('f03.browser.' + viewport + '@example.test');
  await page.getByLabel('Пароль (12–128 символов)').fill('SearchBrowserPassword2026');
  await page.getByRole('button', { name: 'Создать учётную запись' }).click();
  await page.waitForURL('**/me');

  await page.goto(base + '/me/books/new');
  await page.getByLabel('Название книги').fill(title);
  await page.getByLabel('Автор произведения (необязательно)').fill('АвторПоиска');
  await page.getByLabel('Жанры через запятую (до 8)').fill('Фэнтези, Приключения');
  await page.getByLabel('Теги через запятую (до 8)').fill('f02tag-' + viewport);
  await page.getByLabel('Фэндомы через запятую (до 8)').fill('f02fandom-' + viewport);
  await page.getByLabel('Язык').fill('ru');
  await page.getByRole('button', { name: 'Создать книгу' }).click();
  await page.waitForURL(/\/me\/books\/\d+$/);
  const id = page.url().split('/').pop();
  assert(/^[1-9][0-9]*$/.test(id), 'author did not create canonical book ID');

  await page.getByRole('link', { name: 'Добавить главу' }).click();
  await page.waitForURL('**/chapters/new');
  await page.getByLabel('Номер главы').fill('1');
  await page.getByLabel('Название главы').fill('Открытая глава для поиска');
  await page.getByLabel('Текст главы').fill('Настоящая публикация через PostgreSQL.');
  await page.getByRole('button', { name: 'Сохранить главу' }).click();
  await page.waitForURL(base + '/me/books/' + id);
  assert(await page.getByRole('button', { name: 'Опубликовать книгу' }).isVisible(),
    'explicit publication action missing');
  return id;
}

(async () => {
  assert(pg('SELECT current_database()') === 'tor_ci', 'refusing non-test database');
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [viewport, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const author = await browser.newContext({ viewport: { width, height } });
      const guestContext = await browser.newContext({ viewport: { width, height } });
      const page = await author.newPage();
      const guest = await guestContext.newPage();
      try {
        const id = await draftBook(page, viewport);
        const filters = {
          q: 'АвторПоиска', genre: 'фэнтези', language: 'ru',
          tag: 'f02tag-' + viewport, fandom: 'f02fandom-' + viewport,
        };
        await search(guest, filters);
        assert(await bookLink(guest, id).count() === 0, viewport + ': draft leaked to HTML');
        assert(!(await apiHas(guest, id, filters)), viewport + ': draft leaked to JSON');
        // The searched URL can echo its explicitly supplied selection. Check
        // the unfiltered public taxonomy, not the caller's own query string.
        await guest.goto(base + '/catalog', { waitUntil: 'networkidle' });
        assert(await guest.locator('#book-tag option[value="f02tag-' + viewport + '"]').count() === 0,
          viewport + ': private tag leaked to public options');

        await page.getByRole('button', { name: 'Опубликовать книгу' }).click();
        await page.waitForURL(base + '/books/' + id);
        await search(guest, filters);
        assert(await bookLink(guest, id).isVisible(), viewport + ': public book not found');
        await verifyOriginalBookCard(guest, id, viewport);
        assert(await apiHas(guest, id, filters), viewport + ': public JSON not found');
        assert(await guest.locator('#book-tag option[value="f02tag-' + viewport + '"]').count() === 1,
          viewport + ': published tag missing from options');
        await search(guest, { ...filters, tag: 'not-the-tag' });
        assert(await bookLink(guest, id).count() === 0, viewport + ': wrong tag matched');
        await search(guest, { ...filters, fandom: 'not-the-fandom' });
        assert(await bookLink(guest, id).count() === 0, viewport + ': wrong fandom matched');
        await search(guest, filters);
        // F-02: real HTML total must agree with the explicitly requested API total.
        const countRes = await guest.request.get(
          base + '/api/v1/search/books?' +
          new URLSearchParams({ ...filters, sort: 'newest', include_total: 'true' })
        );
        assert(countRes.status() === 200, 'F02 count HTTP failed');
        const count = await countRes.json();
        assert(Number.isSafeInteger(count.total) && count.total >= 1,
          viewport + ': F02 count missing');
        assert((await guest.locator('.catalog-result-count').innerText()) ===
          'Найдено книг: ' + count.total,
          viewport + ': F02 HTML and JSON totals differ');
        await snapshot(guest, viewport + '-published');
        // F-04: the homepage uses the same live public rows, not cached draft metadata.
        const homeResponse = await guest.goto(base + '/', { waitUntil: 'networkidle' });
        assert(homeResponse && homeResponse.status() === 200, viewport + ': homepage unavailable');
        assert(await guest.locator('.home-genre-section').count() > 0,
          viewport + ': real genre shelves missing');
        assert(await guest.locator('a[href="/books/' + id + '"]').count() >= 2,
          viewport + ': newly published book not in latest/genre shelves');
        await search(guest, filters);

        // F-02: lexical sorting is a real catalog control, not just an API flag.
        for (const sort of ['title', 'author']) {
          await guest.getByLabel('Сортировка').selectOption(sort);
          await guest.getByRole('button', { name: 'Найти' }).click();
          await guest.waitForLoadState('networkidle');
          assert(new URL(guest.url()).searchParams.get('sort') === sort,
            viewport + ': lexical sort not applied');
          assert(await bookLink(guest, id).isVisible(),
            viewport + ': matching book vanished after ' + sort + ' sort');
        }

        // Switch language in the search FORM; the canonical translation language is immutable.
        await search(guest, { ...filters, language: 'en' });
        assert(await bookLink(guest, id).count() === 0, viewport + ': wrong language match');
        await search(guest, filters);
        assert(await bookLink(guest, id).isVisible(), viewport + ': language reselect failed');

        // No public metadata-edit UI exists yet. Simulate a B-owned committed
        // genre change against the disposable database; verify live F projection.
        changeBook(id, 'genre');
        await guest.reload({ waitUntil: 'networkidle' });
        assert(await bookLink(guest, id).count() === 0, viewport + ': old genre leaked');
        const detective = { ...filters, genre: 'детектив' };
        await search(guest, detective);
        assert(await bookLink(guest, id).isVisible(), viewport + ': new genre missing');
        assert(await apiHas(guest, id, detective), viewport + ': updated genre missing from JSON');
        await snapshot(guest, viewport + '-genre-updated');

        for (const phase of ['hold', 'draft', 'deleted']) {
          changeBook(id, phase);
          if (phase === 'hold') {
            const home = await guest.goto(base + '/', { waitUntil: 'networkidle' });
            assert(home && home.status() === 200, viewport + ': homepage after hold unavailable');
            assert(await guest.locator('a[href="/books/' + id + '"]').count() === 0,
              viewport + ': revoked book leaked into homepage/genre shelves');
            await guest.goto(base + '/catalog?' + new URLSearchParams(detective));
          }
          await guest.reload({ waitUntil: 'networkidle' });
          assert(await bookLink(guest, id).count() === 0, viewport + ': ' + phase + ' in HTML');
          assert(!(await apiHas(guest, id, detective)), viewport + ': ' + phase + ' in JSON');
          const forbidden = await guest.goto(base + '/books/' + id);
          assert(forbidden && forbidden.status() === 404,
            viewport + ': ' + phase + ' public book detail accessible');
          await guest.goto(base + '/catalog?' + new URLSearchParams(detective));
        }
        console.log('F03/F02 search ' + viewport + ' PASS: publication, author/genre/language/tag/fandom, revocation');
      } finally {
        await author.close();
        await guestContext.close();
      }
    }
  } finally {
    await browser.close();
  }
})().catch(err => {
  console.error(err.stack || err);
  process.exitCode = 1;
});
