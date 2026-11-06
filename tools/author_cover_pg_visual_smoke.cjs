// H-web-11: actual Chromium owner journey. No HTML fixture, bypassCSP or fake S3.
// Runs only with the disposable PostgreSQL and private loopback MinIO in CI.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { approvePendingBookForIsolatedCi } = require('./site_moderation_ci.cjs');

const base = 'http://127.0.0.1:3098';
const output = process.env.TOR_COVER_BROWSER_SHOTS || 'cover-browser-shots';
fs.mkdirSync(output, { recursive: true });

function assert(ok, message) {
  if (!ok) throw new Error(message);
}

async function shot(page, viewport, scheme, state) {
  const frame = page.locator('.book-cover-picker__frame');
  const box = await frame.boundingBox();
  assert(box && Math.abs(box.width / box.height - 2 / 3) < 0.035,
    viewport + '/' + scheme + '/' + state + ': NEwBOO 2:3 cover frame missing');
  const metrics = await page.evaluate(() => ({
    viewport: document.documentElement.clientWidth,
    html: document.documentElement.scrollWidth,
    body: document.body.scrollWidth,
    border: getComputedStyle(document.querySelector('.book-cover-picker__frame')).borderTopStyle,
  }));
  assert(metrics.html <= metrics.viewport + 1 && metrics.body <= metrics.viewport + 1,
    viewport + '/' + scheme + ': horizontal overflow ' + JSON.stringify(metrics));
  assert(metrics.border === 'dashed', 'Original NEwBOO dashed cover frame missing');
  assert(await page.locator('.book-cover-picker').count() === 1,
    'TOR may show only its one functional slot, never three fake cover slots');
  assert(await page.getByRole('heading', { name: 'Обложки книги' }).isVisible(),
    'Original NEwBOO covers heading missing');
  await page.screenshot({
    path: path.join(output, ['cover-owner', viewport, scheme, state].join('-') + '.png'),
    fullPage: true,
  });
}

async function actualPng(page) {
  // Create a decodable 80x120 PNG, not a 1x1 browser-only placeholder.
  const data = await page.evaluate(() => {
    const canvas = document.createElement('canvas');
    canvas.width = 80;
    canvas.height = 120;
    const ctx = canvas.getContext('2d');
    ctx.fillStyle = '#3267a4';
    ctx.fillRect(0, 0, 80, 120);
    ctx.fillStyle = '#ffffff';
    ctx.fillRect(12, 20, 56, 14);
    return canvas.toDataURL('image/png');
  });
  assert(data.startsWith('data:image/png;base64,'), 'Browser did not create a valid PNG');
  return Buffer.from(data.split(',')[1], 'base64');
}


