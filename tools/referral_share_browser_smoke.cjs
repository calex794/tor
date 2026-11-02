// F-09/S052: Chromium proof of first-touch referral link, signup and private UI.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const { randomBytes } = require('node:crypto');

const base = 'http://127.0.0.1:3098';
const out = process.env.TOR_REFERRAL_SHOTS || 'reader-shots';
fs.mkdirSync(out, { recursive: true });

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function register(context, email, name) {
  const page = await context.newPage();
  const response = await page.goto(base + '/register');
  assert(response && response.status() === 200, 'registration form unavailable');
  await page.getByLabel('Отображаемое имя').fill(name);
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('TestReferralPassword2026');
  await page.getByRole('button', { name: 'Создать учётную запись' }).click();
  await page.waitForURL(base + '/me');
  return page;
}

async function invitationLink(page) {
  await page.goto(base + '/me/referrals');
  const button = page.getByRole('button', { name: 'Создать ссылку' });
  assert(await button.isVisible(), 'missing create invite action');
  await button.click();
  await page.locator('#referral-url').waitFor();
  const url = await page.getByLabel('Ваша реферальная ссылка').inputValue();
  assert(url.startsWith(base + '/r/r_'), 'bad referral URL: ' + url);
  return url;
}

async function referralStats(page) {
  const response = await page.request.get(base + '/api/v1/me/referrals');
  assert(response.status() === 200, 'private referral API not available');
  assert((response.headers()['cache-control'] || '').includes('no-store'),
    'referral API must not be cached');
  return response.json();
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const unique = Date.now().toString(36) + randomBytes(4).toString('hex');
    const firstContext = await browser.newContext({
      viewport: { width: 1440, height: 900 },
    });
    const secondContext = await browser.newContext();
    const visitorContext = await browser.newContext({
      viewport: { width: 390, height: 844 },
    });
    const visitor = await visitorContext.newPage();
    const denied = await visitor.goto(base + '/me/referrals');
    assert(denied && denied.status() === 200 && visitor.url().endsWith('/login'),
      'anonymous visitor should be redirected to login');
    const privateApi = await visitor.request.get(base + '/api/v1/me/referrals');
    assert(privateApi.status() === 401, 'unauthenticated API must return 401');

    const first = await register(
      firstContext, 'referral.owner.' + unique + '@example.invalid', 'Первый'
    );
    const firstUrl = await invitationLink(first);
    const aliasName = 'read-' + unique;
    await first.goto(base + '/me/referrals');
    await first.getByLabel('Тип короткой ссылки').selectOption('go');
    await first.getByLabel('Короткое имя ссылки').fill(aliasName);
    await first.getByRole('button', { name: 'Создать короткую ссылку' }).click();
    await first.waitForURL(base + '/me/referrals');
    const firstShort = await first.getByLabel('Ваша короткая ссылка').inputValue();
    assert(firstShort === base + '/go/' + aliasName, 'incorrect /go vanity URL');
    const firstReport = await referralStats(first);
    assert(firstReport.invitation.short_url === firstShort,
      'private API has no owner vanity link');
    const before = await referralStats(first);
    assert(before.active === 0 && before.invitation.url === firstUrl,
      'first invite missing from owner account');
    await first.goto(base + '/books/1');
    const shareAction = first.getByRole('link', { name: 'Поделиться книгой' });
    assert(await shareAction.isVisible(), 'published book is missing share action');
    await shareAction.click();
    await first.waitForURL(base + '/me/referrals?book=1');
    const bookLink = await first.getByLabel('Ссылка для приглашения на книгу').inputValue();
    assert(bookLink === base + '/books/1?ref=' + firstUrl.slice((base + '/r/').length),
      'book share link does not contain the active referral code');
    const invalid = await first.request.post(base + '/api/v1/me/referrals', {
      form: { csrf_token: 'invalid' },
    });
    assert(invalid.status() === 403, 'CSRF bypassed for referral creation');
    await first.screenshot({
      path: path.join(out, 'referrals-owner-desktop.png'), fullPage: true,
    });

    const second = await register(
      secondContext, 'referral.other.' + unique + '@example.invalid', 'Второй'
    );
    const secondUrl = await invitationLink(second);
    assert(secondUrl !== firstUrl, 'different accounts received identical links');
    await second.goto(base + '/me/referrals');
    await second.getByLabel('Тип короткой ссылки').selectOption('promo');
    await second.getByLabel('Короткое имя ссылки').fill(aliasName);
    await second.getByRole('button', { name: 'Создать короткую ссылку' }).click();
    await second.waitForURL(base + '/me/referrals');
    const secondShort = await second.getByLabel('Ваша короткая ссылка').inputValue();
    assert(secondShort === base + '/promo/' + aliasName,
      'identical aliases must stay isolated by prefix');

    // A visible book can collect a passive first-touch ref. A draft must not.
    const bookContext = await browser.newContext({
      viewport: { width: 390, height: 844 },
    });
    const bookVisitor = await bookContext.newPage();
    const ownerCode = firstUrl.slice((base + '/r/').length);
    const otherCode = secondUrl.slice((base + '/r/').length);
    const hidden = await bookVisitor.goto(base + '/books/2?ref=' + ownerCode);
    assert(hidden && hidden.status() === 404, 'hidden book was exposed via ref');
    assert(!(await bookContext.cookies(base)).some(c => c.name === 'tor-dev-guest'),
      'hidden book must not capture a referral');
    const visible = await bookVisitor.goto(base + '/books/1?ref=' + ownerCode);
    assert(visible && visible.status() === 200, 'public book with ref unavailable');
    const bookCookie = (await bookContext.cookies(base))
      .find(c => c.name === 'tor-dev-guest');
    assert(bookCookie && bookCookie.sameSite === 'Lax',
      'book query referral did not preserve a bounded guest cookie');
    await bookVisitor.goto(base + '/books/1?ref=' + otherCode);
    const afterBookSecond = (await bookContext.cookies(base))
      .find(c => c.name === 'tor-dev-guest');
    assert(afterBookSecond && afterBookSecond.value === bookCookie.value,
      'second book query referral overwrote first-touch cookie');

    await visitor.goto(firstShort);
    assert(visitor.url() === base + '/register', 'first vanity alias did not reach signup');
    const firstCookie = (await visitorContext.cookies(base))
      .find(c => c.name === 'tor-dev-guest');
    assert(firstCookie && firstCookie.sameSite === 'Lax',
      'cross-site-compatible guest capture not persisted');
    await visitor.goto(secondShort);
    assert(visitor.url() === base + '/register', 'second vanity alias did not reach signup');
    const afterSecond = (await visitorContext.cookies(base))
      .find(c => c.name === 'tor-dev-guest');
    assert(afterSecond && afterSecond.value === firstCookie.value,
      'second referral overwrote first-touch visitor identity');

    await visitor.getByLabel('Отображаемое имя').fill('Приглашённый');
    await visitor.getByLabel('Электронная почта')
      .fill('referral.invited.' + unique + '@example.invalid');
    await visitor.getByLabel('Пароль (12–128 символов)')
      .fill('TestReferralPassword2026');
    await visitor.getByRole('button', { name: 'Создать учётную запись' }).click();
    await visitor.waitForURL(base + '/me');
    const afterRegistration = await visitorContext.cookies(base);
    assert(!afterRegistration.some(c => c.name === 'tor-dev-guest'),
      'consumed guest capture was not cleared after signup');

    let counted = false;
    for (let attempt = 0; attempt < 45; attempt++) {
      const report = await referralStats(first);
      if (report.active === 1) {
        counted = true;
        break;
      }
      await new Promise(resolve => setTimeout(resolve, 1000));
    }
    assert(counted, 'outbox consumer did not attribute the first inviter');
    assert((await referralStats(second)).active === 0,
      'later link took credit for another inviter');
    // Complete a second registration originating from ?ref on the book.
    await bookVisitor.goto(base + '/register');
    await bookVisitor.getByLabel('Отображаемое имя').fill('Читатель книги');
    await bookVisitor.getByLabel('Электронная почта')
      .fill('referral.book.' + unique + '@example.invalid');
    await bookVisitor.getByLabel('Пароль (12–128 символов)')
      .fill('TestReferralPassword2026');
    await bookVisitor.getByRole('button', { name: 'Создать учётную запись' }).click();
    await bookVisitor.waitForURL(base + '/me');
    let bookCounted = false;
    for (let attempt = 0; attempt < 45; attempt++) {
      if ((await referralStats(first)).active === 2) {
        bookCounted = true;
        break;
      }
      await new Promise(resolve => setTimeout(resolve, 1000));
    }
    assert(bookCounted, 'public-book query capture was not finalized after signup');
    assert((await referralStats(second)).active === 0,
      'later book query took first referral credit');

    const visitorReport = await referralStats(visitor);
    assert(visitorReport.invitation === null && visitorReport.active === 0,
      'invited reader can access inviter link or private statistics');
    const ownerPage = await first.goto(base + '/me/referrals');
    assert(ownerPage && ownerPage.status() === 200, 'owner dashboard missing');
    assert(await first.getByText('Активных привязок:').count() > 0,
      'owner does not see private attribution counter');

    await first.setViewportSize({ width: 390, height: 844 });
    await first.goto(base + '/me/referrals');
    const sizes = await first.evaluate(() => ({
      content: document.documentElement.scrollWidth,
      viewport: document.documentElement.clientWidth,
      body: document.body.scrollWidth,
    }));
    assert(sizes.content <= sizes.viewport + 1 && sizes.body <= sizes.viewport + 1,
      'referral dashboard overflows mobile viewport: ' + JSON.stringify(sizes));
    await first.screenshot({
      path: path.join(out, 'referrals-owner-mobile.png'), fullPage: true,
    });
    console.log('F-09/S052 Chromium direct, public-book ?ref, first-touch and dashboard: PASS');
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
