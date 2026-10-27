// F-05: actual Chromium + disposable PostgreSQL.
// The ranking is seeded internally because a trusted C completion is not yet
// implemented. No user-facing "mark as read" or "start ranking" route exists.
const { chromium } = require('playwright');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const base = 'http://127.0.0.1:3098';
const db = process.env.APP_URL;
const shots = process.env.TOR_RANKING_SHOTS || 'ranking-shots';
fs.mkdirSync(shots, { recursive: true });
function assert(ok, message) { if (!ok) throw new Error(message); }
function pg(sql) {
  assert(db && db.endsWith('/tor_ci'), 'F05 refuses non-test PostgreSQL');
  return execFileSync('psql', ['-X', '-v', 'ON_ERROR_STOP=1', '-At', '-d', db, '-c', sql],
    { encoding: 'utf8', timeout: 10000, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}
function positive(value) {
  // psql prints a command tag after INSERT ... RETURNING even with -At.
  // Select the single numeric row and reject missing/multiple IDs.
  const rows = String(value).split(/\r?\n/).map(v => v.trim())
    .filter(v => /^[1-9][0-9]*$/.test(v));
  assert(rows.length === 1, 'non-numeric database ID');
  return rows[0];
}
async function screenshot(page, name) {
  const size = await page.evaluate(() => ({
    content: document.documentElement.scrollWidth,
    viewport: document.documentElement.clientWidth,
    body: document.body.scrollWidth
  }));
  assert(size.content <= size.viewport + 1 && size.body <= size.viewport + 1,
    name + ': mobile horizontal overflow ' + JSON.stringify(size));
  await page.screenshot({ path: path.join(shots, 'ranking-' + name + '.png'), fullPage: true });
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [viewport, width, height] of [['desktop', 1440, 900], ['mobile', 390, 844]]) {
      const context = await browser.newContext({ viewport: { width, height } });
      const page = await context.newPage();
      const email = 'f05.visual.' + viewport + '@example.test';
      await page.goto(base + '/register');
      await page.getByLabel('Отображаемое имя').fill('Читатель F05 ' + viewport);
      await page.getByLabel('Электронная почта').fill(email);
      await page.getByLabel('Пароль (12–128 символов)').fill('F05BrowserSafePassword2026');
      await page.getByRole('button', { name: 'Создать учётную запись' }).click();
      await page.waitForURL('**/me');
      await page.getByRole('link', { name: 'Мой рейтинг' }).first().click();
      await page.waitForURL('**/me/rankings');
      assert(await page.getByText('Пока нет книг для сравнения').isVisible(),
        viewport + ': empty ranking must not invent completed reads');
      await screenshot(page, viewport + '-empty');

      const uid = positive(pg("SELECT id FROM users WHERE email_normalized='" + email + "'"));
      const a = positive(pg("INSERT INTO books(source_identity,target_language,title,visibility) " +
        "VALUES('ci:f05:visual:" + viewport + ":a','ru','Знакомая книга','public') RETURNING id"));
      const b = positive(pg("INSERT INTO books(source_identity,target_language,title,visibility) " +
        "VALUES('ci:f05:visual:" + viewport + ":b','ru','Любимая книга','public') RETURNING id"));
      pg("INSERT INTO user_book_ranking_head(user_id,revision) VALUES(" + uid + ",2); " +
        "INSERT INTO user_book_rank(user_id,book_id,tier) VALUES(" + uid + "," + a + ",0); " +
        "INSERT INTO user_book_ranking_state(user_id,placing_book_id,low_position,high_position,state_version,placement_epoch) " +
        "VALUES(" + uid + "," + b + ",0,0,1,2);");

      const active = await page.goto(base + '/me/rankings', { waitUntil: 'networkidle' });
      assert(active && active.status() === 200, viewport + ': ranked question HTTP failed');
      assert(await page.getByText('Знакомая книга').isVisible(), viewport + ': original A title missing');
      assert(await page.getByText('Любимая книга').isVisible(), viewport + ': original B title missing');
      assert(await page.getByRole('button', { name: 'Первая лучше' }).isVisible(),
        viewport + ': comparison controls missing');
      assert(await page.getByRole('button', { name: 'Отложить вторую' }).isVisible(),
        viewport + ': defer control missing');
      await screenshot(page, viewport + '-question');

      await page.getByRole('button', { name: 'Пропустить' }).click();
      await page.waitForURL('**/me/rankings');
      assert(await page.getByText('Все предложенные пары пропущены').isVisible(),
        viewport + ': skipped pair reappeared without explicit resume');
      await screenshot(page, viewport + '-exhausted');

      await page.getByRole('button', { name: 'Повторить вопросы' }).click();
      await page.waitForURL('**/me/rankings');
      assert(await page.getByRole('button', { name: 'Равнозначны' }).isVisible(),
        viewport + ': question was not explicitly resumed');
      await page.getByRole('button', { name: 'Равнозначны' }).click();
      await page.waitForURL('**/me/rankings');
      assert(await page.getByText('Пока нет книг для сравнения').isVisible(),
        viewport + ': placed books still require a question');
      assert(pg('SELECT count(*) FROM user_book_comparison WHERE user_id=' + uid) === '2',
        viewport + ': skip and tie were not recorded once each');
      await screenshot(page, viewport + '-placed');
      await context.close();
      console.log('F05 Chromium ' + viewport + ' PASS: session, skip, resume, tie and real PG');
    }
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
