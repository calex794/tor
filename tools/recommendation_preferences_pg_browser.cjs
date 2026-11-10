'use strict';
// P2a: explicit hide/restore survives retirement of individual REC/FLOW collection.
// This script exercises real HTML forms, A sessions, F storage and PostgreSQL.
const {chromium} = require('playwright');
const {execFileSync} = require('node:child_process');
const {createHash, randomBytes} = require('node:crypto');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const db = process.env.APP_URL || '';
const base = process.env.TOR_REC_BASE || 'http://127.0.0.1:3108';
const shots = process.env.TOR_REC_SHOTS || '/tmp/rec-preferences-shots';
const parsed = new URL(db);
if (parsed.protocol !== 'postgresql:' || parsed.hostname !== '127.0.0.1'
    || parsed.port !== '5432' || parsed.pathname !== '/tor_ci'
    || !/^http:\/\/127\.0\.0\.1:\d+$/.test(base)) {
  throw new Error('Preference acceptance requires disposable loopback tor_ci');
}
const proofImages = process.env.TOR_PREFERENCES_PROOF_IMAGES === '1';
if (proofImages && process.env.GITHUB_ACTIONS !== 'true') {
  throw new Error('Inline fixture previews are a CI-only evidence option');
}
fs.mkdirSync(shots, {recursive:true});
const ordinaryUA = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36';
const {legacyScope,optionalSnapshot,assertNoControls,retiredConsent} =
  require('./collection_retirement_fixtures.cjs')(sql);
const feedbackForm = "form[action='/me/recommendations/feedback']";

