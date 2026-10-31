// H-09/S023: actual desktop+mobile owner file -> preview -> PG draft import.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const base = 'http://127.0.0.1:3098';
const out = process.env.TOR_AUTHOR_SHOTS || 'author-chapter-shots';
fs.mkdirSync(out, { recursive: true });

function assert(ok, why) {
  if (!ok) throw new Error(why);
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [name, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const context = await browser.newContext({ viewport: { width, height } });
      try {
        const page = await context.newPage();
        await page.goto(base + '/register');
        await page.getByLabel('Отображаемое имя').fill('Импортёр ' + name);
        await page.getByLabel('Электронная почта')
          .fill('h09.import.' + name + '@example.test');
        await page.getByLabel('Пароль (12–128 символов)').fill('H09ImportBrowser2026');
        await page.getByRole('button', { name: 'Создать учётную запись' }).click();
        await page.waitForURL('**/me');
        await page.getByRole('link', { name: 'Мои книги' }).click();
        await page.getByRole('link', { name: 'Создать книгу' }).click();
        await page.getByLabel('Название книги').fill('Импортированный сборник ' + name);
        await page.getByLabel('Язык').fill('ru');
        await page.getByRole('button', { name: 'Создать книгу' }).click();
        await page.waitForURL(/\/me\/books\/\d+$/);
        const id = page.url().split('/').pop();
        const importLink = page.getByRole('link', { name: 'Импорт глав из TXT/MD' });
        assert(await importLink.isVisible(), name + ': import action missing from draft');
        await importLink.click();
        await page.waitForURL(base + '/me/books/' + id + '/chapters/import');
        assert(await page.getByRole('button', { name: 'Подтвердить и создать главы' }).isDisabled(),
          name + ': save enabled without preview');
        const source =
          '# Глава 1: Начало\nПервый **текст**\n'
          + 'Глава 2: <img src=x onerror=window.importX=1>\nВторой текст';
        await page.locator('#chapter-import-file').setInputFiles({
          name: 'chapters.txt',
          mimeType: 'text/plain',
          buffer: Buffer.from(source, 'utf8'),
        });
        await page.getByRole('button', { name: 'Проверить главы' }).click();
        await page.getByText(/Найдено глав: 2/).waitFor();
        assert(await page.getByRole('button', { name: 'Подтвердить и создать главы' }).isEnabled(),
          name + ': clean preview cannot be confirmed');
        assert(await page.locator('#chapter-import-results li').count() === 2,
          name + ': preview lost chapters');
        assert(await page.evaluate(() => window.importX === undefined),
          name + ': imported title executed as HTML');
        await page.screenshot({
          path: path.join(out, 'chapter-import-' + name + '-preview.png'),
          fullPage: true,
        });
        await page.getByRole('button', { name: 'Подтвердить и создать главы' }).click();
        await page.waitForURL(base + '/me/books/' + id);
        assert(await page.getByText(/Черновик/).isVisible(),
          name + ': import published the book without consent');
        assert(await page.getByText(/№1/).isVisible(),
          name + ': chapter one missing after import');
        assert(await page.getByText(/№2/).isVisible(),
          name + ': chapter two missing after import');
        assert(await page.getByRole('link', { name: 'Редактировать', exact: true }).count() === 2,
          name + ': imported draft chapters cannot be edited');
        await page.screenshot({
          path: path.join(out, 'chapter-import-' + name + '-saved.png'),
          fullPage: true,
        });
        // A genuine Windows-1251 file is not accepted as UTF-8 by default.
        await importLink.click();
        await page.waitForURL(base + '/me/books/' + id + '/chapters/import');
        await page.locator('#chapter-import-file').setInputFiles({
          name: 'chapters-legacy.txt', mimeType: 'text/plain',
          buffer: Buffer.from([0xc3,0xeb,0xe0,0xe2,0xe0,0x20,0x33,0x3a,0x20,0xd2,0xf0,0xe5,0xf2,0xfc,0xff,0xa,0xd2,0xe5,0xea,0xf1,0xf2,0x20,0xe2,0x20,0xf1,0xf2,0xe0,0xf0,0xee,0xe9,0x20,0xea,0xee,0xe4,0xe8,0xf0,0xee,0xe2,0xea,0xe5]),
        });
        await page.getByText(/Файл повреждён или имеет другую кодировку/).waitFor();
        assert(await page.getByRole('button', { name: 'Проверить главы' }).isDisabled(),
          name + ': legacy bytes silently accepted as UTF-8');
        await page.getByLabel('Кодировка файла').selectOption('windows-1251');
        await page.getByText(/Файл прочитан/).waitFor();
        await page.getByRole('button', { name: 'Проверить главы' }).click();
        await page.getByText(/Найдено глав: 1/).waitFor();
        assert((await page.locator('#chapter-import-results').innerText()).includes('Третья'),
          name + ': CP1251 title was decoded incorrectly');
        await page.getByRole('button', { name: 'Подтвердить и создать главы' }).click();
        await page.waitForURL(base + '/me/books/' + id);
        assert(await page.getByText(/№3/).isVisible(),
          name + ': decoded chapter was not saved');

        // Original 350001-byte CP1251 can expand beyond 700000 UTF-8 bytes.
        await importLink.click();
        await page.waitForURL(base + '/me/books/' + id + '/chapters/import');
        await page.getByLabel('Кодировка файла').selectOption('windows-1251');
        await page.locator('#chapter-import-file').setInputFiles({
          name: 'expanded-too-large.txt', mimeType: 'text/plain',
          buffer: Buffer.alloc(350001, 0xc0),
        });
        await page.getByText(/После преобразования в UTF-8 текст больше 700 КБ/).waitFor();
        assert(await page.getByRole('button', { name: 'Проверить главы' }).isDisabled(),
          name + ': expanded UTF-8 limit was bypassed');
        const guest = await browser.newContext({ viewport: { width, height } });
        try {
          const guestPage = await guest.newPage();
          const hidden = await guestPage.goto(base + '/books/' + id);
          assert(hidden && hidden.status() === 404,
            name + ': guest can read imported unpublished chapters');
        } finally {
          await guest.close();
        }
      } finally {
        await context.close();
      }
    }
    console.log('TOR H-09 chapter TXT import browser PASS: desktop/mobile, preview, confirm, private drafts');
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });
