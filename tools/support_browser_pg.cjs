// S050/S051: a guest asks, logs in, keeps the chat and gets an admin answer.
const assert = require('node:assert/strict');
const path = require('node:path');
const {execFileSync} = require('node:child_process');

function administrator(email) {
  const db = process.env.TEST_DATABASE_URL || process.env.APP_URL || '';
  assert(db.endsWith('/tor_ci'));
  execFileSync('psql', ['-X','-q','-v','ON_ERROR_STOP=1','-v','email=' + email,'--dbname',db,'--file','-'], {
    input: "INSERT INTO account_roles(user_id,role,is_active) SELECT id,'administrator',true FROM users WHERE email_normalized=:'email' ON CONFLICT(user_id,role) DO UPDATE SET is_active=true;",
    encoding: 'utf8',
  });
}

async function register(page, base, email, name) {
  await page.goto(base + '/register');
  await page.getByLabel('Отображаемое имя').fill(name);
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('SupportOnlyBrowserPassword2026');
  await page.getByRole('button',{name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
}

module.exports = async (browser, base, out) => {
  for (const [name,width] of [['desktop',1440],['mobile',390]]) {
    const guest = await browser.newContext({viewport:{width,height:900}});
    const admin = await browser.newContext({viewport:{width,height:900}});
    const other = await browser.newContext({viewport:{width,height:900}});
    try {
      const page = await guest.newPage();
      await page.goto(base + '/');
      await page.getByRole('button',{name:'Поддержка',exact:true}).click();
      await page.locator('[data-support-input]').fill('Виджет ' + name);
      await page.locator('[data-support-submit]').click();
      await page.locator('[data-support-messages]').getByText('Виджет ' + name,{exact:true}).waitFor();
      await page.getByRole('button',{name:'Закрыть',exact:true}).click();

      await page.getByRole('link',{name:'Поддержка',exact:true}).click();
      await page.waitForURL('**/support');
      assert((await page.locator('main').innerText()).includes('30 дней'));
      const marker = 'SUPPORT-PRIVATE-' + name;
      await page.getByLabel('Ваше сообщение',{exact:true}).fill(marker + '\n<script>window.supportXss=1</script>');
      await page.getByRole('button',{name:'Отправить сообщение',exact:true}).click();
      await page.waitForURL('**/support');
      assert((await page.locator('#support-messages').innerText()).includes(marker));
      assert.equal(await page.locator('#support-messages script').count(),0);
      const guestCookie = (await guest.cookies()).find(c => c.name === 'tor-dev-support');
      assert(guestCookie && guestCookie.httpOnly && guestCookie.sameSite === 'Strict');
      await register(page,base,`support.${name}.reader@example.test`,'Поддержка читатель ' + name);
      await page.goto(base + '/support');
      assert((await page.locator('#support-messages').innerText()).includes(marker),'guest transcript survives login');
      assert(!(await page.locator('main').innerText()).includes('30 дней'));
      assert(!(await guest.cookies()).some(c => c.name === 'tor-dev-support'),'consumed capability cleared');
      const stranger = await other.newPage();
      await register(stranger,base,`support.${name}.other@example.test`,'Другой читатель ' + name);
      await stranger.goto(base + '/support');
      assert(!(await stranger.locator('main').innerText()).includes(marker));
      assert.equal((await other.request.get(base + '/admin/support')).status(),403);
      const operator = await admin.newPage();
      const email = `support.${name}.admin@example.test`;
      await register(operator,base,email,'Поддержка оператор ' + name);
      administrator(email);
      await operator.goto(base + '/admin/support');
      await operator.getByRole('link',{name:'Поддержка читатель ' + name,exact:true}).click();
      assert((await operator.locator('#support-messages').innerText()).includes(marker));
      await operator.getByLabel('Ваше сообщение',{exact:true}).fill('Ответ поддержки ' + name);
      await operator.getByRole('button',{name:'Отправить сообщение',exact:true}).click();
      await operator.locator('#support-messages').getByText('Ответ поддержки ' + name,{exact:true}).waitFor();
      await operator.waitForURL(/\/admin\/support\/\d+$/);
      await page.getByRole('link',{name:'Обновить переписку',exact:true}).click();
      assert((await page.locator('#support-messages').innerText()).includes('Ответ поддержки ' + name));
      assert.equal(await page.evaluate(() => window.supportXss),undefined);
      const sizes = await page.evaluate(() => [document.documentElement.scrollWidth,innerWidth]);
      assert(sizes[0] <= sizes[1] + 1);
      await page.screenshot({path:path.join(out,`support-${name}.png`),fullPage:true});
      console.log(`E-11/S050/S051 ${name}: guest -> login -> claim -> admin reply -> private history PASS`);
    } finally {await guest.close();await admin.close();await other.close();}
  }
};