function sql(statement) {
  return execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1',
    '--dbname',db,'--command',statement], {
    encoding:'utf8',
    env:{...process.env, PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'},
  }).trim();
}
function scalar(statement) {
  const value = sql(statement);
  assert.match(value, /^\d+$/);
  return Number(value);
}
function noSubject(user) {
  assert.equal(scalar(`SELECT count(*) FROM rec_subjects WHERE user_id=${user}`),0,
    'a hide/restore preference must not create an analytics subject');
}
function preference(user, book) {
  return JSON.parse(sql(`SELECT json_build_object('hidden',hidden,'revision',revision,
    'expires_at',expires_at::text) FROM rec_book_preferences
    WHERE user_id=${user} AND book_id=${book}`));
}
function command(user, key) {
  const digest=createHash('sha256').update(key).digest('hex');
  return JSON.parse(sql(`SELECT json_build_object('key',encode(command_key,'hex'),
    'hash',encode(payload_hash,'hex'),'revision',result_revision,
    'kind',command_type,'created_at',created_at::text)
    FROM rec_user_preference_commands
    WHERE user_id=${user} AND command_key=decode('${digest}','hex')`));
}
async function privateResponse(response, status=200) {
  assert.equal(response.status(),status);
  const headers=await response.allHeaders();
  assert.equal(headers['cache-control'],'no-store');
  assert.match(headers['x-robots-tag'],/noindex/);
  assert.match(headers['x-robots-tag'],/nofollow/);
}
async function register(page,email) {
  await page.goto(base+'/register');
  await page.getByLabel('Отображаемое имя').fill('Выбор книг');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('PreferenceBrowserPassword2026');
  await page.getByRole('button',{name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
  return scalar(`SELECT id FROM users WHERE email_normalized='${email}'`);
}
async function login(page,email) {
  await page.goto(base+'/login');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль',{exact:true}).fill('PreferenceBrowserPassword2026');
  await page.getByRole('button',{name:'Войти',exact:true}).click();
  await page.waitForURL('**/me');
}
function card(page,book) {
  return page.locator('[data-rec-card]').filter({
    has:page.locator(`a[data-rec-link][href='/books/${book}']`),
  });
}
function clearForm(page,book) {
  return page.locator(feedbackForm).filter({
    has:page.locator(`input[name='book_id'][value='${book}']`),
  }).filter({has:page.locator("input[name='choice'][value='clear']")});
}
async function submit(page,form,endpoint) {
  const [response]=await Promise.all([
    page.waitForResponse(response => response.url()===base+endpoint
      && response.request().method()==='POST'),
    form.getByRole('button').click(),
  ]);
  await privateResponse(response,303);
  await page.waitForURL('**/me/recommendations');
}
async function checkStorage(context,page) {
  for (const cookie of await context.cookies()) {
    assert.ok(['tor-dev-session','tor-dev-csrf'].includes(cookie.name),
      'unexpected browser cookie '+cookie.name);
  }
  const storage=await context.storageState();
  assert.equal(storage.origins.reduce((n,origin)=>n+origin.localStorage.length,0),0);
  assert.deepEqual(await page.evaluate(()=>Object.keys(sessionStorage)),[]);
  assert.deepEqual(await page.evaluate(async()=>await indexedDB.databases()),[]);
}
async function evidence(page,width) {
  await page.setViewportSize({width,height:900});
  await privateResponse(await page.goto(base+'/me/recommendations'));
  await assertNoControls(page);
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
  const name='recommendation-preferences-'+width;
  await page.screenshot({path:path.join(shots,name+'.png'),fullPage:true,animations:'disabled'});
  if (!proofImages) return;
  // The actual JPEG viewport is captured directly by Chromium, not generated.
  // Keep the original full-page PNG in the normal artifact.
  await page.getByRole('heading',{name:'Скрытые книги',exact:true}).scrollIntoViewIfNeeded();
  let bytes,quality;
  for (quality of [55,45,35,25]) {
    bytes=await page.screenshot({type:'jpeg',quality,fullPage:false,animations:'disabled'});
    if (bytes.length<=96*1024) break;
  }
  assert.ok(bytes.length<=96*1024,'fixture preview exceeds 96 KiB');
  const head=execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim();
  assert.match(head,/^[0-9a-f]{40}$/);
  const encoded=bytes.toString('base64');
  const chunks=[];
  for (let offset=0;offset<encoded.length;offset+=8192) chunks.push(encoded.slice(offset,offset+8192));
  console.log('TOR_PREF_IMAGE_META '+JSON.stringify({
    name,head,mime:'image/jpeg',width,height:900,quality,bytes:bytes.length,
    sha256:createHash('sha256').update(bytes).digest('hex'),chunks:chunks.length,
  }));
  chunks.forEach((chunk,index)=>console.log(`TOR_PREF_IMAGE_CHUNK ${name} ${index+1}/${chunks.length} ${chunk}`));
  console.log('TOR_PREF_IMAGE_END '+name);
}

(async()=>{
  const run=Date.now()+'-'+randomBytes(4).toString('hex');
  const email=`preferences-browser-${run}@tor-ci.example`;
  const users=[],books=[];
  const previousHold=sql('SELECT hold::text FROM rec_runtime_policy WHERE singleton');
  assert.match(previousHold,/^(true|false)$/);
  let browser;
  try {
    // Select the normal contextual fallback, independent of previous test generations.
    sql('UPDATE rec_runtime_policy SET hold=true WHERE singleton');
    const book=scalar(`INSERT INTO books(source_identity,target_language,title,visibility,access_level)
      VALUES('ci:preferences:browser:${run}','ru','Книга для личного выбора ${run}',
      'public','basic') RETURNING id`);
    books.push(book);
    browser=await chromium.launch({headless:true});
    const context=await browser.newContext({viewport:{width:1440,height:900},userAgent:ordinaryUA});
    const page=await context.newPage();
    const errors=[],unexpectedAnalytics=[];
    page.on('pageerror',error=>errors.push(error.message));
    page.on('request',request=>{
      if (/\/api\/v1\/(?:recommendations\/(?:impressions|activate)|discovery\/activate)$/.test(new URL(request.url()).pathname)) {
        unexpectedAnalytics.push(new URL(request.url()).pathname);
      }
    });
    const user=await register(page,email);
    users.push(user);
    noSubject(user);
    await privateResponse(await page.goto(base+'/me/recommendations'));
    await assertNoControls(page);
    const visible=card(page,book);
    assert.equal(await visible.count(),1);
    assert.equal(await visible.getAttribute('data-rec-ticket'),null);
    const form=visible.locator(feedbackForm);
    assert.equal(await form.locator("input[name='expected_revision']").inputValue(),'0');
    const hideKey=await form.locator("input[name='command_key']").inputValue();
    await submit(page,form,'/me/recommendations/feedback');
    noSubject(user);
    assert.equal(await clearForm(page,book).count(),1);
    assert.equal(await card(page,book).count(),0);
    assert.match(await page.locator('body').innerText(),/он не включает учёт посещений и чтения/);
    const saved=preference(user,book),savedCommand=command(user,hideKey);
    assert.equal(saved.hidden,true);
    assert.equal(saved.revision,1);
    const epoch=sql(`SELECT feedback_epoch FROM rec_preference_accounts WHERE user_id=${user}`);
    assert.equal(epoch,'0');
    await privateResponse(await page.goto(base+'/me/recommendations'));
    assert.deepEqual(preference(user,book),saved,'GET must not extend preference expiry');
    noSubject(user);
    await checkStorage(context,page);
    assert.deepEqual(unexpectedAnalytics,[]);

    // Explicit old-installation fixture, outside every browser command.
    // Cached clients cannot reactivate analytics or erase the independent choice.
    legacyScope(user);
    const scope=optionalSnapshot(user);
    for (const endpoint of ['/me/recommendations/consent','/me/recommendations/flow-consent']) {
      await retiredConsent(page,endpoint,false);
      await retiredConsent(page,endpoint,true);
    }
    assert.deepEqual(optionalSnapshot(user),scope);
    assert.deepEqual(preference(user,book),saved);
    assert.deepEqual(command(user,hideKey),savedCommand);
    assert.equal(sql(`SELECT feedback_epoch FROM rec_preference_accounts WHERE user_id=${user}`),epoch);
    await privateResponse(await page.goto(base+'/me/recommendations'));
    await assertNoControls(page);
    assert.equal(await clearForm(page,book).count(),1);
    assert.equal(await card(page,book).count(),0);
    await checkStorage(context,page);
    assert.deepEqual(unexpectedAnalytics,[]);
    for (const width of [390,1440]) await evidence(page,width);

    // Native form restore must also work without JavaScript after a fresh login.
    const noJs=await browser.newContext({
      viewport:{width:390,height:900},userAgent:ordinaryUA,javaScriptEnabled:false,
    });
    const plain=await noJs.newPage();
    await login(plain,email);
    await privateResponse(await plain.goto(base+'/me/recommendations'));
    assert.equal(await clearForm(plain,book).count(),1);
    await submit(plain,clearForm(plain,book),'/me/recommendations/feedback');
    const cleared=preference(user,book);
    assert.equal(cleared.hidden,false);
    assert.equal(cleared.revision,2);
    assert.equal(await clearForm(plain,book).count(),0);
    assert.equal(await card(plain,book).count(),1);
    assert.equal(await card(plain,book).getAttribute('data-rec-ticket'),null);
    assert.deepEqual(optionalSnapshot(user),scope);
    assert.deepEqual(command(user,hideKey),savedCommand);
    assert.deepEqual(unexpectedAnalytics,[]);
    assert.deepEqual(errors,[]);
    console.log('REC preferences browser PASS: fresh actor without analytics subject, real hide/restore, no tracking requests or new storage, retired clients preserve exact legacy scope and preference TTL/revision/receipt, no-JS restore, 390/1440');
  } finally {
    if (browser) await browser.close();
    sql(`UPDATE rec_runtime_policy SET hold=${previousHold} WHERE singleton`);
    if (users.length) sql(`DELETE FROM rec_subjects WHERE user_id IN (${users.join(',')})`);
    if (books.length) sql(`UPDATE books SET admin_hold=true WHERE id IN (${books.join(',')})`);
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
