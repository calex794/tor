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
      const pageOne = await page.goto(base + '/books/1?limit=1');
      assert(pageOne && pageOne.status() === 200, name + ': paginated book HTTP');
      assert(await page.getByRole('link', { name: 'Следующие главы' }).isVisible(),
        name + ': missing keyset navigation');
      await page.getByRole('link', { name: 'Следующие главы' }).click();
      await page.waitForURL(/after=1\.101#chapters$/);
      assert(await page.getByText('Глава 3. Платная глава').isVisible(),
        name + ': second keyset page invisible');
      assert(!(await page.content()).includes('PRIVATE-PAID-MARKER'),
        name + ': paid body leaked in keyset page');
      await page.goto(base + '/books/1');
      await page.getByRole('link', { name: /Глава 1\. Начало истории/ }).click();
      await page.waitForURL('**/reader/101');
      assert(await page.locator('.reader-public .reader-topbar__back[href="/books/1"]').isVisible(),
        name + ': NEwBOO reader back link missing');
      assert(await page.locator('.reader-public .reader-topbar__title').isVisible(),
        name + ': reader topbar missing');
      assert(await page.locator('.reader-public .reader-page__title').isVisible(),
        name + ': original chapter heading missing');
      assert(await page.locator('.reader-public .reader-page__meta').getByText('Бесплатная глава', { exact: false }).isVisible(),
        name + ': published free chapter label missing');
      assert(!(await page.content()).includes('data-reader-theme-cycle'),
        name + ': unimplemented reader settings displayed');
      assert(await page.getByText('Первая строка').count() > 0, name + ': no actual text');
      const html = await page.content();
      assert(!html.includes('<script>alert(1)</script>'), name + ': unsafe author HTML');
      assert(html.includes('&lt;script&gt;'), name + ': escaped author HTML not displayed');
      assert(!html.includes('PRIVATE-PAID-MARKER'), name + ': paid body leaked');
      assert(html.includes('д'.repeat(1200)), name + ': long Unicode fixture missing');
      await inspect(page, name, 'reader');
      for (const url of [
        '/api/v1/chapters/101/read',
        '/api/v1/chapters/103/read?purchase_id=999',
        '/api/v1/chapters/102/read?preview=1'
      ]) {
        const denied = await page.goto(base + url);
        assert(denied && denied.status() === 404, name + ': alternative reader access ' + url);
        assert((denied.headers()['cache-control'] || '').includes('no-store'),
          name + ': cacheable alternative reader denial ' + url);
        assert(!(await page.content()).includes('PRIVATE-PAID-MARKER'),
          name + ': private marker leaked from ' + url);
      }
      for (const denied of ['/reader/102', '/reader/103', '/books/2', '/api/v1/books/2/toc']) {
        const response = await page.goto(base + denied);
        assert(response && response.status() === 404, name + ': denied URL ' + denied);
        assert((response.headers()['cache-control'] || '').includes('no-store'),
          name + ': shared-cacheable denied URL ' + denied);
      }
      await page.close();
    }
    // Exercise real PostgreSQL account forms on both target viewport sizes.
    for (const [name, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 1 });
      await page.goto(base + '/register');
      assert(await page.getByRole('heading', { name: 'Регистрация' }).isVisible(), name + ': registration form hidden');
      assert(await page.locator('.auth-card--newboo .auth-panda__svg').isVisible(),
        name + ': original NEwBOO panda missing on registration');
      assert(await page.locator('.auth-card__kicker').getByText('Пандаолейт').isVisible(),
        name + ': original auth header missing');
      assert(await page.locator('form[action="/register"][novalidate]').count() === 1,
        name + ': browser-native English validation must be disabled');
      await inspect(page, name, 'register');
      await page.getByLabel('Отображаемое имя').fill('Читатель ' + name);
      await page.getByLabel('Электронная почта').fill('visual.' + name + '@example.test');
      await page.getByLabel('Пароль (12–128 символов)').fill('1234');
      const invalidPost = page.waitForResponse(response =>
        response.url().endsWith('/register') && response.request().method() === 'POST'
      );
      await page.getByRole('button', { name: 'Создать учётную запись' }).click();
      const invalidResponse = await invalidPost;
      assert(invalidResponse.status() === 400, name + ': expected server-side weak-password rejection');
      assert(await page.getByRole('alert').getByText('Пароль должен содержать от 12 до 128 символов').isVisible(),
        name + ': Russian password feedback missing');
      assert(await page.getByLabel('Электронная почта').inputValue() === 'visual.' + name + '@example.test',
        name + ': email lost after validation error');
      assert(await page.getByLabel('Пароль (12–128 символов)').inputValue() === '',
        name + ': submitted password must not be replayed');
      await inspect(page, name, 'register-validation');
      await page.getByLabel('Пароль (12–128 символов)').fill('TestPassphrase2026');
      const beforeCookies = await page.context().cookies(base);
      const beforeCsrf = beforeCookies.find(c => c.name === 'tor-dev-csrf');
      const beforeToken = await page.locator('input[name="csrf_token"]').inputValue();
      // Never print CSRF/session values. Metadata is sufficient to debug cookies.
      const before = {
        csrfCookie: Boolean(beforeCsrf),
        hasMatchingToken: beforeCsrf ? beforeCsrf.value === beforeToken : false,
        tokenLength: beforeToken.length,
        cookieNames: beforeCookies.map(c => c.name),
      };
      const submitted = page.waitForResponse(response =>
        response.url().endsWith('/register') && response.request().method() === 'POST'
      );
      await page.getByRole('button', { name: 'Создать учётную запись' }).click();
      const registerResponse = await submitted;
      if (registerResponse.status() !== 303) {
        const headers = await registerResponse.request().allHeaders();
        const requestCookieNames = (headers.cookie || '').split(';')
          .map(x => x.trim().split('=')[0]).filter(Boolean);
        throw new Error(name + ': register HTTP ' + registerResponse.status() +
          ', diagnostics ' + JSON.stringify({
            ...before,
            origin: headers.origin || null,
            fetchSite: headers['sec-fetch-site'] || null,
            cookieNamesInRequest: requestCookieNames,
            postsCsrf: (registerResponse.request().postData() || '').includes('csrf_token=')
          }));
      }
      await page.waitForURL('**/me', { timeout: 10000 });
      assert(await page.getByText('Читатель ' + name).isVisible(), name + ': profile missing');
      assert(await page.getByRole('heading', { name: 'Кабинет', exact: true }).isVisible(),
        name + ': original NEwBOO dashboard heading missing');
      assert(await page.getByRole('heading', { name: 'Чтение', exact: true }).isVisible(),
        name + ': reading dashboard section missing');
      assert(await page.getByRole('heading', { name: 'Работа с книгами', exact: true }).isVisible(),
        name + ': author dashboard section missing');
      assert(await page.locator('.dashboard-tile__link[href="/me/books/new"]').isVisible(),
        name + ': create book tile missing');
      assert(await page.getByRole('link', { name: 'Мои книги' }).isVisible(),
        name + ': author link missing');
      assert(!(await page.content()).includes('Анализ ваших переходов'),
        name + ': unimplemented personal analysis advertised');
      assert(!(await page.content()).includes('<h2>Приватность</h2>'),
        name + ': obsolete privacy panel still rendered');
      await inspect(page, name, 'me');
      await page.locator('#main').getByRole('link', { name: 'Моя библиотека' }).click();
      await page.waitForURL('**/me/library');
      assert(await page.getByText('Вы пока не сохраняли место чтения').isVisible(), name + ': empty library state missing');
      await inspect(page, name, 'library');
      // S007: a book can be saved before any progress or purchase is recorded.
      await page.goto(base + '/books/1');
      await page.getByRole('link', { name: 'Добавить в библиотеку' }).click();
      await page.waitForURL('**/me/library/add/1');
      assert(await page.getByRole('button', { name: 'Сохранить книгу' }).isVisible(),
        name + ': bookmark confirmation missing');
      await page.getByRole('button', { name: 'Сохранить книгу' }).click();
      await page.waitForURL('**/me/library');
      assert(await page.getByRole('link', { name: 'Книга для браузерной приёмки' }).isVisible(),
        name + ': personal bookmark missing');
      await inspect(page, name, 'bookmarked');
      await page.getByRole('button', { name: 'Убрать из библиотеки' }).click();
      await page.waitForURL('**/me/library');
      assert(await page.getByText('Пока нет сохранённых книг.').isVisible(),
        name + ': bookmark removal failed');
      // Real browser: server-rendered chapter, CSRF POST, private library resume.
      await page.goto(base + '/read/101');
      assert(await page.getByRole('heading', { name: 'Начало истории' }).isVisible(), name + ': private reader heading missing');
      assert(await page.getByRole('button', { name: 'Сохранить после фрагмента 1' }).isVisible(), name + ': save control missing');
      await inspect(page, name, 'private-reader');
      await page.getByRole('button', { name: 'Сохранить после фрагмента 1' }).click();
      await page.waitForURL('**/read/101#p-1');
      await page.goto(base + '/me/library');
      assert(await page.getByRole('link', { name: 'Продолжить' }).isVisible(), name + ': resume card missing');
      assert(await page.getByText('Фрагмент 1').isVisible(), name + ': saved anchor missing');
      await inspect(page, name, 'library-saved');
      await page.getByRole('link', { name: 'Продолжить' }).click();
      await page.waitForURL('**/read/101#p-1');
      assert(await page.getByRole('link', { name: 'Перейти к сохранённому месту' }).isVisible(), name + ': resume target missing');
      for (const denied of ['/read/102', '/read/103', '/read/104', '/read/105']) {
        const response = await page.goto(base + denied);
        assert(response && response.status() === 404, name + ': private denied ' + denied);
      }
      await page.goto(base + '/me');
      await page.getByRole('button', { name: 'Выйти' }).click();
      await page.waitForURL('**/login');
      await page.getByLabel('Электронная почта').fill('visual.' + name + '@example.test');
      await page.getByLabel('Пароль').fill('TestPassphrase2026');
      assert(await page.locator('.auth-card--newboo .auth-panda__svg').isVisible(),
        name + ': original NEwBOO panda missing on login');
      await inspect(page, name, 'login');
      await page.getByRole('button', { name: 'Войти', exact: true }).click();
      await page.waitForURL('**/me');
      assert(await page.getByText('Читатель ' + name).isVisible(), name + ': login did not restore session');
      // Same address as an existing account: a proper styled 409 page, never raw text.
      await page.goto(base + '/register');
      await page.getByLabel('Отображаемое имя').fill('Читатель ' + name);
      await page.getByLabel('Электронная почта').fill('visual.' + name + '@example.test');
      await page.getByLabel('Пароль (12–128 символов)').fill('TestPassphrase2026');
      const duplicatePost = page.waitForResponse(response =>
        response.url().endsWith('/register') && response.request().method() === 'POST'
      );
      await page.getByRole('button', { name: 'Создать учётную запись' }).click();
      const conflict = await duplicatePost;
      assert(conflict.status() === 409, name + ': duplicate account status changed');
      assert(await page.getByRole('heading', { name: 'Не удалось завершить регистрацию' }).isVisible(),
        name + ': styled registration error heading missing');
      assert(await page.getByRole('link', { name: 'Повторить регистрацию' }).isVisible(),
        name + ': retry registration action absent');
      assert(await page.locator('.account-error__actions').getByRole('link', { name: 'Войти', exact: true }).isVisible(),
        name + ': login action absent');
      await inspect(page, name, 'registration-error');
      await page.close();
    }
    console.log('TOR PG reader+auth browser PASS: 2 viewports, localized validation, full error pages, clean profile and real DB');
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
