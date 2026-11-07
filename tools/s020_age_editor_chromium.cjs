// S020/#486: actual clickable, server-owned Book age editor on disposable tor_ci.
// No production activation; the Rust fixture sets the feature flag only for its own router.
const assert = require('node:assert/strict');
const {execFileSync} = require('node:child_process');
const {chromium} = require('playwright');

const base = 'http://127.0.0.1:3098';
const book = Number(process.env.S020_BROWSER_BOOK_ID);
const free = Number(process.env.S020_BROWSER_FREE_ID);
const readerId = Number(process.env.S020_BROWSER_READER_ID);
const ownerSession = process.env.S020_BROWSER_OWNER_SESSION;
const readerSession = process.env.S020_BROWSER_READER_SESSION;
const csrf = process.env.S020_BROWSER_CSRF;
const url = process.env.TEST_DATABASE_URL || '';
assert.ok(Number.isSafeInteger(book) && book > 0);
assert.ok(Number.isSafeInteger(free) && free > 0);
assert.ok(Number.isSafeInteger(readerId) && readerId > 0);
assert.ok(ownerSession && readerSession && csrf);
assert.ok(url.startsWith('postgresql://tor_app:') && url.includes('@127.0.0.1:5432/') && url.endsWith('/tor_ci'));
const settings = '/me/books/' + book + '/settings/access-level';
const bookUrl = '/books/' + book;
const readerUrl = '/reader/' + free;
const marker = 'S020-CHROMIUM-FREE-TEXT-' + book;
// Filter using the real user-facing catalogue; the link itself is the verdict,
// not an incidental book title or a broad shared result count.
const catalogQuery = 'S020 Owner Age UI';
const catalogPath = '/catalog?' + new URLSearchParams({q:catalogQuery}).toString();
const catalogLink = 'a[href="' + bookUrl + '"]';

function pg(sql) {
  return execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1','--dbname',url,'--command',sql], {encoding:'utf8'}).trim();
}
async function newSession(browser, token, width) {
  const context = await browser.newContext({viewport:{width,height:880}});
  await context.addCookies([
    {name:'tor-dev-session',value:token,url:base},
    {name:'tor-dev-csrf',value:csrf,url:base},
  ]);
  return {context,page:await context.newPage()};
}
async function status(page, path, expected) {
  const res = await page.goto(base + path);
  assert.equal(res.status(), expected, 'wrong access on ' + path);
  return await page.content();
}
async function assertCatalogVisibility(page, visible, description) {
  const res = await page.goto(base + catalogPath, {waitUntil:'load'});
  assert.ok(res && res.status() === 200, description + ': catalogue response failed');
  assert.equal((await page.locator(catalogLink).count()) > 0, visible,
    description + ': wrong Book visibility in authenticated catalogue');
}
async function submitTier(page, expected, next) {
  await page.goto(base + settings);
  assert.equal(await page.locator('input[name=expected_level]').inputValue(), expected);
  await page.locator('select[name=requested_level]').selectOption(next);
  const response = page.waitForResponse(r=>r.url() === base + settings && r.request().method() === 'POST');
  await page.getByRole('button',{name:'Повысить уровень'}).click();
  assert.equal((await response).status(), 303);
  await page.waitForLoadState('networkidle');
  assert.equal(await page.locator('input[name=expected_level]').inputValue(), next);
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1), 'age form must fit viewport');
}
(async()=>{
  const browser = await chromium.launch({headless:true});
  try {
    const owner = await newSession(browser,ownerSession,1440);
    const reader = await newSession(browser,readerSession,390);
    const guest = await browser.newContext({viewport:{width:390,height:880}});
    const guestPage = await guest.newPage();
    owner.page.setDefaultTimeout(15000);
    await status(guestPage,bookUrl,200);
    assert.ok((await status(guestPage,readerUrl,200)).includes(marker));
    // Search through the actual catalogue form before changing the Book.
    await guestPage.goto(base + '/catalog');
    await guestPage.getByLabel('Название или автор').fill(catalogQuery);
    await guestPage.getByRole('button',{name:'Найти'}).click();
    await guestPage.waitForURL(u => new URL(u).pathname === '/catalog'
      && new URL(u).searchParams.get('q') === catalogQuery);
    assert.ok((await guestPage.locator(catalogLink).count()) > 0,
      'guest must discover the basic Book through the search UI');
    await owner.page.goto(base + '/me/books/' + book + '/settings');
    await owner.page.getByRole('link',{name:'Возрастной уровень книги'}).click();
    assert.equal(new URL(owner.page.url()).pathname,settings);
    await submitTier(owner.page,'basic','adult');
    await assertCatalogVisibility(guestPage,false,'guest after 18+ upgrade');
    await assertCatalogVisibility(reader.page,true,'18+ reader after upgrade');
    await status(guestPage,bookUrl,404);
    assert.ok(!(await status(guestPage,readerUrl,404)).includes(marker));
    assert.ok((await status(reader.page,readerUrl,200)).includes(marker));
    await owner.page.setViewportSize({width:390,height:880});
    await owner.page.goto(base + settings);
    assert.equal(await owner.page.locator('select[name=requested_level] option[value=basic]').count(),0);
    await submitTier(owner.page,'adult','pro_adult');
    await assertCatalogVisibility(reader.page,false,'18+ reader after 21+ upgrade');
    await status(reader.page,bookUrl,404);
    assert.ok(!(await status(reader.page,readerUrl,404)).includes(marker));
    pg("INSERT INTO user_level_grants(user_id,level,source,is_active) VALUES(" + readerId + ",'pro_adult','moderation',TRUE)");
    await assertCatalogVisibility(reader.page,true,'21+ reader with verified grant');
    await status(reader.page,bookUrl,200);
    assert.ok((await status(reader.page,readerUrl,200)).includes(marker));
    pg("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=" + readerId + " AND level='pro_adult'");
    await assertCatalogVisibility(reader.page,false,'21+ reader after grant revocation');
    await status(reader.page,bookUrl,404);
    assert.ok(!(await status(reader.page,readerUrl,404)).includes(marker));
    await owner.context.close();
    await reader.context.close();
    await guest.close();
    console.log('S020 owner age editor Chromium PASS: desktop, mobile, grant, revoke, catalogue');
  } finally {
    await browser.close();
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