const ciDb = process.env.APP_URL;
if (process.env.GITHUB_ACTIONS !== 'true' || !ciDb) {
  throw new Error('Moderator cover browser needs disposable GitHub CI');
}
const dbUrl = new URL(ciDb);
if (!['postgres:', 'postgresql:'].includes(dbUrl.protocol) ||
    dbUrl.hostname !== '127.0.0.1' || dbUrl.port !== '5432' ||
    dbUrl.pathname !== '/tor_ci' || dbUrl.username !== 'tor_app' ||
    dbUrl.search || dbUrl.hash) {
  throw new Error('Refusing non-disposable cover moderation SQL fixture');
}
function pg(sql) {
  return execFileSync('psql', [
    '-X', '-A', '-t', '-q', '-v', 'ON_ERROR_STOP=1',
    '--dbname', ciDb, '--command', sql,
  ], { encoding: 'utf8', timeout: 15000, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}
async function verifyModeratorWorkbench(reviewer, owner, viewport, scheme, width, height) {
  await reviewer.setViewportSize({ width, height });
  const tabs = [
    ['/moderation/books/registrations', /^Регистрация \(\d+\)$/],
    ['/moderation/covers', /^Обложки \(\d+\)$/],
    ['/moderation/books/deletions', /^Удаление \(\d+\)$/],
  ];
  for (const [url, label] of tabs) {
    const denied = await owner.context().request.get(base + url);
    assert(denied.status() === 403,
      viewport + '/' + scheme + ': ordinary Book owner opened a site queue');
    const response = await reviewer.goto(base + url);
    assert(response && response.status() === 200,
      viewport + '/' + scheme + ': independent site reviewer cannot open ' + url);
    const nav = reviewer.getByRole('navigation', { name: 'Очереди модератора' });
    assert(await nav.count() === 1, 'moderator workbench has no common three-tab nav');
    assert(await nav.getByRole('link', { name: label }).count() === 1,
      viewport + '/' + scheme + ': expected queue/counter missing on ' + url);
    assert(await nav.locator('a[aria-current="page"]').count() === 1,
      'site moderator must know which queue is selected');
    const metrics = await reviewer.evaluate(() => ({
      width: document.documentElement.clientWidth,
      doc: document.documentElement.scrollWidth,
      body: document.body.scrollWidth,
    }));
    assert(metrics.doc <= metrics.width + 1 && metrics.body <= metrics.width + 1,
      viewport + '/' + scheme + ': moderator queue overflows: ' + JSON.stringify(metrics));
    await reviewer.screenshot({
      path: path.join(output, ['moderator-workbench', viewport, scheme,
        url.split('/').pop()].join('-') + '.png'),
      fullPage: false,
    });

    // Native link keyboard access is an acceptance condition, not simply
    // presence of an aria-current attribute in the HTML. Traverse from the
    // start of the document so hidden/mobile nav elements cannot mask failure.
    const target = nav.getByRole('link', { name: label });
    let tabReached = false;
    for (let press = 0; press < 80; press++) {
      await reviewer.keyboard.press('Tab');
      if (await target.evaluate(link => link === document.activeElement)) {
        tabReached = true;
        break;
      }
    }
    assert(tabReached,
      viewport + '/' + scheme + ': site queue tab unreachable with Tab keyboard');
    // The HTTP response can arrive before the replacement document has loaded.
    // Synchronize on the real navigation, otherwise aria-current is a race.
    const [viaKeyboard] = await Promise.all([
      reviewer.waitForResponse(r =>
        r.url() === base + url && r.request().method() === 'GET'),
      reviewer.waitForEvent('load'),
      reviewer.keyboard.press('Enter'),
    ]);
    assert(viaKeyboard.status() === 200,
      viewport + '/' + scheme + ': Enter cannot activate the moderator queue link');
    const current = reviewer.getByRole('navigation', { name: 'Очереди модератора' })
      .locator('a[aria-current="page"]');
    await current.waitFor({ state: 'visible' });
    assert(await current.count() === 1 && await current.getAttribute('href') === url,
      'keyboard-activated queue lost current tab marker');
  }
}

async function approveReadyCover(reviewer, owner, bookId, viewport, scheme) {
  const prefix = '/covers/' + bookId + '/main.webp';
  assert((await owner.context().request.get(base + prefix)).status() === 404,
    'unapproved processed cover leaked through public URL');
  let form;
  for (let attempt = 0; attempt < 50; attempt++) {
    const queue = await reviewer.goto(base + '/moderation/covers');
    assert(queue && queue.status() === 200,
      viewport + '/' + scheme + ': independent site reviewer cannot view cover queue');
    const candidates = reviewer.locator('article.me-now-card').filter({
      hasText: 'Книга #' + bookId + ' ·',
    });
    if (await candidates.count() === 1) {
      form = candidates.locator('form[action^="/moderation/covers/"][action$="/decision"]');
      assert(await form.count() === 1, 'reviewer queue lacks exact candidate form');
      break;
    }
    await reviewer.waitForTimeout(1000);
  }
  assert(form, viewport + '/' + scheme + ': cover worker never created review request');
  const preview = await reviewer.context().request.get(
    base + (await reviewer.locator('article.me-now-card').filter({
      hasText: 'Книга #' + bookId + ' ·',
    }).locator('img[src^="/moderation/covers/"]').getAttribute('src'))
  );
  assert(preview.status() === 200, 'site reviewer could not preview protected WebP');
  const previewBytes = await preview.body();
  assert(previewBytes.subarray(0, 4).toString() === 'RIFF' &&
    previewBytes.subarray(8, 12).toString() === 'WEBP',
    'moderator preview was not a decoded WebP');
  // First POST is a private no-write confirmation page. The staged cover
  // must still be inaccessible through the public URL after this click.
  const [review] = await Promise.all([
    reviewer.waitForResponse(r =>
      r.request().method() === 'POST' && /\/moderation\/covers\/\d+\/decision$/.test(r.url())),
    form.locator('button[name=action][value=approve]').click(),
  ]);
  assert(review.status() === 200,
    viewport + '/' + scheme + ': cover review did not open confirmation');
  assert(await reviewer.getByRole('heading', {
    name: /Подтверждение решения по обложке/,
  }).count() === 1, 'missing explicit second-step cover confirmation');
  assert((await owner.context().request.get(base + prefix)).status() === 404,
    'review preview published a staged cover before confirmation');
  const [decided] = await Promise.all([
    reviewer.waitForResponse(r =>
      r.request().method() === 'POST' && /\/moderation\/covers\/\d+\/decision$/.test(r.url())),
    reviewer.getByRole('button', { name: 'Одобрить обложку' }).focus()
      .then(() => reviewer.keyboard.press('Enter')),
  ]);
  assert(decided.status() === 303,
    viewport + '/' + scheme + ': site moderator cover approval refused');
  // The 303 response precedes the browser's redirect navigation. Wait for
  // that navigation to finish before opening the private decision history.
  await reviewer.waitForURL(base + '/moderation/covers', { waitUntil: 'load' });
  assert((await owner.context().request.get(base + prefix)).status() === 200,
    'approved cover was not published through its B-owned slot');

  // MOD-UI-01: historical decisions and reviewer identities are site-private.
  const bookHistory = '/moderation/books/' + bookId + '/history';
  assert((await owner.context().request.get(base + bookHistory)).status() === 403,
    'Book owner obtained private moderator decision history');
  assert((await owner.context().request.get(
    base + '/moderation/books/reviews')).status() === 403,
    'Book owner obtained the reviewed-decision index');
  const historyResponse = await reviewer.goto(base + bookHistory);
  assert(historyResponse && historyResponse.status() === 200,
    'Site moderator cannot read the approved cover history');
  const historyHtml = await reviewer.content();
  assert(historyHtml.includes('Системная история книги #' + bookId) &&
    historyHtml.includes('Обложка: Одобрено') &&
    historyHtml.includes('Независимый модератор обложек'),
    'Site moderator history lost reviewer attribution');
  const reviewed = await reviewer.goto(
    base + '/moderation/books/reviews?kind=cover&outcome=approved');
  assert(reviewed && reviewed.status() === 200,
    'Site moderator cannot filter approved cover decisions');
  assert(await reviewer.locator('a[href="' + bookHistory + '"]').count() >= 1,
    'Reviewed decisions index lost the approved Book');
  assert((await reviewer.context().request.get(
    base + '/moderation/books/reviews?kind=chapter')).status() === 400,
    'Unexpected moderation decision type was accepted');
  const publicBook = await owner.context().request.get(base + '/books/' + bookId);
  assert(publicBook.status() === 200 &&
    !(await publicBook.text()).includes('Независимый модератор обложек'),
    'Public Book page exposed the private reviewer identity');
}

async function readyCover(page, bookId, viewport, scheme) {
  for (let i = 0; i < 50; i++) {
    const response = await page.goto(base + '/me/books/' + bookId + '/settings/covers');
    assert(response && response.status() === 200,
      viewport + '/' + scheme + ': owner settings lost authorization while processing');
    if (await page.getByText('Обложка одобрена модератором.').isVisible()) {
      assert(await page.locator('img[data-cover-picker-image]').getAttribute('src')
        === '/covers/' + bookId + '/main.webp',
      'Ready cover must use the B-owned published WebP');
      return;
    }
    assert(!(await page.getByText(/Ошибка обработки обложки|Модератор отклонил обложку/).isVisible()),
      viewport + '/' + scheme + ': worker rejected image');
    await page.waitForTimeout(1000);
  }
  throw new Error(viewport + '/' + scheme + ': S3/PG worker never marked cover done');
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const moderatorContext = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    const moderator = await moderatorContext.newPage();
    const moderatorMail = 'cover.site.reviewer.' + process.env.GITHUB_RUN_ID + '@example.invalid';
    await moderator.goto(base + '/register');
    await moderator.getByLabel('Отображаемое имя').fill('Независимый модератор обложек');
    await moderator.getByLabel('Электронная почта').fill(moderatorMail);
    await moderator.getByLabel('Пароль (12–128 символов)').fill('CoverBrowserStrongPassword2026');
    await moderator.getByRole('button', { name: 'Создать учётную запись' }).click();
    await moderator.waitForURL('**/me');
    const reviewerId = pg("SELECT id FROM users WHERE email_normalized='" + moderatorMail + "'");
    assert(/^[1-9]\d*$/.test(reviewerId), 'moderator fixture must be newly registered');
    pg('INSERT INTO account_roles(user_id,role,is_active) VALUES(' + reviewerId + ", 'moderator', TRUE)");
    for (const [viewport, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      for (const scheme of ['light', 'dark']) {
        const context = await browser.newContext({
          viewport: { width, height }, colorScheme: scheme,
        });
        const page = await context.newPage();
        const title = 'Обложка из браузера ' + viewport + ' ' + scheme;
        await page.goto(base + '/register');
        await page.getByLabel('Отображаемое имя').fill('Автор обложки ' + viewport + scheme);
        await page.getByLabel('Электронная почта')
          .fill('cover.browser.' + viewport + '.' + scheme + '@example.test');
        await page.getByLabel('Пароль (12–128 символов)').fill('CoverBrowserStrongPassword2026');
        await page.getByRole('button', { name: 'Создать учётную запись' }).click();
        await page.waitForURL('**/me');

        await page.getByRole('link', { name: 'Мои книги' }).click();
        // S019: exercise the header action at both viewport sizes.
        if (viewport === 'desktop') {
          const create = page.getByRole('navigation', { name: 'Основная навигация' })
            .getByRole('link', { name: 'Создать книгу' });
          assert(await create.isVisible(), 'Desktop header must offer book registration');
          await create.click();
        } else {
          await page.locator('.site-mobile-menu__trigger').click();
          const create = page.getByRole('navigation', { name: 'Мобильная навигация' })
            .getByRole('link', { name: 'Создать книгу' });
          assert(await create.isVisible(), 'Mobile menu must offer book registration');
          await create.click();
        }
        await page.getByLabel('Название книги').fill(title);
        await page.getByLabel('Язык', { exact: true }).fill('ru');
        await page.getByRole('button', { name: 'Создать книгу' }).click();
        await page.waitForURL(/\/me\/books\/\d+$/);
        const bookId = page.url().split('/').pop();

        // A second form with the same title must show a useful 409, not a blank error.
        await page.goto(base + '/me/books/new');
        await page.getByLabel('Название книги').fill(title);
        await page.getByLabel('Язык', { exact: true }).fill('ru');
        const [duplicateResponse] = await Promise.all([
          page.waitForResponse(r => r.url() === base + '/me/books' &&
            r.request().method() === 'POST'),
          page.getByRole('button', { name: 'Создать книгу' }).click(),
        ]);
        assert(duplicateResponse.status() === 409,
          'S019: duplicate title must return 409');
        assert(await page.getByRole('heading', { name: 'Книга не создана' }).isVisible(),
          'S019: duplicate title must explain the failure in Russian');
        assert(await page.getByRole('link', { name: 'Проверить каталог' }).isVisible(),
          'S019: duplicate response must offer a useful next action');
        // S020: edited Chinese and English aliases are claimed and protected.
        await page.goto(base + '/me/books/' + bookId + '/settings/titles');
        const chineseAlias = '修仙之路 ' + viewport + ' ' + scheme;
        const englishAlias = 'Immortal Path ' + viewport + ' ' + scheme;
        await page.getByLabel('Оригинальное название').fill(chineseAlias);
        await page.getByLabel('Альтернативное название 1 (например, английское)')
          .fill(englishAlias);
        await page.getByRole('button', { name: 'Сохранить названия' }).click();
        await page.waitForURL(base + '/me/books/' + bookId + '/settings');
        await page.goto(base + '/me/books/' + bookId + '/settings/titles');
        assert(await page.getByLabel('Оригинальное название').inputValue() === chineseAlias,
          'S020: edited Chinese original title must persist');
        assert(await page.getByLabel('Альтернативное название 1 (например, английское)')
          .inputValue() === englishAlias, 'S020: edited English alias must persist');
        await page.goto(base + '/me/books/new');
        await page.getByLabel('Название книги').fill('Другая книга ' + viewport + scheme);
        await page.getByLabel('Оригинальное название').fill(chineseAlias);
        const [aliasConflict] = await Promise.all([
          page.waitForResponse(r => r.url() === base + '/me/books' &&
            r.request().method() === 'POST'),
          page.getByRole('button', { name: 'Создать книгу' }).click(),
        ]);
        assert(aliasConflict.status() === 409,
          'S020: edited Chinese title blocks new duplicate registration');
        // S020: a member-scoped origin editor persists data without changing publication.
        await page.goto(base + '/me/books/' + bookId + '/settings/origin');
        await page.getByLabel('Язык оригинала (например, zh, en)').fill('zh');
        await page.getByLabel('Год выпуска').fill('2024');
        await page.getByLabel('Раздел каталога').selectOption('novels/chinese');
        await page.getByRole('button', { name: 'Сохранить сведения' }).click();
        await page.waitForURL(base + '/me/books/' + bookId + '/settings');
        await page.goto(base + '/me/books/' + bookId + '/settings/origin');
        assert(await page.getByLabel('Язык оригинала (например, zh, en)').inputValue() === 'zh',
          'S020: original language persists');
        assert(await page.getByLabel('Год выпуска').inputValue() === '2024',
          'S020: release year persists');
        assert(await page.getByLabel('Раздел каталога').inputValue() === 'novels/chinese',
          'S020: catalog section persists');
        // S020: a real member-scoped settings index, annotation and status roundtrip.
        await page.goto(base + '/me/books/' + bookId + '/settings');
        assert(await page.getByRole('link', { name: 'Аннотация книги' }).count() === 1,
          'S020: settings index lists the annotation editor');
        await page.getByRole('link', { name: 'Аннотация книги' }).click();
        const annotation = 'Редакторская аннотация ' + viewport + ' ' + scheme;
        await page.getByLabel('Аннотация (до 4000 символов)').fill(annotation);
        await page.getByRole('button', { name: 'Сохранить аннотацию' }).click();
        await page.waitForURL(base + '/me/books/' + bookId + '/settings');
        await page.getByRole('link', { name: 'Аннотация книги' }).click();
        assert(await page.getByLabel('Аннотация (до 4000 символов)').inputValue() === annotation,
          'S020: annotation persists and scoped redirect works');
        await page.goto(base + '/me/books/' + bookId + '/settings');
        await page.getByRole('link', { name: 'Состояние работы' }).click();
        await page.getByLabel('Состояние работы').selectOption('paused');
        await page.getByRole('button', { name: 'Сохранить', exact: true }).click();
        await page.waitForURL(base + '/me/books/' + bookId + '/settings');
        await page.getByRole('link', { name: 'Состояние работы' }).click();
        assert(await page.getByLabel('Состояние работы').inputValue() === 'paused',
          'S020: project status persists after member-scoped redirect');
        // S019: the same normalized URL from any site must not create a duplicate.
        // Keep query and path semantics; no external URL fetch.
        const providerId = (viewport === 'desktop' ? 910100 : 910200)
          + (scheme === 'light' ? 1 : 2);
        await page.goto(base + '/me/books/new');
        await page.getByLabel('Название книги').fill('Исходная книга ' + providerId);
        await page.getByLabel('Ссылка на первоисточник')
          .fill('http://www.example.org/work/' + providerId + '/');
        await page.getByRole('button', { name: 'Создать книгу' }).click();
        await page.waitForURL(/\/me\/books\/\d+$/);
        await page.goto(base + '/me/books/new');
        await page.getByLabel('Название книги').fill('Совсем другое название ' + providerId);
        await page.getByLabel('Ссылка на первоисточник')
          .fill('https://example.org/work/' + providerId);
        const [providerDuplicate] = await Promise.all([
          page.waitForResponse(r => r.url() === base + '/me/books' &&
            r.request().method() === 'POST'),
          page.getByRole('button', { name: 'Создать книгу' }).click(),
        ]);
        assert(providerDuplicate.status() === 409,
          'S019: identical normalized URL under changed title must be rejected');
        assert(await page.getByRole('heading', { name: 'Книга не создана' }).isVisible(),
          'S019: generic URL conflict must preserve actionable error page');
        // S019: an actual original and a mirror may be from arbitrary hosts.
        // Registration must claim both in one transaction without fetching them.
        const workNum = providerId + 500000;
        const chineseOriginal = 'https://fiction.example.cn/novel/' + workNum;
        const mirrorLink = 'https://mirror.example.org/work/' + workNum;
        await page.goto(base + '/me/books/new');
        await page.getByLabel('Название книги')
          .fill('Перевод китайского оригинала ' + workNum);
        await page.getByLabel('Ссылка на первоисточник').fill(chineseOriginal);
        await page.getByLabel('Дополнительная ссылка на это произведение').fill(mirrorLink);
        await page.getByRole('button', { name: 'Создать книгу' }).click();
        await page.waitForURL(/\/me\/books\/\d+$/);
        const sourceBookId = page.url().split('/').pop();
        await page.getByRole('link', { name: 'Ссылки на произведение' }).click();
        assert(await page.getByLabel('Ссылка на оригинал').inputValue() === chineseOriginal,
          'S019: source settings must show the existing original');
        assert(await page.getByLabel('Дополнительная ссылка на произведение')
          .inputValue() === mirrorLink, 'S019: existing mirror must be shown');
        const editedMirror = 'https://mirror.example.org/updated/' + workNum;
        await page.getByLabel('Дополнительная ссылка на произведение').fill(editedMirror);
        const [linksSaved] = await Promise.all([
          page.waitForResponse(r => r.url() === base + '/me/books/' + sourceBookId +
            '/settings/sources' && r.request().method() === 'POST'),
          page.getByRole('button', { name: 'Сохранить ссылки' }).click(),
        ]);
        assert(linksSaved.status() === 303, 'S019: member may save URLs');
        await page.waitForURL(base + '/me/books/' + sourceBookId + '/settings');
        await page.goto(base + '/me/books/' + sourceBookId + '/settings/sources');
        assert(await page.getByLabel('Дополнительная ссылка на произведение')
          .inputValue() === editedMirror, 'S019: saved mirror must survive reload');

        await page.goto(base + '/me/books/new');
        await page.getByLabel('Название книги').fill('Другое название зеркала ' + workNum);
        await page.getByLabel('Ссылка на первоисточник').fill(editedMirror);
        const [mirrorDuplicate] = await Promise.all([
          page.waitForResponse(r => r.url() === base + '/me/books' &&
            r.request().method() === 'POST'),
          page.getByRole('button', { name: 'Создать книгу' }).click(),
        ]);
        assert(mirrorDuplicate.status() === 409,
          'S019: a supplied mirror URL must prevent duplicate registration');
        assert(await page.getByRole('heading', { name: 'Книга не создана' }).isVisible(),
          'S019: conflict must not reveal private book details');
        await page.goto(base + '/me/books/' + bookId);

        await page.getByRole('link', { name: 'Добавить главу' }).click();
        assert(await page.getByLabel('Номер главы').inputValue() === '1',
          'S021: new book starts with automatic chapter number 1');
        await page.getByLabel('Название главы').fill('Проверочная глава');
        await page.getByLabel('Текст главы').fill('Опубликованный текст с обложкой.');
        await page.getByRole('button', { name: 'Сохранить главу' }).click();
        await page.waitForURL(base + '/me/books/' + bookId);
        await page.getByRole('button', { name: 'Опубликовать книгу' }).click();
        await page.waitForURL(base + '/books/' + bookId);
        const awaitingApproval = await page.goto(base + '/books/' + bookId);
        assert(awaitingApproval && awaitingApproval.status() === 404,
          'Unapproved Book unexpectedly has a public cover page');
        approvePendingBookForIsolatedCi(bookId);
        const approved = await page.goto(base + '/books/' + bookId);
        assert(approved && approved.status() === 200,
          'Test-approved Book must become visible without changing author release');

        let response = await page.goto(base + '/me/books/' + bookId + '/settings/covers');
        assert(response && response.status() === 200,
          viewport + '/' + scheme + ': real owner cover page unavailable');
        const csp = response.headers()['content-security-policy'];
        assert(csp && csp.includes("script-src 'self'") && csp.includes("img-src 'self' blob:"),
          'Page-specific restrictive CSP lost');
        await shot(page, viewport, scheme, 'empty');

        const file = await actualPng(page);
        await page.locator('[data-cover-picker-input]').setInputFiles({
          name: 'ci-real-cover.png', mimeType: 'image/png', buffer: file,
        });
        await page.waitForFunction(() => {
          const image = document.querySelector('[data-cover-picker-image]');
          return image && !image.hidden && image.src.startsWith('blob:');
        });
        assert(await page.locator('[data-cover-picker-filename]').innerText()
          === 'ci-real-cover.png', 'NEwBOO local preview did not update');
        await shot(page, viewport, scheme, 'selected');

        await page.getByRole('button', { name: 'Загрузить обложку' }).click();
        await page.waitForURL(base + '/me/books/' + bookId + '/settings/covers');
        await verifyModeratorWorkbench(moderator, page, viewport, scheme, width, height);
        await approveReadyCover(moderator, page, bookId, viewport, scheme);
        await readyCover(page, bookId, viewport, scheme);
        await shot(page, viewport, scheme, 'ready');

        // S019: published book appears as a hint before a duplicate is submitted.
        await page.goto(base + '/me/books/new');
        await page.getByLabel('Название книги').fill(title);
        await page.waitForFunction(id =>
          document.querySelector('#book-title-suggestions a[href="/books/' + id + '"]') !== null,
          bookId, { timeout: 10000 });
        const hint = page.locator('#book-title-suggestions');
        assert(await hint.getByRole('link', { name: title }).isVisible(),
          'S019: published title hint must point to the real book');
        await page.goto(base + '/books/' + bookId);
        const publicBook = await page.goto(base + '/books/' + bookId);
        assert(publicBook && publicBook.status() === 200, 'Published book lost');
        assert(await page.locator('img[src="/covers/' + bookId + '/main.webp"]').count() === 1,
          'Published book does not display its verified cover');
        const card = await context.request.get(base + '/covers/' + bookId + '/card.webp');
        const main = await context.request.get(base + '/covers/' + bookId + '/main.webp');
        for (const [kind, image] of [['card', card], ['main', main]]) {
          assert(image.status() === 200, kind + ': public WebP HTTP is not readable');
          const bytes = await image.body();
          assert(bytes.subarray(0, 4).toString() === 'RIFF' &&
            bytes.subarray(8, 12).toString() === 'WEBP',
          kind + ': invalid WebP bytes');
        }
        await page.goto(base + '/catalog?q=' + encodeURIComponent(title));
        assert(await page.locator('img[src="/covers/' + bookId + '/card.webp"]').count() >= 1,
          'Cover missing from real search results');
        await page.goto(base + '/');
        assert(await page.locator('img[src="/covers/' + bookId + '/card.webp"]').count() >= 1,
          'Cover missing from real home showcase');

        const anon = await browser.newContext();
        const denied = await anon.request.get(base + '/me/books/' + bookId + '/settings/covers', {
          maxRedirects: 0,
        });
        assert(denied.status() === 303,
          'Anonymous browser must not open owner cover settings');
        await anon.close();
        await context.close();
        console.log('H-web-11 PASS: ' + viewport + ' ' + scheme +
          ' real session -> published book -> S3 -> PostgreSQL worker -> public WebP');
      }
    }
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
