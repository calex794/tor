'use strict';
// S042/S043: real isolated PostgreSQL -> Axum -> native forms -> Chromium.
const { chromium } = require('playwright');
const { spawn, execFileSync } = require('node:child_process');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const db = process.env.APP_URL || process.env.TEST_DATABASE_URL || '';
const parsed = new URL(db);
assert.equal(parsed.protocol, 'postgresql:'); assert.equal(parsed.hostname, '127.0.0.1');
assert.equal(parsed.port, '5432'); assert.equal(parsed.pathname, '/tor_ci');
const base = 'http://127.0.0.1:3116';
const shots = process.env.TOR_READER_SHOTS || '/tmp/paid-ads-shots';
fs.mkdirSync(shots, { recursive: true });
const log = fs.openSync(path.join(shots, 'paid-ads-server.log'), 'w');
const server = spawn('./target/debug/tor-web', [], { stdio: ['ignore', log, log], env: {
  ...process.env, DATABASE_URL: db, TOR_AUTH_ENABLED: '1', TOR_LISTEN_ADDR: '127.0.0.1:3116', TOR_PUBLIC_ORIGIN: base,
  TOR_ADS_DAILY_PURCHASE_ENABLED: '1', TOR_ADS_DAILY_SERVING_ENABLED: '1', TOR_CHAPTER_ADS_PURCHASE_ENABLED: '1',
  TOR_CHAPTER_ADS_PLANNER_ENABLED: '1', TOR_CHAPTER_ADS_SERVING_ENABLED: '1', TOR_CHAPTER_ADS_BILLABLE_ENABLED: '1',
  TOR_CHAPTER_ADS_SETTLEMENT_ENABLED: '1'
} });
function sql(query) {
  return execFileSync('psql', ['-XAtq', '-v', 'ON_ERROR_STOP=1', '--dbname', db, '-c', query], {
    encoding: 'utf8', env: { ...process.env, PGOPTIONS: '-c statement_timeout=5000 -c lock_timeout=2000' }
  }).trim();
}
function id(query) { const n = Number(sql(query)); assert.ok(Number.isSafeInteger(n) && n > 0); return n; }
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function poll(fn, attempts = 80) { let last; for (let i = 0; i < attempts; i++) { try { return await fn(); } catch (e) { last = e; await pause(150); } } throw last; }
async function register(browser, label) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 960 } });
  await context.addInitScript(() => {
    window.adCspViolations = [];
    document.addEventListener('securitypolicyviolation', e => window.adCspViolations.push(e.effectiveDirective));
  });
  const page = await context.newPage(); const email = `paid-ads-${label}-${Date.now()}@example.invalid`.toLowerCase();
  await page.goto(base + '/register'); await page.getByLabel('Отображаемое имя').fill(label);
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('PaidAdsBrowserPassword2026');
  await page.getByRole('button', { name: 'Создать учётную запись' }).click(); await page.waitForURL('**/me');
  return { context, page, user: id(`SELECT id FROM users WHERE email_normalized='${email}'`) };
}
async function fill(form, fields) {
  for (const [name, value] of Object.entries(fields)) {
    const input = form.locator(`[name="${name}"]`);
    if (await input.evaluate(el => el.tagName === 'SELECT')) await input.selectOption(String(value));
    else await input.fill(String(value));
  }
}
async function submit(page, form) {
  const action = await form.getAttribute('action');
  const response = page.waitForResponse(r => new URL(r.url()).pathname === action && r.request().method() === 'POST');
  await form.getByRole('button').click(); const result = await response;
  assert.equal(result.status(), 303, `${action}: unexpected response status`);
  await page.waitForLoadState('networkidle'); return result.request().postData();
}
function counts(campaign) { return sql(`SELECT reserved_units||'|'||reserved_minor||'|'||settled_minor||'|'||refunded_minor FROM finance_ad_funding WHERE product='chapter' AND campaign_id=${campaign}`).split('|').map(Number); }
async function csrf(page) { return await page.locator('[data-free-progress-csrf]').getAttribute('data-free-progress-csrf'); }
async function api(page, route, payload, extra = {}) {
  return page.request.post(base + route, { data: payload, headers: { Origin: base, 'X-CSRF-Token': await csrf(page), ...extra } });
}
(async () => {
  let browser;
  try {
    await poll(async () => assert.equal((await fetch(base + '/readyz')).status, 200));
    browser = await chromium.launch({ headless: true, ...(process.env.TOR_CHROMIUM_EXECUTABLE ? { executablePath: process.env.TOR_CHROMIUM_EXECUTABLE } : {}) });
    const admin = await register(browser, 'AdsAdmin');
    assert.equal((await admin.page.goto(base + '/admin/promotion/paid')).status(), 403);
    sql(`INSERT INTO account_roles(user_id,role,is_active) VALUES(${admin.user},'administrator',true)`);
    await admin.page.goto(base + '/admin/promotion/paid');
    let form = admin.page.locator('form[action="/admin/promotion/paid/daily-offer"]');
    await fill(form, { expected_revision: 0, slot: 'home_featured', rate_minor: 100, min_days: 1, max_days: 30, terms_version: 'ci-days-1', enabled: false, daily_cap:100, hourly_cap:100, session_cap:100, min_interval_seconds:0 });
    await submit(admin.page, form);
    const offer = id(`SELECT id FROM ad_daily_offers WHERE created_by=${admin.user} ORDER BY id DESC LIMIT 1`);
    assert.ok((await admin.page.locator('main').innerText()).includes('продажа: выключена'));
    const unavailable=await admin.page.request.get(base+'/api/v1/ads/daily-offers');assert.equal((await unavailable.json()).offers.length,0);
    form=admin.page.locator('form[action="/admin/promotion/paid/daily-offer"]');
    await fill(form,{id:offer,expected_revision:1,slot:'home_featured',rate_minor:100,min_days:1,max_days:30,terms_version:'ci-days-2',enabled:true,
      daily_cap:100,hourly_cap:100,session_cap:100,min_interval_seconds:0});await submit(admin.page,form);
    form = admin.page.locator('form[action="/admin/promotion/paid/chapter-package"]');
    await fill(form, { expected_revision: 0, price_minor: 101, target_impressions: 4, publisher_share_bps: 7000, hold_seconds: 0,
      daily_cap: 100, hourly_cap: 100, session_cap: 100, terms_version: 'ci-shows-1', enabled: true });
    await submit(admin.page, form);
    const pack = id(`SELECT id FROM ad_packages WHERE created_by=${admin.user} ORDER BY id DESC LIMIT 1`);
    for (const slot of ['home_featured', 'chapter_end_free']) {
      form = admin.page.locator('form[action="/admin/promotion/paid/slot-policy"]');
      await fill(form, { slot, expected_revision: 0, paid_first: true, daily_enabled: slot === 'home_featured', chapter_enabled: slot === 'chapter_end_free', max_cards: 4 });
      await submit(admin.page, form);
    }
    const buyer = await register(browser, 'AdsBuyer');
    const publisher = await register(browser, 'AdsPublisher');
    const viewer = await register(browser, 'AdsViewer');
    const stamp = Date.now();
    const book = id(`INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id) VALUES('ci:paid-ads:${stamp}:promoted','ru','Реклама <safe>','public','basic',${buyer.user}) RETURNING id`);
    const host = id(`INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id) VALUES('ci:paid-ads:${stamp}:host','ru','Книга издателя','public','basic',${publisher.user}) RETURNING id`);
    const chapter = id(`INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) VALUES(${host},1,'Открытая глава','public','free',0) RETURNING id`);
    const version = id(`INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) VALUES(${chapter},1,repeat('Текст открытой главы для проверки видимости рекламы. '||chr(10)||chr(10),180)) RETURNING id`);
    sql(`INSERT INTO chapter_publications(chapter_id,version_id) VALUES(${chapter},${version}); INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) VALUES(${buyer.user},10000,'RUB')`);
    await publisher.page.goto(base + '/me/ads');
    form = publisher.page.locator('form[action="/me/ads/publisher/settings"]'); await fill(form, { enabled: true }); await submit(publisher.page, form);
    await buyer.page.goto(base + `/me/ads?book=${book}`);
    form = buyer.page.locator('form[action="/me/ads/daily"]');
    await fill(form, { offer: `${offer}:2:home_featured`, days: 3 }); await form.locator('[name="accept_terms"]').check();
    const dailyPayload = await submit(buyer.page, form);
    const daily = id(`SELECT id FROM ad_legacy_campaigns WHERE advertiser_id=${buyer.user}`);
    const replay = await buyer.page.request.post(base + '/me/ads/daily', { data: dailyPayload, headers: { Origin: base, 'Content-Type': 'application/x-www-form-urlencoded' }, maxRedirects: 0 });
    assert.equal(replay.status(), 303);
    await buyer.page.goto(base + `/me/ads?book=${book}`);
    form = buyer.page.locator('form[action="/me/ads/chapter"]'); await fill(form, { package: `${pack}:1` });
    await form.locator('[name="accept_terms"]').check();
    let lostPayload='';let finished;
    const intercepted=new Promise(resolve=>{finished=resolve;});
    await buyer.page.route(base+'/me/ads/chapter',async route=>{
      lostPayload=route.request().postData();
      const committed=await route.fetch({maxRedirects:0});assert.equal(committed.status(),303);
      await route.abort('connectionfailed');finished();
    });
    await Promise.all([form.getByRole('button').click().catch(()=>{}),intercepted]);
    await buyer.page.unroute(base+'/me/ads/chapter');
    assert.ok(lostPayload);
    const recovered=await buyer.page.request.post(base+'/me/ads/chapter',{data:lostPayload,
      headers:{Origin:base,'Content-Type':'application/x-www-form-urlencoded'},maxRedirects:0});
    assert.equal(recovered.status(),303);await buyer.page.goto(base+'/me/ads');
    const campaign = id(`SELECT id FROM ad_chapter_campaigns WHERE advertiser_id=${buyer.user}`);
    assert.equal(sql(`SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=${buyer.user}`), '9599');
    await poll(() => assert.equal(sql(`SELECT count(*) FROM ad_live_assignments WHERE campaign_id=${campaign} AND host_book_id=${host}`), '1'));
    await viewer.page.goto(base + `/reader/${chapter}`);
    let card = viewer.page.locator(`[data-paid-campaign="${campaign}"]`);
    await card.waitFor(); const token = await card.getAttribute('data-paid-ad-token'); assert.match(token, /^[A-Za-z0-9_-]{43}$/);
    assert.deepEqual(counts(campaign).slice(0, 2), [0, 0]);
    const proof={campaign_id:String(campaign),generation_id:await card.getAttribute('data-paid-generation'),assignment_id:await card.getAttribute('data-paid-assignment'),viewability_token:token};
    assert.equal((await api(viewer.page, '/api/v1/ads/chapter/impressions', { ...proof, observed_visible_fraction: .59, observed_duration_ms: 1100 })).status(), 422);
    assert.equal((await api(viewer.page, '/api/v1/ads/chapter/impressions', { ...proof, observed_visible_fraction: .60, observed_duration_ms: 900 })).status(), 422);
    await viewer.page.evaluate(() => { document.body.style.paddingBottom = '1500px'; });
    const box = await card.boundingBox();
    await viewer.page.evaluate(y => window.scrollTo(0, y), box.y + box.height * .43);
    await pause(1300); assert.equal(counts(campaign)[0], 0, '57% visibility must not bill');
    await card.scrollIntoViewIfNeeded();
    await poll(() => assert.equal(counts(campaign)[0], 1));
    const accepted = await api(viewer.page, '/api/v1/ads/chapter/impressions', { ...proof, observed_visible_fraction: .60, observed_duration_ms: 1500 });
    assert.equal(accepted.status(), 200); assert.equal((await accepted.json()).replayed, true);
    const badOrigin = await api(viewer.page, '/api/v1/ads/chapter/impressions', { ...proof, observed_visible_fraction: .60, observed_duration_ms: 1500 }, { Origin: 'https://other.invalid' });
    assert.equal(badOrigin.status(), 403);
    await viewer.page.screenshot({ path: path.join(shots, 'paid-ads-reader-desktop.png'), fullPage: false });
    await viewer.page.reload(); card = viewer.page.locator(`[data-paid-campaign="${campaign}"]`); await card.scrollIntoViewIfNeeded();
    await poll(() => assert.equal(counts(campaign)[0], 2));
    await card.getByRole('link').first().click(); await viewer.page.waitForURL(base + `/books/${book}`);
    await poll(() => assert.equal(sql(`SELECT clicks||'|'||opens FROM ad_campaign_stats WHERE campaign_id=${campaign}`), '1|1'));
    assert.equal((await viewer.page.request.get(base + `/api/v1/ads/chapter/campaigns/${campaign}`)).status(), 404);
    await buyer.page.goto(base + '/me/ads');
    form = buyer.page.locator(`form[action="/me/ads/chapter/${campaign}/cancel"]`); await fill(form, { reason: 'Вернуть неиспользованные показы' }); await submit(buyer.page, form);
    await poll(() => assert.deepEqual(counts(campaign), [2, 50, 50, 51]));
    form = buyer.page.locator(`form[action="/me/ads/daily/${daily}/cancel"]`); await fill(form, { reason: 'Вернуть будущие полные дни' }); await submit(buyer.page, form);
    assert.equal(sql(`SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=${buyer.user}`), '9850');
    await publisher.page.goto(base + '/me/ads'); form = publisher.page.locator('form[action="/me/ads/publisher/credit"]');
    const transferPayload = await submit(publisher.page, form);
    assert.equal(sql(`SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=${publisher.user}`), '35');
    const transferReplay = await publisher.page.request.post(base + '/me/ads/publisher/credit', { data: transferPayload, headers: { Origin: base, 'Content-Type': 'application/x-www-form-urlencoded' }, maxRedirects: 0 });
    assert.equal(transferReplay.status(), 303); assert.equal(sql(`SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=${publisher.user}`), '35');
    for (const actor of [buyer, publisher]) {
      await actor.page.setViewportSize({ width: 390, height: 844 }); await actor.page.goto(base + '/me/ads');
      assert.ok(await actor.page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
      assert.deepEqual(await actor.page.evaluate(() => window.adCspViolations), []);
      await actor.page.screenshot({ path: path.join(shots, `paid-ads-${actor === buyer ? 'buyer' : 'publisher'}-mobile.png`), fullPage: true });
    }
    const conservation = sql(`SELECT budget_minor||'|'||settled_minor||'|'||publisher_settled_minor||'|'||refunded_minor FROM finance_ad_funding WHERE product='chapter' AND campaign_id=${campaign}`);
    assert.equal(conservation, '101|50|35|51');
    console.log('PAID_ADS_PG_CHROMIUM_OK native_daily_and_chapter viewability_57_60 lost_response_replay origin owner settlement refund publisher_wallet desktop_mobile');
  } finally {
    if (browser) await browser.close(); server.kill('SIGTERM'); fs.closeSync(log);
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
