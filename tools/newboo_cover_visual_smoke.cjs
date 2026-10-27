// Visual contract only: renders the exact production cover settings template
// inside the real TOR page shell, with no authentication/S3 test backdoors.
// The PostgreSQL owner/CSRF/worker integration is checked separately.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');

const base = 'http://127.0.0.1:3099';
const out = process.env.TOR_SCREENSHOT_DIR || 'tor-shots';
const template = fs.readFileSync(
  path.join(__dirname, '../crates/web/templates/author_cover_settings.html'), 'utf8'
);
fs.mkdirSync(out, { recursive: true });

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
function fixtureHtml() {
  return template
    .replaceAll('<!--BOOK_ID-->', '42')
    .replaceAll('<!--BOOK_TITLE-->', 'Проверка оформления обложки')
    .replaceAll('<!--CSRF_TOKEN-->', 'ci-visual-only')
    .replaceAll('<!--COVER_STATUS-->', 'Обложка ещё не загружена.')
    .replaceAll('<!--COVER_IMAGE-->',
      '<img class="book-cover-picker__image" data-cover-picker-image alt="" hidden>')
    .replaceAll('<!--PLACEHOLDER_HIDDEN-->', '')
    .replaceAll('<!--CHANGE_LABEL-->', 'Добавить')
    .replaceAll('<!--FILE_LABEL-->', 'Файл не выбран');
}
async function checkLayout(page, label) {
  const measured = await page.evaluate(() => {
    const frame = document.querySelector('.book-cover-picker__frame');
    const css = getComputedStyle(frame);
    return {
      frame: frame.getBoundingClientRect().toJSON(),
      border: css.borderTopStyle,
      doc: document.documentElement.scrollWidth,
      viewport: document.documentElement.clientWidth,
      cards: document.querySelectorAll('.book-cover-picker').length,
      title: document.querySelector('h1').textContent,
    };
  });
  assert(measured.doc <= measured.viewport + 1,
    label + ': page overflows viewport ' + JSON.stringify(measured));
  assert(Math.abs(measured.frame.width / measured.frame.height - 2 / 3) < 0.035,
    label + ': NEwBOO 2:3 frame was lost: ' + JSON.stringify(measured));
  assert(measured.border === 'dashed', label + ': NEwBOO dashed picker missing');
  assert(measured.cards === 1, label + ': TOR must not expose fake cover slots');
  assert(measured.title === 'Обложки книги', label + ': wrong reference heading');
}
(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [mode, width, height] of [
      ['desktop', 1440, 900], ['mobile', 390, 844],
    ]) {
      for (const scheme of ['light', 'dark']) {
        const ctx = await browser.newContext({
          viewport: { width, height }, colorScheme: scheme, bypassCSP: true,
        });
        const page = await ctx.newPage();
        await page.goto(base + '/catalog');
        await page.locator('main#main').evaluate(
          (node, markup) => { node.innerHTML = markup; },
          fixtureHtml()
        );
        await page.addScriptTag({ url: base + '/assets/newboo-cover-picker.js' });
        const key = mode + '-' + scheme;
        await checkLayout(page, key);
        await page.screenshot({
          path: path.join(out, 'newboo-cover-fixture-' + key + '-empty.png'),
          fullPage: true,
        });
        await page.locator('[data-cover-picker-input]').setInputFiles({
          name: 'ci-cover.png',
          mimeType: 'image/png',
          buffer: Buffer.from(
            'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9SGOHUgAAAAASUVORK5CYII=',
            'base64'
          ),
        });
        await page.waitForFunction(() =>
          !document.querySelector('[data-cover-picker-image]').hidden
        );
        const caption = await page.locator('[data-cover-picker-filename]').innerText();
        assert(caption === 'ci-cover.png', key + ': original picker preview is inactive');
        await checkLayout(page, key + '/selected');
        await page.screenshot({
          path: path.join(out, 'newboo-cover-fixture-' + key + '-selected.png'),
          fullPage: true,
        });
        await ctx.close();
      }
    }
    console.log('NEwBOO cover visual fixture PASS: 390/1440, light/dark, empty/selected');
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });
