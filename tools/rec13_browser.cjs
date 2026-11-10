'use strict';
// Native recommendations, real C progress and independent choices after runtime retirement.
const {chromium} = require('playwright');
const {legacyScope,optionalSnapshot,assertNoControls,retiredConsent} = require('./collection_retirement_fixtures.cjs')(sql);
const preview = require('./ci_browser_preview.cjs');
const {benchmarkOrganicActivation} = require('./rec13_organic_load.cjs');
const {execFileSync} = require('node:child_process');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const db = process.env.APP_URL || '';
const base = process.env.TOR_REC_BASE || 'http://127.0.0.1:3108';
const shots = process.env.TOR_REC_SHOTS || '/tmp/rec13-shots';
const database = new URL(db);
if (database.protocol!=='postgresql:' || database.hostname!=='127.0.0.1'
    || database.port!=='5432' || database.pathname!=='/tor_ci'
    || !/^http:\/\/127\.0\.0\.1:\d+$/.test(base)) throw new Error('isolated loopback tor_ci only');
assert.equal(process.env.GITHUB_ACTIONS,'true','REC13 compatibility load is isolated CI only');
fs.mkdirSync(shots, {recursive:true});
function sql(statement) {
  return execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1','--dbname',db,'--command',statement], {encoding:'utf8'}).trim();
}
function scalar(statement) {
  const result = sql(statement).split('\n').find(line => /^\d+$/.test(line));
  if (result === undefined) throw new Error('expected numeric SQL result');
  return Number(result);
}
async function register(page, email) {
  await page.goto(base+'/register');
  await page.getByLabel('Отображаемое имя').fill('Проверка рекомендаций');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('BrowserRec13SafePassword2026');
  await page.getByRole('button',{name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
}
async function completeFreeChapter(page, chapter, version) {
  // Functional progress belongs to the AJAX reader; the manual /read route
  // intentionally keeps connect-src closed and submits ordinary POST forms.
  assert.equal((await page.goto(base+`/reader/${chapter}`)).status(),200);
  await page.waitForFunction(() =>
    document.querySelector('[data-free-progress-chapter]')?.dataset.freeProgressReady==='1');
  const progress=await page.evaluate(async ({chapter,version})=>{
    const csrf=document.querySelector('[data-free-progress-chapter]')?.dataset.freeProgressCsrf;
    async function save(offset,revision) {
      const response=await fetch('/api/v1/me/progress/free',{method:'PUT',credentials:'same-origin',headers:{'Content-Type':'application/json','X-CSRF-Token':csrf},body:JSON.stringify({chapter_id:String(chapter),version_id:String(version),block_id:1,char_offset:offset,expected_revision:revision})});
      if (!response.ok) throw new Error('validated progress rejected: '+response.status);
      return response.json();
    }
    const first=await save(3,0);
    await save(13,first.revision);
    return true;
  },{chapter,version});
  assert.equal(progress, true);
}
(async()=>{
  const run = Date.now().toString();
  const email = 'rec13-browser-'+run+'@tor-ci.example';
  const mobileEmail = 'rec13-mobile-'+run+'@tor-ci.example';
  const books=[];
  let browser;
  try {
    for (let n=0;n<3;n++) {
      const book=scalar(`INSERT INTO books(source_identity,target_language,title,visibility,access_level)
        VALUES('ci:rec13:browser:${run}:${n}','ru','Browser recommendation ${n}','public','basic') RETURNING id`);
      books.push(book);
      sql(`INSERT INTO book_recommendation_audiences(book_id,audience) VALUES(${book},'general')`);
    }
    const chapters=books.map((id,n)=>{
      const chapter=scalar(`INSERT INTO chapters(book_id,chapter_no,title,visibility)
        VALUES(${id},1,'Настоящее чтение ${n}','public') RETURNING id`);
      const version=scalar(`INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
        VALUES(${chapter},1,'Readable text') RETURNING id`);
      sql(`INSERT INTO chapter_publications(chapter_id,version_id) VALUES(${chapter},${version})`);
      return {chapter,version};
    });
    const book=books[2],{chapter,version}=chapters[2];
    browser=await chromium.launch({headless:true});
    const desktop=await browser.newContext({viewport:{width:1440,height:900}});
    const page=await desktop.newPage();
    const errors=[],unexpectedAnalytics=[];
    let explicitLegacyLoad=false;
    page.on('pageerror',error=>errors.push(error.message));
    page.on('request',request=>{
      if (!explicitLegacyLoad && /\/api\/v1\/(?:recommendations\/(?:impressions|activate)|discovery\/activate)$/.test(new URL(request.url()).pathname)) {
        unexpectedAnalytics.push(new URL(request.url()).pathname);
      }
    });
    await register(page,email);
    const user=scalar(`SELECT id FROM users WHERE email_normalized='${email}'`);
    await assertNoControls(page);
    assert.equal(scalar(`SELECT count(*) FROM rec_subjects WHERE user_id=${user}`),0);
    assert.equal(scalar(`SELECT count(*) FROM rec_public_books WHERE id IN(${books.join(',')})`),books.length);
    sql(`UPDATE chapters SET title='Исправленное название главы' WHERE id=${chapter}`);
    assert.equal(scalar(`SELECT count(*) FROM rec_public_books WHERE id IN(${books.join(',')})`),books.length);
    await page.goto(base+'/me/recommendations');
    await assertNoControls(page);
    await retiredConsent(page,'/me/recommendations/consent',true);
    assert.equal(scalar(`SELECT count(*) FROM rec_subjects WHERE user_id=${user}`),0);

    // Explicit old-installation fixture. Every subsequent native route must
    // ignore its enabled flag while preserving the exact stored legacy state.
    legacyScope(user);
    let before=optionalSnapshot(user);
    for (const [index,target] of [[0,'/catalog?q=Browser%20recommendation%200'],[1,'/catalog?sort=newest']]) {
      assert.equal((await page.goto(base+target)).status(),200);
      await assertNoControls(page);
      const link=page.locator(`a.book-mini-card__link[href='/books/${books[index]}']`).first();
      assert.ok(await link.isVisible());
      await link.click();
      await page.waitForURL(base+`/books/${books[index]}`);
      await page.goto(base+`/read/${chapters[index].chapter}`);
      await completeFreeChapter(page,chapters[index].chapter,chapters[index].version);
      assert.deepEqual(optionalSnapshot(user),before);
    }

    // The retained 150-request/25rps gate now proves private410 and zero writes
    // for genuine cached tokens. Its fixture rows are explicitly labeled.
    explicitLegacyLoad=true;
    const organicLoadStarted=process.hrtime.bigint();
    try {
      await benchmarkOrganicActivation({page,sql,user,book:books[0],base,db});
    } finally {
      explicitLegacyLoad=false;
      const elapsedSeconds=Number(process.hrtime.bigint()-organicLoadStarted)/1e9;
      console.log(`CI_REC13_STEP_SECONDS stage=organic_post_load seconds=${elapsedSeconds.toFixed(3)}`);
    }
    before=optionalSnapshot(user);

    const disabledJS=await browser.newContext({javaScriptEnabled:false,viewport:{width:1440,height:900}});
    await disabledJS.addCookies(await desktop.cookies());
    const plain=await disabledJS.newPage();
    await plain.goto(base+'/');
    await assertNoControls(plain);
    const nativeRec=plain.locator(`a[data-rec-link][href='/books/${book}']`).first();
    assert.ok(await nativeRec.count());
    await nativeRec.click();
    await plain.waitForURL(base+`/books/${book}`);
    await plain.goto(base+'/catalog?sort=newest');
    const nativeOrganic=plain.locator(`a.book-mini-card__link[href='/books/${books[0]}']`).first();
    assert.ok(await nativeOrganic.count());
    await nativeOrganic.click();
    await plain.waitForURL(base+`/books/${books[0]}`);
    assert.deepEqual(optionalSnapshot(user),before);
    await disabledJS.close();

    for (const target of ['/', '/catalog', `/books/${book}`, '/me/library', `/read/${chapter}`]) {
      assert.equal((await page.goto(base+target)).status(),200,target);
      assert.ok(await page.locator('[data-rec-panel]').count(),'panel absent on '+target);
      await assertNoControls(page);
    }
    await page.goto(base+'/');
    const card=page.locator('[data-rec-card]').filter({has:page.locator(`a[href='/books/${book}']`)}).first();
    await card.scrollIntoViewIfNeeded();
    assert.equal(await card.getAttribute('data-rec-ticket'),null);
    await page.screenshot({path:path.join(shots,'rec13-desktop.png'),fullPage:true});
    await preview(page,'rec13-desktop','[data-rec-panel]');
    await card.locator('a[data-rec-link]').click();
    await page.waitForURL(base+`/books/${book}`);
    await page.goto(base+`/read/${chapter}`);
    await completeFreeChapter(page,chapter,version);
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_starts WHERE user_id=${user}`),3);
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_completions WHERE user_id=${user}`),3);
    assert.deepEqual(optionalSnapshot(user),before);

    await page.goto(base+'/');
    const hiding=page.locator('[data-rec-card]').filter({has:page.locator(`a[href='/books/${book}']`)}).first();
    await hiding.getByRole('button',{name:'Не интересно'}).click();
    await page.waitForURL('**/me/recommendations');
    await assertNoControls(page);
    const saved=sql(`SELECT row_to_json(p) FROM rec_book_preferences p WHERE user_id=${user} AND book_id=${book}`);
    await retiredConsent(page,'/me/recommendations/consent',false);
    await retiredConsent(page,'/me/recommendations/flow-consent',true);
    assert.equal(sql(`SELECT row_to_json(p) FROM rec_book_preferences p WHERE user_id=${user} AND book_id=${book}`),saved);
    await page.goto(base+'/');
    assert.equal(await page.locator(`a[data-rec-link][href='/books/${book}']`).count(),0);

    const mobile=await browser.newContext({viewport:{width:390,height:844}});
    const mobilePage=await mobile.newPage();
    mobilePage.on('pageerror',error=>errors.push(error.message));
    await register(mobilePage,mobileEmail);
    const mobileUser=scalar(`SELECT id FROM users WHERE email_normalized='${mobileEmail}'`);
    const mobileBefore=optionalSnapshot(mobileUser);
    await assertNoControls(mobilePage);
    await mobilePage.goto(base+'/');
    assert.ok(await mobilePage.locator(`a[data-rec-link][href='/books/${book}']`).count(),
      'explicit choice leaked to another actor');
    await mobilePage.locator('[data-rec-panel]').scrollIntoViewIfNeeded();
    assert.ok(await mobilePage.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
    await mobilePage.screenshot({path:path.join(shots,'rec13-mobile.png'),fullPage:true});
    await preview(mobilePage,'rec13-mobile','[data-rec-panel]');
    await mobilePage.goto(base+'/catalog?q=Browser%20recommendation%201');
    await assertNoControls(mobilePage);
    await mobilePage.locator(`a.book-mini-card__link[href='/books/${books[1]}']`).first().click();
    await mobilePage.waitForURL(base+`/books/${books[1]}`);
    await mobilePage.goto(base+`/read/${chapters[1].chapter}`);
    await completeFreeChapter(mobilePage,chapters[1].chapter,chapters[1].version);
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_completions WHERE user_id=${mobileUser}`),1);
    assert.deepEqual(optionalSnapshot(mobileUser),mobileBefore);
    await mobile.close();

    await page.goto(base+'/me/recommendations');
    const restoring=page.locator('article').filter({has:page.locator(`a[href='/books/${book}']`)}).first();
    await restoring.getByRole('button',{name:'Вернуть в рекомендации'}).click();
    await page.waitForURL('**/me/recommendations');
    assert.deepEqual(optionalSnapshot(user),before);
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_completions WHERE user_id=${user}`),3);
    assert.deepEqual(unexpectedAnalytics,[]);
    assert.deepEqual(errors,[]);
    console.log('REC13 browser PASS: native contextual links on five surfaces, search/catalog and mobile real C completions, legacy scope inert, 150 private410 cached commands/zero writes, explicit hide/restore, no-JS links');
  } finally {
    if (browser) await browser.close();
    // Fixtures live only in tor_ci; preserve unrelated data and immutable events.
    if (books.length) sql(`DELETE FROM books WHERE id IN(${books.join(',')})`);
    sql(`DELETE FROM users WHERE email_normalized IN('${email}','${mobileEmail}')`);
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
