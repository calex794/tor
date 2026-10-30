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
      const form = a.page.locator('form[data-mail-compose]');
      await form.locator('[name="client_message_id"]').evaluate((el, value) => { el.value = value; }, 'mail-replay-' + name);
      const submitted = await form.evaluate(el => Object.fromEntries(new FormData(el)));
      // Fail with the actual network status and visible error, not a generic DOM timeout.
      const sentResponse = a.page.waitForResponse(response =>
        response.url().endsWith(conversation + '/messages') && response.request().method() === 'POST',
      { timeout: 15000 });
      await form.getByRole('button', { name: 'Отправить сообщение', exact: true }).click();
      const sendReply = await sentResponse;
      assert.equal(sendReply.status(), 200, 'AJAX POST status; composer error: ' +
        await a.page.locator('#mail-send-error').textContent());
      assert.match(sendReply.request().headers()['content-type'] || '',
        /^application\/x-www-form-urlencoded/i, 'Axum Form accepts URL-encoded AJAX, not multipart');
      await a.page.locator('.mail-message-body').filter({ hasText: marker }).waitFor({ timeout: 15000 });
      const replay = await a.context.request.post(base + conversation + '/messages', {
        form: submitted, headers: { Origin: base }, maxRedirects: 0,
      });
      assert.equal(replay.status(), 303);
      await b.page.locator('.mail-message-body').filter({ hasText: marker }).waitFor({ timeout: 15000 });
      assert.equal(await b.page.locator('[data-message-id]').count(), 1, 'polling and retry deliver one canonical message');
      // An actual browser recipient gets one in-app notification. This is
      // separate from the NEwBOO mail read state; retries create no duplicates.
      await b.page.goto(base + '/notifications');
      let notificationReady = false;
      for (let attempt = 0; attempt < 18; attempt++) {
        await b.page.reload();
        if (await b.page.locator('[data-notification-kind="mail"]').count() === 1) {
          notificationReady = true;
          break;
        }
        await b.page.waitForTimeout(650);
      }
      assert(notificationReady, 'mail notification must arrive via the durable outbox consumer');
      const notification = b.page.locator('[data-notification-kind="mail"]');
      const notificationText = await notification.innerText();
      assert(notificationText.includes('Новое сообщение'), 'recipient notification title');
      assert(!notificationText.includes(marker), 'notification must never disclose the message body');
      const notificationId = await notification.getAttribute('data-notification-id');
      assert(/^\d+$/.test(notificationId), 'notification id');
      await c.page.goto(base + '/notifications');
      assert.equal(await c.page.locator('[data-notification-kind="mail"]').count(), 0,
        'unrelated account has no access to the recipient feed');
      const cCsrf = await c.page.locator('form[action="/notifications/read-all"] [name="csrf_token"]').inputValue();
      const deniedRead = await c.context.request.post(base + '/notifications/' + notificationId + '/read', {
        form: { csrf_token: cCsrf }, headers: { Origin: base }, maxRedirects: 0,
      });
      assert.equal(deniedRead.status(), 404, 'another logged-in user cannot mark recipient notification read');
      const bCsrf = await b.page.locator('form[action="/notifications/read-all"] [name="csrf_token"]').inputValue();
      const wrongOrigin = await b.context.request.post(base + '/notifications/' + notificationId + '/read', {
        form: { csrf_token: bCsrf }, headers: { Origin: 'https://attacker.invalid' }, maxRedirects: 0,
      });
      assert.equal(wrongOrigin.status(), 403, 'notification mark-read rejects cross-origin requests');
      await notification.getByRole('button', { name: 'Отметить прочитанным' }).click();
      await b.page.waitForURL('**/notifications');
      assert((await b.page.locator('[data-notification-id="' + notificationId + '"]').innerText()).includes('Прочитано'));
      await b.page.goto(base + conversation);
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
      // A transcript longer than the initial 50-message page: older AJAX prepends
      // into the current transcript without losing recent messages or duplicating IDs.
      for (let n = 0; n < 53; n++) {
        const reply = await a.context.request.post(base + conversation + '/messages', {
          form: { body: 'История ' + n, client_message_id: 'history-' + name + '-' + n,
            csrf_token: submitted.csrf_token }, headers: {Origin: base}, maxRedirects: 0,
        });
        assert.equal(reply.status(), 303);
      }
      await a.page.reload();
      assert.equal(await a.page.locator('[data-message-id]').count(), 50);
      await a.page.getByRole('link', {name: 'Предыдущие сообщения', exact: true}).click();
      await a.page.waitForFunction(() => document.querySelectorAll('[data-message-id]').length === 55);
      assert.equal(await a.page.locator('[data-message-id]').count(), 55);
      assert.equal(await a.page.locator('.mail-message-body').filter({hasText: marker}).count(), 2);
      const messageIds = await a.page.locator('[data-message-id]').evaluateAll(els => els.map(e => e.dataset.messageId));
      assert.equal(new Set(messageIds).size, 55);
      await a.page.getByRole('button', { name: 'Добавить в друзья', exact: true }).click();
      await a.page.waitForURL(base + conversation);
      await b.page.reload();
      await b.page.getByRole('button', { name: 'Принять в друзья', exact: true }).click();
      await b.page.waitForURL(base + conversation);
      await a.page.reload();
      assert.equal(await a.page.getByRole('button', { name: 'Удалить из друзей', exact: true }).count(), 1,
        'friendship is accepted only on reciprocal request');
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
      assert.equal(await a.page.locator('[data-message-id]').count(), 50, 'blocked participants retain the latest history page');
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
        // Native keyboard submit avoids smooth-scroll animation on a long transcript.
        await plain.getByRole('button', { name: 'Отправить сообщение', exact: true }).press('Enter');
        await plain.waitForURL(base + conversation);
        assert((await plain.locator('#mail-messages').innerText()).includes('Без JavaScript ' + name));
      } finally { await noJs.close(); }
      console.log(`E-08/S057 mail profile-to-send transport ${name}: PG/HTML/polling/replay/third-user/block/no-JS PASS`);
    } finally { for (const client of clients) await client.context.close(); }
  }
};
