// E-08/S057: three real accounts, private mail, polling and blocking.
const assert = require('node:assert/strict');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
// Fixture-only identity lookup for participant ACL checks; start uses real profile click.
function fixtureUser(email) {
  const db = process.env.TEST_DATABASE_URL || process.env.APP_URL || '';
  assert(db.endsWith('/tor_ci'), 'disposable mail fixture database required');
  const id = execFileSync('psql', ['-X', '-A', '-t', '-q', '-v', 'ON_ERROR_STOP=1',
    '-v', 'mail_email=' + email, '--dbname', db, '--file', '-'], {
    input: "SELECT id FROM users WHERE email_normalized=:'mail_email';", encoding: 'utf8',
  }).trim();
  assert(/^[1-9][0-9]*$/.test(id), 'one fixture account');
  return id;
}

module.exports = async (browser, base, out) => {
  for (const [name, width] of [['desktop', 1440], ['mobile', 390]]) {
    const clients = [];
    try {
      for (const suffix of ['a', 'b', 'c']) {
        const context = await browser.newContext({ viewport: { width, height: 900 } });
        const page = await context.newPage();
        await page.goto(base + '/register');
        await page.getByLabel('Отображаемое имя').fill('Почта ' + name + ' ' + suffix);
        await page.getByLabel('Электронная почта').fill(`mail.${name}.${suffix}@example.test`);
        await page.getByLabel('Пароль (12–128 символов)').fill('MailOnlyTestingPassword2026');
        await page.getByRole('button', { name: 'Создать учётную запись' }).click();
        await page.waitForURL('**/me');
        await page.getByRole('link', { name: 'Личная почта', exact: true }).click();
        assert.equal(await page.getByLabel('Номер получателя').count(), 0);
        const id = fixtureUser(`mail.${name}.${suffix}@example.test`);
        clients.push({ context, page, id });
      }
      const [a, b, c] = clients;
      // E2E profile entry: no fixture POST or invented recipient picker.
      await a.page.goto(base + '/users/' + encodeURIComponent('Почта ' + name + ' b'));
      await a.page.getByRole('button', { name: 'Почта', exact: true }).click();
      await a.page.waitForURL('**/mail/*');
      const conversation = new URL(a.page.url()).pathname;
      await b.page.goto(base + conversation); // Open empty transcript before first send.
      const marker = 'PRIVATE-MAIL-' + name;
      await a.page.getByLabel('Ваше сообщение', { exact: true }).fill(marker + '\n<img src=x onerror="window.mailXss=1"> /books/2');
      const form = a.page.locator('form').filter({ has: a.page.locator('[name="command_key"]') });
      await form.locator('[name="client_message_id"]').evaluate((el, value) => { el.value = value; }, 'mail-replay-' + name);
      const submitted = await form.evaluate(el => Object.fromEntries(new FormData(el)));
      await form.getByRole('button', { name: 'Отправить сообщение', exact: true }).click();
      await a.page.locator('.mail-message-body').filter({ hasText: marker }).waitFor({ timeout: 15000 });
      const replay = await a.context.request.post(base + conversation + '/messages', {
        form: submitted, headers: { Origin: base }, maxRedirects: 0,
      });
      assert.equal(replay.status(), 303);
      await b.page.locator('.mail-message-body').filter({ hasText: marker }).waitFor({ timeout: 15000 });
      assert.equal(await b.page.locator('[data-message-id]').count(), 1, 'polling and retry deliver one canonical message');
      assert.equal(await b.page.locator('#mail-messages img, #mail-messages script').count(), 0);
      assert.equal(await b.page.evaluate(() => window.mailXss), undefined);
      assert.equal(await b.page.locator('#mail-messages a[href="/books/2"]').count(), 0, 'no hidden-book preview');
      assert.equal((await c.context.request.get(base + conversation)).status(), 404, 'third account denied');
      const conversationId = conversation.split('/').at(-1);
      assert.equal((await c.context.request.get(base + '/api/v1/me/messages?conversation_id=' + conversationId)).status(), 404);
      assert.equal(await b.page.getByRole('button', { name: /прочитанными/ }).count(), 0,
        'NEwBOO mail has no manual read receipt action');
      await b.page.getByLabel('Ваше сообщение', { exact: true }).fill('Ответ ' + marker);
      await b.page.getByRole('button', { name: 'Отправить сообщение', exact: true }).click();
      await b.page.locator('.mail-message-body').filter({ hasText: 'Ответ ' + marker }).waitFor({ timeout: 15000 });
      await a.page.locator('.mail-message-body').filter({ hasText: 'Ответ ' + marker }).waitFor({ timeout: 15000 });
      await b.page.getByRole('button', { name: 'Заблокировать собеседника', exact: true }).click();
      await b.page.waitForURL(base + conversation);
      assert.equal(await b.page.getByLabel('Ваше сообщение', { exact: true }).count(), 0);
      const denied = await a.context.request.post(base + conversation + '/messages', {
        form: submitted, headers: { Origin: base }, maxRedirects: 0,
      });
      assert.equal(denied.status(), 403, 'stale form cannot bypass a committed block');
      const cannotUnblock = await a.context.request.post(base + conversation + '/block', {
        form: { blocked: 'false', csrf_token: submitted.csrf_token }, headers: { Origin: base }, maxRedirects: 0,
      });
      assert.equal(cannotUnblock.status(), 303);
      assert.equal((await a.context.request.post(base + conversation + '/messages', {
        form: submitted, headers: { Origin: base }, maxRedirects: 0,
      })).status(), 403, 'only blocker can remove their block');
      await a.page.reload();
      assert.equal(await a.page.locator('[data-message-id]').count(), 2, 'blocked participants retain the old history');
      await a.page.screenshot({ path: path.join(out, `mail-blocked-${name}.png`), fullPage: true });
      await b.page.getByRole('button', { name: 'Снять мою блокировку', exact: true }).click();
      await b.page.waitForURL(base + conversation);
      await a.page.reload();
      assert.equal(await a.page.getByLabel('Ваше сообщение', { exact: true }).count(), 1);
      await a.page.screenshot({ path: path.join(out, `mail-conversation-${name}.png`), fullPage: true });
      const sizes = await a.page.evaluate(() => [document.documentElement.scrollWidth, innerWidth]);
      assert(sizes[0] <= sizes[1] + 1, 'mail page has no horizontal overflow');
      // HTML remains usable with scripting disabled.
      const noJs = await browser.newContext({ javaScriptEnabled: false, storageState: await a.context.storageState(), viewport: { width, height: 900 } });
      try {
        const plain = await noJs.newPage();
        await plain.goto(base + conversation);
        await plain.getByLabel('Ваше сообщение', { exact: true }).fill('Без JavaScript ' + name);
        await plain.getByRole('button', { name: 'Отправить сообщение', exact: true }).click();
        await plain.waitForURL(base + conversation);
        assert((await plain.locator('#mail-messages').innerText()).includes('Без JavaScript ' + name));
      } finally { await noJs.close(); }
      console.log(`E-08/S057 mail profile-to-send transport ${name}: PG/HTML/polling/replay/third-user/block/no-JS PASS`);
    } finally { for (const client of clients) await client.context.close(); }
  }
};
