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

function crc32(input) {
  let crc = 0xffffffff;
  for (const value of input) {
    crc ^= value;
    for (let bit = 0; bit < 8; bit++) {
      crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

// Small valid DOCX stored ZIP fixture without a third-party JS ZIP package.
function makeZip(members) {
  const localParts = [];
  const centralParts = [];
  let offset = 0;
  let cdSize = 0;
  for (const [filename, text] of members) {
    const name = Buffer.from(filename);
    const bytes = Buffer.from(text, 'utf8');
    const crc = crc32(bytes);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(bytes.length, 18);
    local.writeUInt32LE(bytes.length, 22);
    local.writeUInt16LE(name.length, 26);
    localParts.push(local, name, bytes);

    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(bytes.length, 20);
    central.writeUInt32LE(bytes.length, 24);
    central.writeUInt16LE(name.length, 28);
    central.writeUInt32LE(offset, 42);
    centralParts.push(central, name);
    cdSize += central.length + name.length;
    offset += local.length + name.length + bytes.length;
  }
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(members.length, 8);
  end.writeUInt16LE(members.length, 10);
  end.writeUInt32LE(cdSize, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...localParts, ...centralParts, end]);
}

function makeDocxXml(xml) {
  return makeZip([
    ['[Content_Types].xml', '<Types/>'],
    ['word/document.xml', xml],
  ]);
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
        // We are already on the importer after the oversized Windows-1251 test.
        // DOCX is validated on preview, then editable before PG confirmation.
        const docx = makeDocxXml(
          '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
          + '<w:body><w:p><w:r><w:t>Глава 4: Ворд</w:t></w:r></w:p>'
          + '<w:p><w:r><w:t>Текст &amp; проверка</w:t></w:r></w:p>'
          + '</w:body></w:document>');
        await page.locator('#chapter-import-file').setInputFiles({
          name: 'chapter.docx',
          mimeType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
          buffer: docx,
        });
        await page.getByText(/DOCX прочитан/).waitFor();
        await page.getByRole('button', { name: 'Проверить главы' }).click();
        await page.getByText(/Найдено глав: 1/).waitFor();
        const extracted = await page.locator('#chapter-import-text').inputValue();
        assert(extracted.includes('Глава 4: Ворд') && extracted.includes('Текст & проверка'),
          name + ': DOCX extraction lost Russian text or XML entities');
        await page.locator('#chapter-import-text').fill(
          extracted.replace('Текст & проверка', 'Текст исправлен автором'));
        assert(await page.getByRole('button', { name: 'Подтвердить и создать главы' }).isDisabled(),
          name + ': changing DOCX text did not revoke preview confirmation');
        await page.getByRole('button', { name: 'Проверить главы' }).click();
        await page.getByText(/Найдено глав: 1/).waitFor();
        await page.getByRole('button', { name: 'Подтвердить и создать главы' }).click();
        await page.waitForURL(base + '/me/books/' + id);
        assert(await page.getByText(/№4/).isVisible(),
          name + ': extracted DOCX chapter not saved');

        // A ZIP with out-of-order member names must preview in natural order.
        await importLink.click();
        await page.waitForURL(base + '/me/books/' + id + '/chapters/import');
        const zipped = makeZip([
          ['10.txt', 'Глава 10: Десятая\nТекст десятой главы'],
          ['5.txt', 'Глава 5: Пятая\nТекст пятой главы'],
        ]);
        await page.locator('#chapter-import-file').setInputFiles({
          name: 'numbered-chapters.zip', mimeType: 'application/zip',
          buffer: zipped,
        });
        await page.getByText(/ZIP прочитан/).waitFor();
        await page.getByRole('button', { name: 'Проверить главы' }).click();
        await page.getByText(/Найдено глав: 2/).waitFor();
        const titles = await page.locator('#chapter-import-results li strong').allTextContents();
        assert(titles[0].includes('Глава 5') && titles[1].includes('Глава 10'),
          name + ': ZIP members not naturally sorted');
        await page.getByRole('button', { name: 'Подтвердить и создать главы' }).click();
        await page.waitForURL(base + '/me/books/' + id);
        assert(await page.getByText(/№5/).isVisible() && await page.getByText(/№10/).isVisible(),
          name + ': ZIP chapter batch not saved');

        await importLink.click();
        await page.waitForURL(base + '/me/books/' + id + '/chapters/import');
        const unsafe = makeZip([
          ['../escape.txt', 'Глава 6: Опасная\nТекст'],
        ]);
        await page.locator('#chapter-import-file').setInputFiles({
          name: 'unsafe.zip', mimeType: 'application/zip',
          buffer: unsafe,
        });
        await page.getByRole('button', { name: 'Проверить главы' }).click();
        await page.getByText(/Не удалось проверить файл/).waitFor();
        assert(await page.getByRole('button', { name: 'Подтвердить и создать главы' }).isDisabled(),
          name + ': unsafe ZIP unexpectedly enabled confirmation');

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
