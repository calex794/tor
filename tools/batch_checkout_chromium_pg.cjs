// S035: real Chromium native forms and disposable PostgreSQL 18 checkout.
// Only tor_ci on loopback is permitted. No external funding or real purchases.
const { chromium } = require('playwright');
const { execFileSync } = require('node:child_process');
const { strict: assert } = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');

const base = 'http://127.0.0.1:3098';
const database = process.env.APP_URL || '';
const url = new URL(database);
assert.equal(url.hostname, '127.0.0.1');
assert.equal(url.pathname, '/tor_ci');
assert.equal(url.username, 'tor_app');
const shots = process.env.TOR_READER_SHOTS || 'reader-shots';
fs.mkdirSync(shots, { recursive: true });

function pg(sql) {
  return execFileSync('psql', [
    '-X', '-q', '-A', '-t', '-v', 'ON_ERROR_STOP=1',
    '--dbname', database, '--command', sql,
  ], { encoding: 'utf8', timeout: 20000 }).trim();
}
function id(sql) {
  const value = pg(sql);
  assert.match(value, /^[1-9][0-9]*$/);
  return Number(value);
}
async function register(page, name, email) {
  await page.goto(base + '/register');
  await page.getByLabel('Отображаемое имя').fill(name);
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('S035-ci-password-2026-long');
  await page.getByRole('button', { name: 'Создать учётную запись' }).click();
  await page.waitForURL('**/me');
  return id("SELECT id FROM users WHERE email_normalized='" + email + "'");
}
function makeBook(seller, suffix) {
  const book = id(
    "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id) " +
    "VALUES ('ci:s035:chromium:" + suffix + "','ru','Пакет для Chromium','public'," +
    seller + ') RETURNING id'
  );
  const chapters = [];
  for (const [number, amount] of [[1, 50], [2, 75]]) {
    const chapter = id(
      'INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) ' +
      "VALUES (" + book + ',' + number + ",'Платная глава','public','paid'," +
      amount + ') RETURNING id'
    );
    const version = id(
      'INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) ' +
      "VALUES (" + chapter + ",1,'S035-PRIVATE-CONTENT') RETURNING id"
    );
    pg('INSERT INTO chapter_publications(chapter_id,version_id) VALUES (' +
       chapter + ',' + version + ')');
    chapters.push({ chapter, version });
  }
  return { book, chapters };
}
async function device(browser, seller, name, width, height) {
  const context = await browser.newContext({ viewport: { width, height } });
  const page = await context.newPage();
  try {
    const buyer = await register(
      page, 'S035 ' + name, 's035-ci-' + name + '@tor-ci.example'
    );
    pg("INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) " +
       "VALUES (" + buyer + ",600,'RUB')");
    const { book, chapters } = makeBook(seller, name);
    const landing = await page.goto(base + '/books/' + book);
    assert.equal(landing.status(), 200);
    assert.equal(await page.getByRole('link', {
      name: 'Купить несколько глав'
    }).count(), 1, name + ': missing book CTA');
    assert(!(await page.content()).includes('S035-PRIVATE-CONTENT'));
    await page.getByRole('link', { name: 'Купить несколько глав' }).click();
    await page.waitForURL('**/buy-chapters');
    for (const { chapter } of chapters) {
      await page.locator('input[name="chapter_' + chapter + '"]').check();
    }
    await page.screenshot({
      path: path.join(shots, 's035-' + name + '-select.png'), fullPage: true
    });
    const select = page.waitForResponse(r =>
      r.url().endsWith('/buy-chapters') && r.request().method() === 'POST');
    await page.getByRole('button', { name: 'Проверить выбор и цену' }).click();
    assert.equal((await select).status(), 200);
    assert(await page.getByRole('heading', {
      name: 'Подтверждение покупки'
    }).isVisible());
    assert(await page.getByText('Итого: 1,25 ₽').isVisible());
    assert.equal(pg('SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=' +
      buyer), '600', 'confirmation must not charge wallet');
    const geometry = await page.evaluate(() => ({
      document: document.documentElement.scrollWidth,
      viewport: document.documentElement.clientWidth
    }));
    assert(geometry.document <= geometry.viewport + 1,
      name + ': confirmation overflows viewport');
    const form = await page.locator('form[action$="/buy-chapters"]')
      .evaluate(element => Object.fromEntries(new FormData(element)));
    await page.screenshot({
      path: path.join(shots, 's035-' + name + '-confirm.png'), fullPage: true
    });
    const buy = page.waitForResponse(r =>
      r.url().endsWith('/buy-chapters') && r.request().method() === 'POST');
    await page.getByRole('button', { name: /Подтвердить покупку на 1,25 ₽/ })
      .click();
    assert.equal((await buy).status(), 200);
    assert(await page.getByRole('heading', {
      name: 'Покупка завершена'
    }).isVisible());
    assert(await page.getByRole('link', {
      name: 'Перейти в библиотеку'
    }).isVisible());
    assert(!(await page.content()).includes('S035-PRIVATE-CONTENT'));
    assert.equal(pg('SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=' +
      buyer), '475');
    const receipts = pg('SELECT id,chapter_id,version_id FROM chapter_purchase_facts ' +
      'WHERE user_id=' + buyer + ' ORDER BY id')
      .split('\n').map(line => line.split('|'));
    assert.equal(receipts.length, 2, 'one commit must grant two distinct chapters');
    const replay = await context.request.post(
      base + '/me/books/' + book + '/buy-chapters',
      { form, headers: { origin: base } }
    );
    assert.equal(replay.status(), 200);
    assert((await replay.text()).includes('Повтор без нового списания'));
    assert.equal(pg('SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=' +
      buyer), '475');
    for (const [purchaseId, chapterId, versionId] of receipts) {
      const read = await context.request.get(base +
        '/api/v1/me/progress/purchased?purchase_id=' + purchaseId +
        '&chapter_id=' + chapterId + '&version_id=' + versionId);
      assert.equal(read.status(), 200, 'purchased version must be authorized');
      assert(!(await read.text()).includes('S035-PRIVATE-CONTENT'));
    }
    await page.getByRole('link', { name: 'Перейти в библиотеку' }).click();
    await page.waitForURL('**/me/library');
    assert(await page.getByRole('heading', {
      name: 'Продолжить чтение'
    }).isVisible());
    const chooser = await page.goto(base + '/me/books/' + book + '/buy-chapters');
    assert.equal(chooser.status(), 200);
    assert(await page.getByText(/нет двух доступных для покупки глав/).isVisible());
    console.log('S035 Chromium ' + name + ' PASS: two chapters, quote, wallet, replay, rights, library');
  } finally {
    await context.close();
  }
}
(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const owner = await browser.newPage();
    const seller = await register(owner, 'S035 owner', 's035-ci-owner@tor-ci.example');
    await owner.close();
    await device(browser, seller, 'desktop', 1440, 900);
    await device(browser, seller, 'mobile', 390, 844);
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
