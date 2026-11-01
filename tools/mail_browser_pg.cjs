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

function fixtureBook(visibility) {
  assert(['public', 'draft'].includes(visibility));
  const db = process.env.TEST_DATABASE_URL || process.env.APP_URL || '';
  assert(db.endsWith('/tor_ci'), 'disposable mail book fixture required');
  const nonce = require('node:crypto').randomUUID().replace(/-/g, '');
  const title = visibility === 'public' ? 'Browser book <safe>' : 'BROWSER-SECRET-TITLE';
  const statement = "INSERT INTO books(source_identity,target_language,title,visibility) VALUES('ci:e08:browser:"
    + nonce + "','ru','" + title + "','" + visibility + "') RETURNING id";
  const id = execFileSync('psql', ['-X', '-A', '-t', '-q', '-v', 'ON_ERROR_STOP=1',
    '--dbname', db, '--command', statement], { encoding: 'utf8' }).trim();
  assert(/^[1-9][0-9]*$/.test(id), 'one book fixture ID');
  return id;
}
module.exports = async (browser, base, out) => {
  for (const [name, width] of [['desktop', 1440], ['mobile', 390]]) {
    const clients = [];
    try {
      for (const suffix of ['a', 'b', 'c', 'd']) {
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
      const [a, b, c, d] = clients;
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
      // Open acknowledges the alert and navigates to the source conversation.
      await notification.getByRole('button', { name: 'Открыть', exact: true }).click();
      await b.page.waitForURL('**' + conversation);
      await b.page.goto(base + '/notifications');
      assert((await b.page.locator('[data-notification-id="' + notificationId + '"]').innerText()).includes('Прочитано'));
      await b.page.goto(base + conversation);
      assert.equal(await b.page.locator('#mail-messages img, #mail-messages script').count(), 0);
      assert.equal(await b.page.evaluate(() => window.mailXss), undefined);
      assert.equal(await b.page.locator('#mail-messages a[href="/books/2"]').count(), 0, 'no hidden-book preview');
      assert.equal((await c.context.request.get(base + conversation)).status(), 404, 'third account denied');
      const conversationId = conversation.split('/').at(-1);
      // CI simulates a real signed BIGINT ID beyond Number.MAX_SAFE_INTEGER.
      // Never truncate the cursor in JS: first poll returns this synthetic ID,
      // the next GET must carry exactly the same decimal digits. The fixture
      // stays entirely within this browser page; no DB sequences are changed.
      const largeId = '9007199254740993';
      const probe = await b.context.newPage();
      const seenCursors = [];
      let firstProbe = true;
      try {
        await probe.route('**/mail/*/messages?*', async route => {
          const url = new URL(route.request().url());
          if (url.pathname !== conversation + '/messages' ||
              route.request().method() !== 'GET') {
            await route.continue();
            return;
          }
          seenCursors.push(url.searchParams.get('after_id'));
          const messages = firstProbe ? [{
            id: largeId, sender_id: a.id, text: 'BIGINT cursor probe',
            body_without_book_links: 'BIGINT cursor probe', book_previews: [],
            created_at: '2026-11-01T00:00:00Z', created_at_display: '01.11.2026 03:00'
          }] : [];
          firstProbe = false;
          await route.fulfill({
            status: 200, contentType: 'application/json',
            headers: { 'Cache-Control': 'no-store' },
            body: JSON.stringify({ ok: true, messages, blocked: false, has_more_older: false })
          });
        });
        const nextPoll = probe.waitForResponse(response => {
          const url = new URL(response.url());
          return url.pathname === conversation + '/messages' &&
            url.searchParams.get('after_id') === largeId;
        }, { timeout: 12000 });
        await probe.goto(base + conversation);
        await probe.locator('[data-message-id="' + largeId + '"]').waitFor({ timeout: 8000 });
        await nextPoll;
        assert(seenCursors.includes(largeId), 'BIGINT polling cursor was rounded');
      } finally {
        await probe.close();
      }
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
      // Public numeric links become cards via the same ACL in AJAX, polling and SSR.
      const publicBook = fixtureBook('public');
      const hiddenBook = fixtureBook('draft');
      await a.page.getByLabel('Ваше сообщение', { exact: true })
        .fill('Card /books/' + publicBook + ' hidden /books/' + hiddenBook);
      await a.page.getByRole('button', { name: 'Отправить сообщение', exact: true }).click();
      await a.page.locator('[data-mail-book-preview="' + publicBook + '"]').waitFor({ timeout: 15000 });
      await b.page.locator('[data-mail-book-preview="' + publicBook + '"]').waitFor({ timeout: 20000 });
      await b.page.reload();
      assert.equal(await b.page.locator('[data-mail-book-preview="' + publicBook + '"]').count(), 1);
      assert.equal(await b.page.locator('a[href="/books/' + hiddenBook + '"]').count(), 0);
      assert.equal((await b.page.locator('#mail-messages').innerText()).includes('BROWSER-SECRET-TITLE'), false);
      assert.equal((await b.page.locator('[data-mail-book-preview="' + publicBook + '"]').innerText()).includes('Browser book <safe>'), true);
      const projected = await b.context.request.get(base + '/api/v1/me/messages?conversation_id=' + conversationId);
      assert.equal(projected.status(), 200);
      const projectedMessages = (await projected.json()).messages;
      const bookMessage = projectedMessages.find(item => item.text.includes('/books/' + publicBook));
      assert(bookMessage, 'book link must occur in visible conversation');
      assert.equal(bookMessage.book_previews.length, 1);
      assert.equal(bookMessage.book_previews[0].url, '/books/' + publicBook);
      assert.equal(bookMessage.book_previews[0].cover_url, '');
      assert(bookMessage.body_without_book_links.includes('/books/' + hiddenBook));
      assert.equal((await c.context.request.get(base + conversation)).status(), 404, 'third user is still denied');
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
      // Exercise the actual group-mail UI rather than only compiling routes
      // and testing HTTP forms. d remains a nonmember for negative checks.
      await a.page.goto(base + '/users/' + encodeURIComponent('Почта ' + name + ' c'));
      await a.page.getByRole('button', { name: 'Почта', exact: true }).click();
      await a.page.waitForURL('**/mail/*');
      await a.page.goto(base + '/mail/groups');
      await a.page.locator('#group-title').fill('Группа <встреча> ' + name);
      await a.page.locator('#peer_one').selectOption(String(b.id));
      await a.page.locator('#peer_two').selectOption(String(c.id));
      await a.page.getByRole('button', { name: 'Создать группу' }).click();
      await a.page.waitForURL('**/mail/groups/*');
      const groupPath = new URL(a.page.url()).pathname;
      assert.match(groupPath, /^\/mail\/groups\/[1-9][0-9]*$/);
      assert((await a.page.locator('h1').innerText()).includes('Группа <встреча> ' + name));
      assert.equal(await a.page.locator('встреча').count(), 0, 'group title is escaped');
      assert.equal((await d.context.request.get(base + groupPath)).status(), 404,
        'uninvited account cannot view group title or transcript');

      await b.page.goto(base + groupPath);
      assert.equal(await b.page.locator('#group-messages').getAttribute('data-last-id'), '0');
      const groupForm = a.page.locator('form[action="' + groupPath + '/messages"]');
      const groupKey = await groupForm.locator('[name="command_key"]').inputValue();
      const groupCsrf = await groupForm.locator('[name="csrf_token"]').inputValue();
      const groupBody = 'GROUP-MESSAGE-' + name + ' <img src=x onerror=alert(1)>';
      await groupForm.getByLabel('Сообщение').fill(groupBody);
      const sentGroup = a.page.waitForResponse(response =>
        response.url().endsWith(groupPath + '/messages') && response.request().method() === 'POST');
      await groupForm.getByRole('button', { name: 'Отправить сообщение' }).click();
      assert.equal((await sentGroup).status(), 303, 'group form submits through protected HTTP route');
      await a.page.locator('.mail-message-body').filter({ hasText: 'GROUP-MESSAGE-' + name })
        .waitFor({ timeout: 15000 });
      const groupReplay = await a.context.request.post(base + groupPath + '/messages', {
        form: { body: groupBody, command_key: groupKey, csrf_token: groupCsrf },
        headers: { Origin: base }, maxRedirects: 0,
      });
      assert.equal(groupReplay.status(), 303, 'group retry is idempotent');
      const crossOriginGroup = await a.context.request.post(base + groupPath + '/messages', {
        form: { body: groupBody, command_key: groupKey, csrf_token: groupCsrf },
        headers: { Origin: 'https://attacker.invalid' }, maxRedirects: 0,
      });
      assert.equal(crossOriginGroup.status(), 403, 'group send rejects foreign origin');
      await b.page.locator('.mail-message-body').filter({ hasText: 'GROUP-MESSAGE-' + name })
        .waitFor({ timeout: 15000 });
      await c.page.goto(base + groupPath);
      assert.equal(await b.page.locator('.mail-message-body').count(), 1,
        'recipient receives group mail without a page reload');
      assert.equal(await c.page.locator('.mail-message-body').count(), 1);
      assert.equal(await b.page.locator('.mail-message-body img').count(), 0,
        'HTML in group message never becomes an element');
      assert((await b.page.locator('.mail-message-body').innerText()).includes('<img src=x onerror=alert(1)>'));
      await a.page.screenshot({ path: path.join(out, 'mail-group-' + name + '.png'), fullPage: true });
      const groupSizes = await a.page.evaluate(() => [document.documentElement.scrollWidth, innerWidth]);
      assert(groupSizes[0] <= groupSizes[1] + 1, 'group page has no horizontal overflow');

      // C is a real member and must receive an on-site group notification,
      // whereas the fourth account never gets any private group notification.
      await c.page.goto(base + '/notifications');
      let receivedGroupNotice = false;
      for (let attempt = 0; attempt < 18; attempt++) {
        await c.page.reload();
        if (await c.page.locator('[data-notification-kind="mail"]')
          .filter({ hasText: 'Новое сообщение в группе' }).count() === 1) {
          receivedGroupNotice = true;
          break;
        }
        await c.page.waitForTimeout(650);
      }
      assert(receivedGroupNotice, 'group notification must reach an active recipient');
      const noGroupLeak = await d.context.request.get(base + groupPath);
      assert.equal(noGroupLeak.status(), 404);
      await b.page.goto(base + groupPath);
      await b.page.locator('form[action="' + groupPath + '/leave"]')
        .getByRole('button', { name: 'Выйти из группы' }).click();
      await b.page.waitForURL('**/mail/groups');
      assert.equal((await b.context.request.get(base + groupPath)).status(), 404,
        'departed recipient loses access to the group immediately');
      assert.equal((await c.context.request.get(base + groupPath)).status(), 200,
        'other members retain access when someone exits');

      // Ownership is transferred by a native POST. The former creator may then
      // leave, while the new creator cannot orphan the group by leaving.
      await a.page.goto(base + groupPath);
      await a.page.locator('#group-new-creator').selectOption(String(c.id));
      await a.page.getByRole('button', { name: 'Передать управление' }).click();
      await a.page.locator('#group-new-creator').waitFor({ state: 'detached', timeout: 15000 });
      assert.equal(await a.page.locator('#group-new-creator').count(), 0,
        'previous creator lost management rights');
      await c.page.goto(base + groupPath);
      assert.equal(await c.page.locator('#group-new-creator').count(), 1,
        'new creator can manage group');
      assert.equal(await c.page.locator('form[action="' + groupPath + '/leave"]').count(), 0,
        'new creator cannot leave without another transfer');
      await a.page.locator('form[action="' + groupPath + '/leave"]')
        .getByRole('button', { name: 'Выйти из группы' }).click();
      await a.page.waitForURL('**/mail/groups');
      assert.equal((await a.context.request.get(base + groupPath)).status(), 404);
      assert.equal((await c.context.request.get(base + groupPath)).status(), 200);

      // A pending invitation is not membership: D cannot read until accepting.
      await c.page.goto(base + '/users/' + encodeURIComponent('Почта ' + name + ' d'));
      await c.page.getByRole('button', { name: 'Почта', exact: true }).click();
      await c.page.waitForURL('**/mail/*');
      await c.page.goto(base + groupPath + '/invite');
      await c.page.locator('#group-invite-user').selectOption(String(d.id));
      await c.page.getByRole('button', { name: 'Отправить приглашение' }).click();
      await c.page.waitForURL('**/mail/groups/*/invite');
      assert.equal((await d.context.request.get(base + groupPath)).status(), 404);
      await d.page.goto(base + '/mail/group-invitations');
      assert((await d.page.locator('body').innerText()).includes('Группа <встреча> ' + name));
      await d.page.getByRole('button', { name: 'Принять' }).click();
      await d.page.waitForURL('**/mail/groups/*');
      assert.equal(new URL(d.page.url()).pathname, groupPath);
      await d.page.locator('.mail-message-body').filter({ hasText: 'GROUP-MESSAGE-' + name })
        .waitFor({ timeout: 15000 });
      await d.page.locator('form[action="' + groupPath + '/leave"]')
        .getByRole('button', { name: 'Выйти из группы' }).click();
      await d.page.waitForURL('**/mail/groups');
      assert.equal((await d.context.request.get(base + groupPath)).status(), 404);
      await c.page.goto(base + groupPath + '/invite');
      await c.page.locator('#group-invite-user').selectOption(String(d.id));
      await c.page.getByRole('button', { name: 'Отправить приглашение' }).click();
      await d.page.goto(base + '/mail/group-invitations');
      await d.page.getByRole('button', { name: 'Отклонить' }).click();
      await d.page.waitForURL('**/mail/group-invitations');
      assert.equal((await d.context.request.get(base + groupPath)).status(), 404);
      console.log('E-08/S057 group mail UI ' + name + ': create/read/send/replay/CSRF/third-user/notifications/leave/transfer/invite/accept/decline PASS');
      console.log(`E-08/S057 mail profile-to-send transport ${name}: PG/HTML/polling/replay/third-user/block/no-JS PASS`);
    } finally { for (const client of clients) await client.context.close(); }
  }
};
