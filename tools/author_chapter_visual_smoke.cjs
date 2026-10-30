// Real desktop/mobile acceptance: draft-book release and immediate public free Save.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');

const base = 'http://127.0.0.1:3098';
const out = process.env.TOR_AUTHOR_SHOTS || 'author-chapter-shots';
fs.mkdirSync(out, { recursive: true });

function assert(ok, reason) {
  if (!ok) throw new Error(reason);
}

async function inspect(page, viewport, name) {
  const sizes = await page.evaluate(() => ({
    doc: document.documentElement.scrollWidth,
    window: document.documentElement.clientWidth,
    body: document.body.scrollWidth,
  }));
  assert(
    sizes.doc <= sizes.window + 1 && sizes.body <= sizes.window + 1,
    viewport + '/' + name + ': horizontal overflow ' + JSON.stringify(sizes)
  );
  assert(await page.locator('h1').isVisible(), viewport + '/' + name + ': missing heading');
  await page.screenshot({
    path: path.join(out, 'author-' + viewport + '-' + name + '.png'),
    fullPage: true,
  });
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [viewport, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const context = await browser.newContext({ viewport: { width, height } });
      const page = await context.newPage();
      await page.goto(base + '/register');
      await page.getByLabel('Отображаемое имя').fill('Автор ' + viewport);
      await page.getByLabel('Электронная почта').fill('author.chapter.' + viewport + '@example.test');
      await page.getByLabel('Пароль (12–128 символов)').fill('TestAuthorPassword2026');
      await page.getByRole('button', { name: 'Создать учётную запись' }).click();
      await page.waitForURL('**/me');

      await page.getByRole('link', { name: 'Мои книги' }).click();
      await page.waitForURL('**/me/books');
      await page.getByRole('link', { name: 'Создать книгу' }).click();
      await page.waitForURL('**/me/books/new');
      await page.getByLabel('Название книги').fill('Новая книга ' + viewport);
      await page.getByLabel('Язык').fill('ru');
      await page.getByRole('button', { name: 'Создать книгу' }).click();
      await page.waitForURL(/\/me\/books\/\d+$/);

      const bookId = page.url().split('/').pop();
      assert(await page.getByRole('link', { name: 'Добавить главу' }).isVisible(),
        viewport + ': missing author chapter action');
      await inspect(page, viewport, 'draft');
      const description = 'Аннотация ' + viewport + ': <script>window.seoInjected=true</script> & текст';
      await page.getByRole('link', { name: 'Редактировать аннотацию', exact: true }).click();
      await page.waitForURL(base + '/me/books/' + bookId + '/description');
      await page.getByLabel('Аннотация (до 4000 символов)').fill(description);
      await inspect(page, viewport, 'description-editor');
      await page.getByRole('button', { name: 'Сохранить аннотацию', exact: true }).click();
      await page.waitForURL(base + '/me/books/' + bookId);
      await page.getByRole('link', { name: 'Редактировать аннотацию', exact: true }).click();
      assert(await page.getByLabel('Аннотация (до 4000 символов)').inputValue() === description,
        viewport + ': description did not survive save and reload');
      await page.getByRole('link', { name: 'Вернуться к книге', exact: true }).click();
      await page.waitForURL(base + '/me/books/' + bookId);
      await page.getByRole('link', { name: 'Добавить главу' }).click();
      await page.waitForURL('**/chapters/new');
      await page.getByLabel('Номер главы').fill('1');
      await page.getByLabel('Название главы').fill('Глава для проверки');
      await page.getByLabel('Текст главы').fill('Проверочный текст: **важная строка**.');
      await inspect(page, viewport, 'editor');
      await page.getByRole('button', { name: 'Сохранить главу' }).click();
      await page.waitForURL(base + '/me/books/' + bookId);
      assert(await page.getByText('Глава для проверки').isVisible(),
        viewport + ': saved chapter absent from author list');
      assert(await page.getByText(/Черновик/).isVisible(),
        viewport + ': saving chapter must not publish the book');
      assert(await page.getByRole('button', { name: 'Опубликовать книгу' }).isVisible(),
        viewport + ': missing explicit publishing control');
      await inspect(page, viewport, 'chapter-saved');
      await page.getByRole('link', { name: 'Редактировать', exact: true }).click();
      await page.waitForURL(/\/chapters\/\d+\/edit$/);
      assert(await page.getByRole('heading', { name: 'Редактировать главу' }).isVisible(),
        viewport + ': missing draft editor');
      assert(await page.locator('h1 a[href^="/reader/"]').count() === 0,
        viewport + ': unpublished draft must not advertise an unreadable reader page');
      assert(await page.getByText('Редакция №1.').isVisible(),
        viewport + ': initial chapter revision missing');
      await page.getByLabel('Текст главы').fill('Обновлённая версия: **важная строка**.');
      await inspect(page, viewport, 'edit-draft');
      await page.getByRole('button', { name: 'Сохранить изменения' }).click();
      await page.waitForURL(base + '/me/books/' + bookId);
      assert(await page.getByText(/Черновик/).isVisible(),
        viewport + ': draft edit must not publish book');

      const hidden = await page.goto(base + '/books/' + bookId);
      assert(hidden && hidden.status() === 404,
        viewport + ': saved chapter exposed before explicit publication');
      await page.goto(base + '/me/books/' + bookId);
      await page.getByRole('button', { name: 'Опубликовать книгу' }).click();
      await page.waitForURL(base + '/books/' + bookId);
      assert(await page.getByRole('heading', { name: 'Новая книга ' + viewport }).isVisible(),
        viewport + ': public book missing after release');
      await inspect(page, viewport, 'published');
      assert(await page.getByText(description, { exact: true }).isVisible(),
        viewport + ': public description missing or changed');
      assert(await page.evaluate(() => window.seoInjected === undefined),
        viewport + ': description executed as HTML');

      await page.getByRole('link', { name: /Глава 1.*Глава для проверки/ }).click();
      await page.waitForURL(/\/reader\/\d+$/);
      assert(await page.getByText('Обновлённая версия:').isVisible(),
        viewport + ': published chapter text missing');
      await inspect(page, viewport, 'reader');

      // BKR-96: one Save on an already public book immediately publishes
      // the new free chapter, without a second release confirmation.
      await page.goto(base + '/me/books/' + bookId);
      await page.getByRole('link', { name: 'Добавить главу' }).click();
      await page.waitForURL('**/chapters/new');
      assert(await page.getByText(/сразу после сохранения/).isVisible(),
        viewport + ': immediate release help text missing');
      await page.getByLabel('Номер главы').fill('2');
      await page.getByLabel('Название главы').fill('Вторая глава опубликована');
      await page.getByLabel('Текст главы').fill('PUBLIC B06 new');
      await inspect(page, viewport, 'second-editor');
      await page.getByRole('button', { name: 'Сохранить главу' }).click();
      await page.waitForURL(base + '/me/books/' + bookId);
      assert(await page.getByText('Вторая глава опубликована').isVisible(),
        viewport + ': second saved chapter missing in author list');
      assert(!(await page.content()).includes('Черновик (не виден читателям)'),
        viewport + ': saved free chapter must be released immediately');
      assert(await page.getByRole('link', { name: 'Выпустить главу' }).count() === 0,
        viewport + ': released free chapter still asks for confirmation');
      await inspect(page, viewport, 'second-published-author');

      const publicBook = await page.goto(base + '/books/' + bookId);
      assert(publicBook && publicBook.status() === 200,
        viewport + ': public book missing after second Save');
      const secondLink = page.getByRole('link', { name: /Глава 2.*Вторая глава опубликована/ });
      assert(await secondLink.isVisible(),
        viewport + ': second free chapter not visible in public TOC');
      const href = await secondLink.getAttribute('href');
      assert(href && /^\/reader\/\d+$/.test(href),
        viewport + ': released chapter reader link invalid');
      await inspect(page, viewport, 'second-public-toc');
      await secondLink.click();
      await page.waitForURL(/\/reader\/\d+$/);
      assert(await page.getByText('PUBLIC B06 new').isVisible(),
        viewport + ': reader did not display new public version');
      await inspect(page, viewport, 'second-reader');

      // BKR-96: the published free chapter editor is available to its owner.
      // A second release remains forbidden; Save immediately publishes V2.
      const chapterId = href.split('/').pop();
      const edit = await page.goto(base + '/me/books/' + bookId +
        '/chapters/' + chapterId + '/edit');
      assert(edit && edit.status() === 200,
        viewport + ': owner cannot edit a published free chapter');
      assert(await page.getByText(/Сохранение сразу обновит опубликованную редакцию/).isVisible(),
        viewport + ': published editor lacks immediate release explanation');
      assert(await page.getByRole('button', { name: 'Сохранить изменения' }).isVisible(),
        viewport + ': published free chapter editor has no Save');
      // NEwBOO "В главу": in TOR the chapter title itself is clickable.
      const linkedTitle = page.locator('h1 a.author-chapter-view-link');
      assert(await linkedTitle.getAttribute('href') === '/reader/' + chapterId,
        viewport + ': published chapter title does not lead to the real reader');
      await linkedTitle.click();
      await page.waitForURL(base + '/reader/' + chapterId);
      assert(await page.getByText('PUBLIC B06 new').isVisible(),
        viewport + ': title-to-reader navigation lost the published chapter');
      await page.goto(base + '/me/books/' + bookId +
        '/chapters/' + chapterId + '/edit');
      await inspect(page, viewport, 'published-editor-title-link');
      const release = await page.goto(base + '/me/books/' + bookId +
        '/chapters/' + chapterId + '/release');
      assert(release && release.status() === 404,
        viewport + ': published chapter still permits second release');

      // S001/S002/S006/S007: follow the same book across three independent
      // browser identities. A working author session must not substitute for
      // a guest's discovery or a reader's durable personal progress.
      const guestContext = await browser.newContext({ viewport: { width, height } });
      const readerContext = await browser.newContext({ viewport: { width, height } });
      try {
        const guest = await guestContext.newPage();
        const catalogResponse = await guest.goto(base + '/catalog?q=' +
          encodeURIComponent('Новая книга ' + viewport), { waitUntil: 'networkidle' });
        assert(catalogResponse && catalogResponse.status() === 200,
          viewport + ': anonymous catalog failed');
        const bookCard = guest.locator('.book-mini-card__link[href="/books/' + bookId + '"]');
        assert(await bookCard.isVisible(),
          viewport + ': author publication is not discoverable by a guest');
        await bookCard.click();
        await guest.waitForURL(base + '/books/' + bookId);
        const freeLink = guest.locator('a[href="' + href + '"]');
        assert(await freeLink.isVisible(), viewport + ': published free chapter missing in guest TOC');
        await freeLink.click();
        await guest.waitForURL(base + href);
        assert(await guest.getByText('PUBLIC B06 new').isVisible(),
          viewport + ': guest cannot read the real published chapter');

        const reader = await readerContext.newPage();
        const readerEmail = 'reader.chapter.' + viewport + '@example.test';
        await reader.goto(base + '/register');
        await reader.getByLabel('Отображаемое имя').fill('Читатель новой книги ' + viewport);
        await reader.getByLabel('Электронная почта').fill(readerEmail);
        await reader.getByLabel('Пароль (12–128 символов)').fill('ReaderChapterPassword2026');
        await reader.getByRole('button', { name: 'Создать учётную запись' }).click();
        await reader.waitForURL('**/me');

        const bookResponse = await reader.goto(base + '/books/' + bookId);
        assert(bookResponse && bookResponse.status() === 200,
          viewport + ': distinct reader cannot open the book');
        await reader.getByRole('link', { name: 'Добавить в библиотеку' }).click();
        await reader.waitForURL(base + '/me/library/add/' + bookId);
        await reader.getByRole('button', { name: 'Сохранить книгу' }).click();
        await reader.waitForURL('**/me/library');
        assert(await reader.locator('.library-card a[href="/books/' + bookId + '"]').isVisible(),
          viewport + ': new reader bookmark is missing');

        // E-05 / S014: an account-follow is independent of saving a Book.
        // Exercise the real user action and private author-follow listing.
        await reader.goto(base + '/books/' + bookId);
        assert(await reader.getByRole('button', { name: 'Следить за автором' }).isVisible(),
          viewport + ': creator-follow control missing from book');
        await reader.getByRole('button', { name: 'Следить за автором' }).click();
        await reader.waitForURL(base + '/books/' + bookId);
        assert(await reader.getByRole('button', { name: 'Отписаться от автора' }).isVisible(),
          viewport + ': account-follow did not persist');
        await reader.goto(base + '/me/creators');
        assert(await reader.getByRole('heading', { name: 'Подписки на авторов' }).isVisible(),
          viewport + ': private followed-creators page missing');
        assert(await reader.getByText('Автор ' + viewport).isVisible(),
          viewport + ': authoritative owner not in reader follows');
        assert(await reader.getByRole('link', { name: 'Новая книга ' + viewport }).isVisible(),
          viewport + ': source Book attribution missing');
        await inspect(reader, viewport, 'creator-followed');
        await reader.getByRole('button', { name: 'Отписаться от автора' }).click();
        await reader.waitForURL(base + '/me/creators');
        assert(await reader.getByText(/Вы пока не подписаны на авторов/).isVisible(),
          viewport + ': unsubscribe did not deactivate the follow');
        await reader.goto(base + '/books/' + bookId);
        assert(await reader.getByRole('button', { name: 'Следить за автором' }).isVisible(),
          viewport + ': re-follow after unsubscribe unavailable');
        await reader.getByRole('button', { name: 'Следить за автором' }).click();
        await reader.waitForURL(base + '/books/' + bookId);
        assert(await reader.getByRole('button', { name: 'Отписаться от автора' }).isVisible(),
          viewport + ': new active creator period absent');

        const privateReader = await reader.goto(base + '/read/' + chapterId);
        assert(privateReader && privateReader.status() === 200,
          viewport + ': account-only free reader failed');
        assert(await reader.getByRole('heading', { name: 'Вторая глава опубликована' }).isVisible(),
          viewport + ': saved free edition heading missing');
        const savePlace = reader.getByRole('button', { name: 'Сохранить после фрагмента 1' });
        assert(await savePlace.isVisible(), viewport + ': progress form absent');
        await savePlace.click();
        await reader.waitForURL(base + '/read/' + chapterId + '#p-1');
        await reader.goto(base + '/me/library');
        assert(await reader.locator('a[href="/read/' + chapterId + '#p-1"]').isVisible(),
          viewport + ': new reader did not persist a chapter-specific place');
        await inspect(reader, viewport, 'cross-user-saved');

        await reader.goto(base + '/me');
        await reader.getByRole('button', { name: 'Выйти' }).click();
        await reader.waitForURL('**/login');
        await reader.goto(base + '/me/library');
        assert(new URL(reader.url()).pathname === '/login',
          viewport + ': revoked reader session still accesses personal library');
        await reader.getByLabel('Электронная почта').fill(readerEmail);
        await reader.getByLabel('Пароль').fill('ReaderChapterPassword2026');
        await reader.getByRole('button', { name: 'Войти', exact: true }).click();
        await reader.waitForURL('**/me');
        await reader.goto(base + '/me/library');
        const resume = reader.locator('a[href="/read/' + chapterId + '#p-1"]');
        assert(await resume.isVisible(),
          viewport + ': reader lost saved place after logout and login');
        await resume.click();
        await reader.waitForURL(base + '/read/' + chapterId + '#p-1');
        assert(await reader.getByRole('link', { name: 'Перейти к сохранённому месту' }).isVisible(),
          viewport + ': saved anchor not available after return');
        assert(await reader.getByText('PUBLIC B06 new').isVisible(),
          viewport + ': resumed reader text differs from author publication');
        await inspect(reader, viewport, 'cross-user-resumed');
      } finally {
        await guestContext.close();
        await readerContext.close();
      }
      await context.close();
    }
    console.log('TOR H/S001/S002/S006/S007 browser PASS: 390/1440 author release, guest discovery, distinct reader bookmark and resumed progress');
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
