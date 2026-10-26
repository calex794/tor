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
async function search(page, { q = '', genre = '', language = '' }) {
  const res = await page.goto(base + '/catalog', { waitUntil: 'networkidle' });
  assert(res && res.status() === 200, 'catalog HTTP failed');
  await page.getByLabel('Название или автор').fill(q);
  await page.getByLabel('Жанр', { exact: true }).fill(genre);
  await page.getByLabel('Язык перевода').fill(language);
  await page.getByRole('button', { name: 'Найти' }).click();
  await page.waitForLoadState('networkidle');
  assert(new URL(page.url()).pathname === '/catalog', 'search form navigation failed');
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
        const filters = { q: 'АвторПоиска', genre: 'фэнтези', language: 'ru' };
        await search(guest, filters);
        assert(await bookLink(guest, id).count() === 0, viewport + ': draft leaked to HTML');
        assert(!(await apiHas(guest, id, filters)), viewport + ': draft leaked to JSON');

        await page.getByRole('button', { name: 'Опубликовать книгу' }).click();
        await page.waitForURL(base + '/books/' + id);
        await search(guest, filters);
        assert(await bookLink(guest, id).isVisible(), viewport + ': public book not found');
        assert(await apiHas(guest, id, filters), viewport + ': public JSON not found');
        await snapshot(guest, viewport + '-published');

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
        const detective = { q: 'АвторПоиска', genre: 'детектив', language: 'ru' };
        await search(guest, detective);
        assert(await bookLink(guest, id).isVisible(), viewport + ': new genre missing');
        assert(await apiHas(guest, id, detective), viewport + ': updated genre missing from JSON');
        await snapshot(guest, viewport + '-genre-updated');

        for (const phase of ['hold', 'draft', 'deleted']) {
          changeBook(id, phase);
          await guest.reload({ waitUntil: 'networkidle' });
          assert(await bookLink(guest, id).count() === 0, viewport + ': ' + phase + ' in HTML');
          assert(!(await apiHas(guest, id, detective)), viewport + ': ' + phase + ' in JSON');
          const forbidden = await guest.goto(base + '/books/' + id);
          assert(forbidden && forbidden.status() === 404,
            viewport + ': ' + phase + ' public book detail accessible');
          await guest.goto(base + '/catalog?' + new URLSearchParams(detective));
        }
        console.log('F03 search ' + viewport + ' PASS: real publication, author/genre/language, live revocation');
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
