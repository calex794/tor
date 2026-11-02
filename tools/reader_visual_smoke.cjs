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
      assert(await page.getByText('Платная · нужен доступ').isVisible(), name + ': paywall label missing');
      assert(!(await page.content()).includes('PRIVATE-PAID-MARKER'), name + ': paid text in TOC');
      await inspect(page, name, 'toc');
      // S005: a visible paid TOC entry must lead to an explanatory page,
      // not an empty 404, an invented checkout, or private chapter text.
      const paidLink = page.getByRole('link', { name: /Глава 3\. Платная глава/ });
      assert(await paidLink.isVisible(), name + ': paid chapter link missing');
      await paidLink.click();
      await page.waitForURL('**/reader/103');
      assert(await page.getByRole('heading', { name: 'Для чтения нужен доступ' }).isVisible(),
        name + ': paywall heading missing');
      assert(await page.locator('.reader-access-page').getByRole('link', { name: 'Войти', exact: true }).isVisible(),
        name + ': paywall sign-in link missing');
      assert(await page.getByRole('link', { name: 'Вернуться к книге' }).isVisible(),
        name + ': paywall return link missing');
      assert(!(await page.content()).includes('PRIVATE-PAID-MARKER'),
        name + ': paid chapter leaked in paywall');
      const paidResponse = await page.goto(base + '/reader/103');
      assert(paidResponse && paidResponse.status() === 200,
        name + ': published paid chapter should show access explanation');
      assert((paidResponse.headers()['cache-control'] || '').includes('no-store'),
        name + ': paywall must not enter shared caches');
      assert((paidResponse.headers()['x-robots-tag'] || '').includes('noindex'),
        name + ': paywall must not be indexed');
      await inspect(page, name, 'paywall');
      await page.goto(base + '/books/1');
      // E-02: Chromium must exercise the actual NEwBOO discussion link and page,
      // not just HTTP tests of the Rust handlers. Use only public fixture data.
      const commentsLink = page.getByRole('link', { name: 'Читать и оставить комментарий' });
      assert(await commentsLink.isVisible(), name + ': book discussion link missing');
      await commentsLink.click();
      await page.waitForURL('**/books/1/comments');
      assert(await page.getByRole('heading', { name: /Комментарии к книге/ }).isVisible(),
        name + ': discussion heading missing');
      assert(await page.getByRole('link', { name: 'Войти, чтобы оставить комментарий' }).isVisible(),
        name + ': anonymous reader must see sign-in invitation');
      assert(await page.locator('form.comments-form').count() === 0,
        name + ': guest must not receive an authenticated comment form');
      assert(await page.getByText('Комментариев пока нет.').isVisible(),
        name + ': empty discussion state missing');
      const commentsHTML = await page.content();
      assert(!commentsHTML.includes('PRIVATE-PAID-MARKER') &&
        !commentsHTML.includes('PRIVATE-DRAFT-MARKER'),
        name + ': private text leaked through discussion page');
      await inspect(page, name, 'comments-guest');
      const hiddenDiscussion = await page.goto(base + '/books/2/comments');
      assert(hiddenDiscussion && hiddenDiscussion.status() === 404,
        name + ': hidden book discussion must be denied');
      assert((hiddenDiscussion.headers()['cache-control'] || '').includes('no-store'),
        name + ': hidden discussion response must not enter shared caches');
      await page.goto(base + '/books/1');
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
      assert(await page.locator('[data-free-settings-open]').isVisible(),
        name + ': free settings missing');
      // H/S047: TOC is loaded from the real bounded, free+paid metadata API.
      assert(await page.locator('[data-free-toc-list] li').count() === 0,
        name + ': free TOC loaded before opening');
      await page.locator('[data-free-toc] > summary').click();
      await page.waitForFunction(() =>
        document.querySelectorAll('[data-free-toc-list] li').length >= 2);
      assert(await page.locator('[data-free-toc-list] [aria-current="page"]').count() === 1,
        name + ': free reader TOC lacks active chapter');
      assert(await page.locator('[data-free-toc-list] a[href="/reader/103"]').isVisible(),
        name + ': published paid chapter metadata missing from free TOC');
      assert(await page.locator('[data-free-toc-list]').getByText('Платная · нужен доступ').isVisible(),
        name + ': free reader TOC missing paid access warning');
      assert(!(await page.locator('[data-free-toc-list]').innerText()).includes('PRIVATE-PAID-MARKER'),
        name + ': paid content leaked into public TOC');
      await page.locator('[data-free-toc] > summary').click();
      assert(await page.locator('[data-free-issue-form]').count() === 0,
        name + ': guest must not receive issue submission form');
      assert(await page.locator('[data-chapter-comments]').count() === 1,
        name + ': guest cannot read public chapter discussion');
      assert(await page.locator('[data-chapter-comment-form]').count() === 0,
        name + ': guest received chapter comment form or CSRF');
      assert(await page.locator('[data-chapter-comments]')
        .getByRole('link', { name: 'Войти, чтобы оставить комментарий' }).isVisible(),
        name + ': guest missing chapter-only login action');
      assert(await page.getByRole('link', { name: 'Войти, чтобы сообщить автору' }).isVisible(),
        name + ': guest lacks account-only report entrypoint');
      assert(await page.locator('[data-free-progress-chapter]').count() === 0,
        name + ': guest received authenticated progress metadata');
      const freeResponse = await page.request.get(base + '/reader/101');
      assert((freeResponse.headers()['content-security-policy'] || '').includes("script-src 'self'"),
        name + ': free reader script blocked');
      assert((freeResponse.headers()['content-security-policy'] || '').includes("connect-src 'self'"),
        name + ': authenticated reader cannot reach its own progress API');
      const lockedResponse = await page.request.get(base + '/reader/103');
      assert((lockedResponse.headers()['content-security-policy'] || '').includes("script-src 'none'"),
        name + ': paid reader CSP relaxed');
      // NEwBOO's original reader keeps a scroll/paginated setting in the
      // same storage key. TOR must not erase an unsupported setting.
      await page.evaluate(() => {
        const saved = JSON.parse(localStorage.getItem('readerSettings') || '{}');
        localStorage.setItem('readerSettings',
          JSON.stringify({ ...saved, renderMode: 'paginated' }));
      });
      await page.reload({ waitUntil: 'networkidle' });
      await page.locator('[data-free-settings-open]').click();
      await page.locator('[data-free-theme="sepia"]').click();
      await page.locator('[data-free-font-family="sans"]').click();
      await page.locator('[data-free-font-scale]').fill('120');
      await page.locator('[data-free-line-height]').fill('200');
      await page.locator('[data-free-reader-width]').fill('980');
      await page.keyboard.press('Escape');
      await page.reload({ waitUntil: 'networkidle' });
      const preferences = await page.evaluate(() => ({
        theme: document.documentElement.dataset.freeReaderTheme,
        width: document.querySelector('[data-free-reader]').style.getPropertyValue('--free-reader-width'),
        font: document.querySelector('[data-free-reader]').style.getPropertyValue('--free-font-family'),
        stored: JSON.parse(localStorage.getItem('readerSettings') || '{}')
      }));
      assert(preferences.theme === 'sepia' && preferences.width === '980px' &&
        preferences.font.includes('Arial') && preferences.stored.fontScale === 1.2 &&
        preferences.stored.renderMode === 'paginated',
        name + ': saved preferences failed ' + JSON.stringify(preferences));
      await inspect(page, name + '-preferences', 'reader');
      await page.locator('[data-free-settings-open]').click();
      await page.locator('[data-free-settings-reset]').click();
      await page.keyboard.press('Escape');
      await page.reload({ waitUntil: 'networkidle' });
      assert(await page.locator('[data-free-font-output]').textContent() === '100%',
        name + ': reset not persisted');
      const afterReset = await page.evaluate(() =>
        JSON.parse(localStorage.getItem('readerSettings') || '{}'));
      assert(afterReset.renderMode === 'paginated',
        name + ': reset erased NEwBOO render mode');
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
      for (const denied of ['/reader/102', '/reader/104', '/books/2', '/api/v1/books/2/toc']) {
        const response = await page.goto(base + denied);
        assert(response && response.status() === 404, name + ': denied URL ' + denied);
        assert((response.headers()['cache-control'] || '').includes('no-store'),
          name + ': shared-cacheable denied URL ' + denied);
        if (denied === '/books/2') {
          const cards = page.locator('.book-error-promos__card');
          assert(await cards.count() >= 1 && await cards.count() <= 3,
            name + ': missing published-book recovery links');
          assert(await page.getByRole('heading', { name: 'Другие книги на сайте' }).isVisible(),
            name + ': book recovery heading missing');
          assert(!(await page.content()).includes('PRIVATE-PAID-MARKER'),
            name + ': private chapter body leaked into 404');
          assert((await page.locator('meta[name="robots"]').getAttribute('content')).includes('noindex'),
            name + ': 404 indexed');
        }
      }
      await page.close();
    }
    // H-web-07: real source dark tokens and original dark logo, no JavaScript.
    // The old NEwBOO manual theme control is intentionally not advertised
    // until the strict script-src 'none' policy has a reviewed server alternative.
    for (const [name, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const page = await browser.newPage({
        viewport: { width, height }, deviceScaleFactor: 1, colorScheme: 'dark'
      });
      for (const [location, url] of [
        ['home', '/'],
        ['catalog', '/catalog'],
        ['book', '/books/1'],
        ['comments', '/books/1/comments'],
        ['reader', '/reader/101'],
        ['register', '/register']
      ]) {
        const response = await page.goto(base + url, { waitUntil: 'networkidle' });
        assert(response && response.status() === 200,
          name + ': dark theme ' + location + ' failed HTTP');
        const css = await page.evaluate(() => {
          const root = getComputedStyle(document.documentElement);
          const logo = getComputedStyle(document.querySelector('.site-logo'));
          const body = getComputedStyle(document.body);
          return {
            scheme: root.colorScheme,
            background: body.backgroundColor,
            token: root.getPropertyValue('--bg-main').trim(),
            text: root.getPropertyValue('--text-main').trim(),
            logo: logo.backgroundImage
          };
        });
        assert(css.scheme.includes('dark'), name + ': CSS did not advertise dark controls');
        assert(css.token === '#0b1120' && css.text === '#f8fafc',
          name + ': original NEwBOO dark tokens missing: ' + JSON.stringify(css));
        assert(css.background === 'rgb(11, 17, 32)',
          name + ': still uses light page background');
        assert(css.logo.includes('pandaolate-header-dark.png'),
          name + ': original dark PNG not selected');
        await inspect(page, name + '-dark', location);
        if (location === 'catalog' && width === 390) {
          await page.locator('.site-mobile-menu__trigger').click();
          assert(await page.getByRole('navigation', { name: 'Мобильная навигация' }).isVisible(),
            name + ': dark mobile menu not usable');
          await page.locator('.site-mobile-menu__trigger').click();
        }
      }
      await page.emulateMedia({ colorScheme: 'light' });
      await page.goto(base + '/catalog', { waitUntil: 'networkidle' });
      const restored = await page.evaluate(() => {
        const root = getComputedStyle(document.documentElement);
        return {
          scheme: root.colorScheme,
          token: root.getPropertyValue('--bg-main').trim(),
          logo: getComputedStyle(document.querySelector('.site-logo')).backgroundImage,
          background: getComputedStyle(document.body).backgroundColor
        };
      });
      assert(restored.scheme.includes('light') && restored.token === '#f7f7f8',
        name + ': light theme should be preserved on preference change');
      assert(restored.background === 'rgb(247, 247, 248)' &&
        restored.logo.includes('pandaolate-header.png') &&
        !restored.logo.includes('-dark.png'),
        name + ': original light logo and surface not restored');
      await inspect(page, name + '-light', 'catalog');
      await page.close();
    }
    // Exercise real PostgreSQL account forms on both target viewport sizes.
    for (const [name, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const context = await browser.newContext({
        viewport: { width, height }, deviceScaleFactor: 1
      });
      const page = await context.newPage();
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
      // H-05/S006: an authenticated reader uses the existing PostgreSQL CAS
      // cursor through the public, themed chapter view. A guest has no token.
      await page.goto(base + '/reader/101', { waitUntil: 'networkidle' });
      const progressArticle = page.locator('[data-free-progress-chapter="101"]');
      assert(await progressArticle.count() === 1,
        name + ': authenticated free reader has no progress metadata');
      assert((await progressArticle.getAttribute('data-free-progress-csrf') || '').length === 43,
        name + ': missing server-origin CSRF token');
      await page.waitForFunction(() =>
        document.querySelector('[data-free-progress-chapter]')?.dataset.freeProgressReady === '1');
      // Merely opening a chapter and leaving must NOT create ChapterStarted.
      await page.goto(base + '/books/1', { waitUntil: 'networkidle' });
      const unopened = await page.request.get(base +
        '/api/v1/me/progress/free?chapter_id=101');
      const untouched = await unopened.json();
      assert(unopened.status() === 200 && untouched.revision === 0 &&
        untouched.block_id === null,
        name + ': GET/pagehide created progress without reading');
      await page.goto(base + '/reader/101', { waitUntil: 'networkidle' });
      await page.waitForFunction(() =>
        document.querySelector('[data-free-progress-chapter]')?.dataset.freeProgressReady === '1');
      const progressPost = page.waitForResponse(response =>
        response.url().endsWith('/api/v1/me/progress/free') &&
        response.request().method() === 'PUT', { timeout: 12000 });
      await page.evaluate(() =>
        window.scrollTo(0, document.documentElement.scrollHeight));
      const firstSaved = await progressPost;
      assert(firstSaved.status() === 200,
        name + ': free chapter auto-save failed HTTP ' + firstSaved.status());
      const sent = firstSaved.request().postDataJSON();
      const sourceBlocks = (await progressArticle.getAttribute('data-free-progress-blocks'))
        .split(',').map(part => Number(part.split(':')[1]));
      assert(sent.block_id >= 1 && sent.block_id <= sourceBlocks.length &&
        sent.char_offset < sourceBlocks[sent.block_id - 1],
        name + ': scroll must never silently mark chapter complete');
      const storedState = await page.request.get(base +
        '/api/v1/me/progress/free?chapter_id=101');
      assert(storedState.status() === 200,
        name + ': cannot read actor-scoped saved progress');
      const cursor = await storedState.json();
      assert(cursor.block_id === sent.block_id &&
        cursor.char_offset === sent.char_offset && cursor.revision > 0,
        name + ': PostgreSQL cursor does not match auto-save');
      await page.reload({ waitUntil: 'networkidle' });
      await page.waitForFunction(() => {
        const article = document.querySelector('[data-free-progress-chapter]');
        return article && (article.dataset.freeProgressRestored === '1' ||
          window.scrollY > 0);
      });
      const secondTab = await page.context().newPage();
      await secondTab.goto(base + '/reader/101', { waitUntil: 'networkidle' });
      await secondTab.waitForFunction(() =>
        document.querySelector('[data-free-progress-chapter]')?.dataset.freeProgressRestored === '1');
      assert(await secondTab.evaluate(() => window.scrollY > 0),
        name + ': second browser tab did not restore PostgreSQL cursor');
      await secondTab.close();
      await inspect(page, name, 'free-resume');
      // S047: a selected excerpt on the public themed page must bind to the
      // exact current ChapterVersion and canonical UTF-16 block, not free text.
      const selectIssueQuote = async () => page.evaluate(() => {
        const span = [...document.querySelectorAll(
          '[data-free-progress-chapter] span[data-md-start]'
        )].find(node => node.firstChild?.nodeType === Node.TEXT_NODE &&
          node.firstChild.textContent.includes('Первая строка'));
        if (!span) throw new Error('S047 public selected source span unavailable');
        const node = span.firstChild;
        const offset = node.textContent.indexOf('Первая');
        const range = document.createRange();
        range.setStart(node, offset);
        range.setEnd(node, offset + 6);
        const selection = window.getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
        document.dispatchEvent(new Event('selectionchange'));
        return selection.toString();
      });
      const selectedQuote = await selectIssueQuote();
      await page.locator('[data-free-issues] > summary').click();
      assert(await page.locator('[data-free-issue-quote]').inputValue() === selectedQuote,
        name + ': S047 selected quotation does not match browser text');
      assert(await page.locator('[data-free-issue-block]').inputValue() === '1',
        name + ': S047 incorrect canonical source block');
      const publicIssuePost = page.waitForResponse(response =>
        response.url().endsWith('/read/101/text-issues') &&
        response.request().method() === 'POST');
      await page.locator('[data-free-issue-suggestion]').fill('Верная строка');
      await page.locator('[data-free-issue-submit]').click();
      assert((await publicIssuePost).status() === 303,
        name + ': public reader report rejected by trusted E-12 handler');
      await page.waitForURL(/\/reader\/101\?issue=sent#text-issues$/);
      assert(await page.locator('[data-free-issues] .reader-issues__feedback')
        .getByText('Сообщение об ошибке отправлено автору.').isVisible(),
        name + ': report confirmation failed to return to modern public reader');
      // A replay from the same author+quote+suggestion must not create a second issue.
      await selectIssueQuote();
      await page.locator('[data-free-issue-suggestion]').fill('Верная строка');
      await page.locator('[data-free-issue-submit]').click();
      await page.waitForURL(/\/reader\/101\?issue=repeat#text-issues$/);
      assert(await page.locator('[data-free-issues] .reader-issues__feedback')
        .getByText('Такое сообщение уже было отправлено автору.').isVisible(),
        name + ': repeated report was not deduplicated');
      // E/S011: no book-level comments are recycled as chapter comments.
      // The selected quotation must have the immutable edition's UTF-16
      // offset; submission and replies use real PostgreSQL + the same CSRF.
      const chapterComments = page.locator('[data-chapter-comments]');
      assert(await chapterComments.count() === 1 &&
        await chapterComments.locator('[data-chapter-comment-form]').count() >= 1,
        name + ': signed free reader missing chapter discussion form');
      assert((await chapterComments.getAttribute('data-reader-comment-blocks') || '')
        .startsWith('0:'), name + ': missing canonical source offset map');
      await selectIssueQuote();
      assert(await chapterComments.locator('[data-chapter-comment-block]').inputValue() === '1' &&
        await chapterComments.locator('[data-chapter-comment-block]').getAttribute('name') === 'anchor_block',
        name + ': selected quote did not bind to E-01 canonical chapter anchor');
      const rootCommentBody = 'Сценарий ' + name + ': текст <script> ||секрет автора||';
      await chapterComments.locator('textarea[name="body"]').first().fill(rootCommentBody);
      const commentPost = page.waitForResponse(response =>
        response.url().endsWith('/reader/101/comments') &&
        response.request().method() === 'POST');
      await chapterComments.getByRole('button', { name: 'Отправить комментарий' }).click();
      const commentReply = await commentPost;
      assert(commentReply.status() === 303,
        name + ': real S011 chapter POST failed ' + commentReply.status());
      const postedForm = commentReply.request().postData();
      assert(postedForm.includes('anchor_block=1') && postedForm.includes('anchor_quote='),
        name + ': source-bound chapter comment was sent without its anchor');
      await page.waitForURL(/\/reader\/101\?comment=sent#chapter-comments$/);
      assert(await chapterComments.getByText('Комментарий опубликован.').isVisible(),
        name + ': S011 chapter submission did not return to public reader');
      assert(await chapterComments.locator('.comment-card').filter({ hasText: 'Сценарий ' + name }).count() >= 1,
        name + ': new chapter comment missing after redirect');
      assert((await chapterComments.locator('.comment-card').first().innerHTML()).includes('&lt;script&gt;'),
        name + ': comment body was not escaped');
      assert(await chapterComments.locator('.comment-spoiler').count() >= 1,
        name + ': chapter comment spoilers not masked');
      assert(await chapterComments.locator('[data-chapter-comment-goto]').count() >= 1,
        name + ': text-bound comment lacks source navigation');
      await chapterComments.locator('[data-chapter-comment-goto]').first().click();
      assert(await page.locator('span.reader-chapter-comments__source-highlight').count() > 0,
        name + ': click to UTF-16 source fragment failed');
      // Same command key+payload must not produce another row. Different
      // version, quote and paid target must be denied without leaking text.
      const sameOriginHeaders = {
        Origin: base, 'Content-Type': 'application/x-www-form-urlencoded'
      };
      const replay = await page.request.post(base + '/reader/101/comments', {
        data: postedForm, headers: sameOriginHeaders, maxRedirects: 0
      });
      assert(replay.status() === 303 &&
        (replay.headers().location || '').includes('comment=repeat'),
        name + ': chapter comment replay was not idempotent');
      const wrongQuote = new URLSearchParams(postedForm);
      wrongQuote.set('command_key', 'z'.repeat(43));
      wrongQuote.set('anchor_quote', 'несуществующая цитата');
      const rejectedQuote = await page.request.post(base + '/reader/101/comments', {
        data: wrongQuote.toString(), headers: sameOriginHeaders, maxRedirects: 0
      });
      assert(rejectedQuote.status() === 422,
        name + ': fabricated quote reached canonical chapter storage');
      const wrongVersion = new URLSearchParams(postedForm);
      wrongVersion.set('version_id', '99999999');
      const rejectedVersion = await page.request.post(base + '/reader/101/comments', {
        data: wrongVersion.toString(), headers: sameOriginHeaders, maxRedirects: 0
      });
      assert(rejectedVersion.status() === 404,
        name + ': wrong chapter edition accepted for discussion');
      const rejectedPaid = await page.request.post(base + '/reader/103/comments', {
        data: postedForm, headers: sameOriginHeaders, maxRedirects: 0
      });
      assert(rejectedPaid.status() === 404,
        name + ': paid chapter comment accepted from public free endpoint');
      const rejectedOrigin = await page.request.post(base + '/reader/101/comments', {
        data: postedForm,
        headers: { Origin: 'https://evil.invalid',
          'Content-Type': 'application/x-www-form-urlencoded' },
        maxRedirects: 0
      });
      assert(rejectedOrigin.status() === 403,
        name + ': cross-origin chapter comment forgery not blocked');
      const replyDetails = chapterComments.locator('.comment-reply').first();
      await replyDetails.locator('summary').click();
      await replyDetails.locator('textarea[name="body"]').fill('Ответ ' + name);
      const replyPost = page.waitForResponse(response =>
        response.url().endsWith('/reader/101/comments') &&
        response.request().method() === 'POST');
      await replyDetails.getByRole('button', { name: 'Отправить ответ' }).click();
      assert((await replyPost).status() === 303,
        name + ': same-edition comment reply rejected');
      await page.waitForURL(/\/reader\/101\?comment=sent#chapter-comments$/);
      assert(await chapterComments.getByText('Ответ ' + name, { exact: true }).count() === 1,
        name + ': chapter reply not visible after posting');
      await inspect(page, name, 'chapter-discussion');
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

      // F-05: real browser, no script required in the reader itself.
      // Saving an ordinary paragraph must not mint a completion or a ranking.
      await page.goto(base + '/me/rankings');
      assert(await page.getByText('Пока нет книг для сравнения').isVisible(),
        name + ': saved position incorrectly counted as completed chapter');
      await page.goto(base + '/read/101');
      const completeButton = page.getByRole('button', { name: 'Глава прочитана' });
      assert(await completeButton.isVisible(), name + ': explicit finish button missing');
      assert(await page.locator('form[action="/read/101/complete"]').count() === 1,
        name + ': exactly one completion form should exist');
      await inspect(page, name, 'complete-action');
      const completionPost = page.waitForResponse(response =>
        response.url().endsWith('/read/101/complete') && response.request().method() === 'POST'
      );
      await completeButton.click();
      assert((await completionPost).status() === 303, name + ': authenticated finish must redirect');
      await page.waitForURL('**/read/101#continuation');
      assert(await page.getByText('Глава уже отмечена прочитанной').isVisible(),
        name + ': completed reading must return to the same chapter');
      // Rankings are still available, but completion must not force the
      // reader away from the newly available continuation action.
      const rankLink = page.getByRole('link', { name: 'Мои рейтинги' });
      assert(await rankLink.isVisible(), name + ': explicit ranking link missing');
      await rankLink.click();
      await page.waitForURL('**/me/rankings');
      assert(await page.getByRole('heading', { name: 'Мои книги' }).isVisible(),
        name + ': trusted completion was not immediately added to personal ranking');
      assert(await page.locator('.ranking-books .ranking-book').count() === 1,
        name + ': exactly one verified book should be ranked');
      assert(await page.locator('.ranking-book__place').innerText() === 'Место 1',
        name + ': verified first book must occupy first rank');
      await inspect(page, name, 'completed-ranking');
      await page.goto(base + '/read/101');
      assert(await page.getByText('Глава уже отмечена прочитанной').isVisible(),
        name + ': durable completed state missing after reload');
      assert(await page.getByRole('button', { name: 'Глава прочитана' }).count() === 0,
        name + ': repeat completion action must be hidden');

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
      await page.emulateMedia({ colorScheme: 'dark' });
      const errorStyle = await page.locator('.account-error__card')
        .evaluate(node => ({
          background: getComputedStyle(node).backgroundColor,
          text: getComputedStyle(node).color
        }));
      assert(errorStyle.background === 'rgb(17, 24, 39)',
        name + ': registration error remains white in dark mode');
      assert(errorStyle.text === 'rgb(248, 250, 252)',
        name + ': registration error has unreadable foreground in dark mode');
      await inspect(page, name + '-dark', 'registration-error');
      await context.close();
    }
    await require('./comment_spoilers_browser.cjs')(browser, base, out);
    await require('./mail_browser_pg.cjs')(browser, base, out);
    await require('./support_browser_pg.cjs')(browser, base, out);
    console.log('TOR PG reader+auth browser PASS: 2 viewports, localized validation, full error pages, clean profile and real DB');
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
