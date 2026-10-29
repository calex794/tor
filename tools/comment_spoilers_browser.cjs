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
    } finally {
      await context.close();
    }
  }
  console.log('E-04/S013 public comment spoilers: PostgreSQL + Chromium PASS');
};
