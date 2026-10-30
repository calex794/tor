// E-04/S013: real registration -> POST -> PostgreSQL -> reload -> keyboard reveal.
// Runs after reader smoke's empty-discussion assertions, against its isolated tor_ci.
const path = require('node:path');
const assert = require('node:assert/strict');

module.exports = async function checkSpoilers(browser, base, out) {
  for (const [name, width] of [['desktop', 1440], ['mobile', 390]]) {
    const context = await browser.newContext({ viewport: { width, height: 900 } });
    try {
      const page = await context.newPage();
      await page.goto(base + '/register');
      await page.getByLabel('Отображаемое имя').fill('Читатель спойлеров');
      await page.getByLabel('Электронная почта').fill(`spoiler.${name}@example.test`);
      await page.getByLabel('Пароль (12–128 символов)').fill('SpoilerTestPassword2026');
      await page.getByRole('button', { name: 'Создать учётную запись' }).click();
      await page.waitForURL('**/me');
      await page.goto(base + '/books/1/comments');
      const marker = 'SECRET-' + name;
      const body = `Вступление ||${marker} <img src=x onerror="window.spoilerXss=1"> @private_user @[user:42]|| конец ||ещё|| ||незакрытый`;
      await page.getByLabel('Ваш комментарий').fill(body);
      await page.getByRole('button', { name: 'Отправить', exact: true }).click();
      await page.waitForURL('**/books/1/comments#comments');
      await page.reload();
      const card = page.locator('.comment-card').filter({ hasText: marker });
      assert.equal(await card.count(), 1, 'POST/reload must yield one persisted comment');
      const details = card.locator('details.comment-spoiler');
      assert.equal(await details.count(), 2, 'paired spoilers only');
      const secret = details.first().locator('.comment-spoiler__content');
      assert.equal(await secret.isVisible(), false, 'secret initially hidden');
      assert.equal(await details.first().getAttribute('open'), null);
      const toggle = details.first().locator('summary');
      await toggle.focus();
      await page.keyboard.press('Enter');
      assert.equal(await secret.isVisible(), true, 'keyboard reveal');
      assert((await secret.innerText()).includes(marker));
      assert.equal(await card.locator('img, script').count(), 0, 'HTML is text, not elements');
      assert.equal(await page.evaluate(() => window.spoilerXss), undefined);
      assert(!(await card.innerHTML()).includes('@private_user'), 'private handle masked');
      assert(!(await card.innerHTML()).includes('@[user:42]'), 'structured ID masked');
      assert((await card.innerText()).includes('||незакрытый'), 'source unclosed-pair policy');
      await page.keyboard.press('Space');
      assert.equal(await secret.isVisible(), false, 'keyboard hide');
      await page.screenshot({ path: path.join(out, `spoiler-${name}.png`), fullPage: true });
      const sizes = await page.evaluate(() => [document.documentElement.scrollWidth, innerWidth]);
      assert(sizes[0] <= sizes[1] + 1, 'no horizontal overflow');
      // No JavaScript required: guest can reveal the persisted public discussion.
      const guest = await browser.newContext({ javaScriptEnabled: false });
      try {
        const guestPage = await guest.newPage();
        await guestPage.goto(base + '/books/1/comments');
        const guestCard = guestPage.locator('.comment-card').filter({ hasText: marker });
        const guestSecret = guestCard.locator('.comment-spoiler__content').first();
        assert.equal(await guestSecret.isVisible(), false);
        await guestCard.locator('summary').first().click();
        assert.equal(await guestSecret.isVisible(), true);
      } finally {
        await guest.close();
      }
      // E-03: second reader replies through the persisted public form.
      const responder = await browser.newContext({ viewport: { width, height: 900 } });
      try {
        const replyPage = await responder.newPage();
        await replyPage.goto(base + '/register');
        await replyPage.getByLabel('Отображаемое имя').fill('Второй читатель');
        await replyPage.getByLabel('Электронная почта').fill(`reply.${name}@example.test`);
        await replyPage.getByLabel('Пароль (12–128 символов)').fill('ReplyTestPassword2026');
        await replyPage.getByRole('button', { name: 'Создать учётную запись' }).click();
        await replyPage.waitForURL('**/me');
        await replyPage.goto(base + '/books/1/comments');
        const target = replyPage.locator('.comment-card').filter({ hasText: marker });
        const parentId = (await target.getAttribute('id')).replace('comment-', '');
        await target.locator('details.comment-reply > summary').click();
        await target.getByLabel(`Ваш ответ на комментарий №${parentId}`, { exact: true }).fill(`Ответ-${name} ||скрытый ответ||`);
        await target.getByRole('button', { name: 'Отправить ответ', exact: true }).click();
        await replyPage.waitForURL('**/books/1/comments#comments');
        await replyPage.reload();
        const reply = replyPage.locator('.comment-card').filter({ hasText: `Ответ-${name}` });
        assert.equal(await reply.count(), 1);
        assert((await reply.innerText()).includes(`Ответ на комментарий №${parentId}`));
        assert.equal(await reply.locator('.comment-spoiler__content').isVisible(), false);
        await replyPage.screenshot({ path: path.join(out, `reply-${name}.png`), fullPage: true });
        const sizes = await replyPage.evaluate(() => [document.documentElement.scrollWidth, innerWidth]);
        assert(sizes[0] <= sizes[1] + 1, 'reply page no horizontal overflow');
        const replyId = (await reply.getAttribute('id')).replace('comment-', '');
        await page.goto(base + '/me');
        const inbox = page.locator('section[aria-labelledby="me-replies-title"]');
        const link = inbox.locator(`a[href$="#comment-${replyId}"]`);
        assert.equal(await link.count(), 1, 'only parent author receives reply');
        assert(!(await inbox.innerText()).includes('скрытый ответ'), 'no spoiler copy in dashboard');
        await page.screenshot({ path: path.join(out, `received-replies-${name}.png`), fullPage: true });
        await link.click();
        await page.waitForURL(`**#comment-${replyId}`);
        assert.equal(await page.locator(`#comment-${replyId}`).count(), 1);
        // Complete the conversation through the private Inbox, using its real form.
        await page.goto(base + '/me/inbox?filter=unread');
        const received = page.locator('article.me-now-card').filter({
          has: page.locator(`a[href$="#comment-${replyId}"]`),
        });
        assert.equal(await received.count(), 1);
        assert(!(await received.innerText()).includes('скрытый ответ'), 'Inbox contains no body copy');
        await received.locator('details.comment-reply > summary').click();
        const responseMarker = 'INBOX-ANSWER-' + name;
        await received.getByLabel('Ваш ответ', { exact: true }).fill(responseMarker + ' ||ответ из Inbox||');
        const form = received.locator('form.comments-form');
        const submitted = await form.evaluate(el => Object.fromEntries(new FormData(el)));
        assert.equal(submitted.parent_id, replyId, 'reply targets the original canonical comment');
        assert.equal(submitted.return_to, 'inbox');
        await form.getByRole('button', { name: 'Отправить ответ', exact: true }).click();
        await page.waitForURL(base + '/me/inbox');
        const replay = await context.request.post(base + '/books/1/comments', {
          form: submitted, headers: { Origin: base }, maxRedirects: 0,
        });
        assert.equal(replay.status(), 303, 'retry remains idempotent');
        assert.equal(replay.headers().location, '/me/inbox');
        const entry = page.locator('article.me-now-card').filter({
          has: page.locator(`a[href$="#comment-${replyId}"]`),
        });
        await entry.getByRole('button', { name: 'Отметить прочитанным', exact: true }).click();
        await page.waitForURL(base + '/me/inbox');
        await page.goto(base + '/me/inbox?filter=read');
        assert.equal(await page.locator(`a[href$="#comment-${replyId}"]`).count(), 1);
        await page.screenshot({ path: path.join(out, `inbox-inline-${name}.png`), fullPage: true });
        const inboxSizes = await page.evaluate(() => [document.documentElement.scrollWidth, innerWidth]);
        assert(inboxSizes[0] <= inboxSizes[1] + 1, 'Inbox no horizontal overflow');
        await entry.getByRole('button', { name: 'Отметить непрочитанным', exact: true }).click();
        await page.waitForURL(base + '/me/inbox');
        await page.goto(base + '/me/inbox?filter=unread');
        assert.equal(await page.locator(`a[href$="#comment-${replyId}"]`).count(), 1);
        await page.goto(base + '/books/1/comments');
        const response = page.locator('.comment-card').filter({ hasText: responseMarker });
        assert.equal(await response.count(), 1, 'resubmission must not duplicate the answer');
        assert((await response.innerText()).includes(`Ответ на комментарий №${replyId}`));
        assert.equal(await response.locator('.comment-spoiler__content').isVisible(), false);
        const responseId = (await response.getAttribute('id')).replace('comment-', '');
        await replyPage.goto(base + '/me/inbox');
        const returned = replyPage.locator(`a[href$="#comment-${responseId}"]`);
        assert.equal(await returned.count(), 1, 'answer returns to the author of the received reply');
        assert(!(await replyPage.locator('main').innerText()).includes(responseMarker), 'no answer body copied');
        await returned.click();
        await replyPage.waitForURL(`**#comment-${responseId}`);
        assert.equal(await replyPage.locator(`#comment-${responseId}`).count(), 1);
        await replyPage.goto(base + '/me');
        assert.equal(await replyPage.locator(`section[aria-labelledby="me-replies-title"] a[href$="#comment-${replyId}"]`).count(), 0, 'writer is not recipient');

      } finally {
        await responder.close();
      }

    } finally {
      await context.close();
    }
  }
  console.log('E-04/S013 public comment spoilers: PostgreSQL + Chromium PASS');
};
