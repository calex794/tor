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
    const before = await referralStats(first);
    assert(before.active === 0 && before.invitation.url === firstUrl,
      'first invite missing from owner account');
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

    await visitor.goto(firstUrl);
    assert(visitor.url() === base + '/register', 'first invitation did not reach signup');
    const firstCookie = (await visitorContext.cookies(base))
      .find(c => c.name === 'tor-dev-guest');
    assert(firstCookie && firstCookie.sameSite === 'Lax',
      'cross-site-compatible guest capture not persisted');
    await visitor.goto(secondUrl);
    assert(visitor.url() === base + '/register', 'second invitation did not reach signup');
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
    console.log('F-09/S052 Chromium referral share, first touch and dashboard: PASS');
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
