'use strict';
// Real browser-generated visibility, activation, C progress and explicit feedback.
const {chromium} = require('playwright');
const {execFileSync} = require('node:child_process');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const db = process.env.APP_URL || '';
const base = process.env.TOR_REC_BASE || 'http://127.0.0.1:3108';
const shots = process.env.TOR_REC_SHOTS || '/tmp/rec13-shots';
if (!db.endsWith('/tor_ci') || !/^http:\/\/127\.0\.0\.1:\d+$/.test(base)) throw new Error('isolated loopback tor_ci only');
fs.mkdirSync(shots, {recursive:true});
function sql(statement) {
  return execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1','--dbname',db,'--command',statement], {encoding:'utf8'}).trim();
}
function scalar(statement) {
  const result = sql(statement).split('\n').find(line => /^\d+$/.test(line));
  if (result === undefined) throw new Error('expected numeric SQL result');
  return Number(result);
}
async function waitFor(check, label) {
  for (let n=0; n<50; n++) {
    if (await check()) return;
    await new Promise(resolve=>setTimeout(resolve,200));
  }
  throw new Error('condition not observed: '+label);
}
async function register(page, email) {
  await page.goto(base+'/register');
  await page.getByLabel('Отображаемое имя').fill('Проверка рекомендаций');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('BrowserRec13SafePassword2026');
  await page.getByRole('button',{name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
}
(async()=>{
  const run = Date.now().toString();
  const email = 'rec13-browser-'+run+'@tor-ci.example';
  const mobileEmail = 'rec13-mobile-'+run+'@tor-ci.example';
  const policy = JSON.parse(sql('SELECT row_to_json(p) FROM rec_runtime_policy p WHERE singleton'));
  const books=[];
  let browser;
  try {
    // A deterministic fallback fixture; restore the exact prior policy afterward.
    sql('UPDATE rec_runtime_policy SET hold=true WHERE singleton');
    for (let n=0;n<3;n++) {
      const book=scalar(`INSERT INTO books(source_identity,target_language,title,visibility) VALUES('ci:rec13:browser:${run}:${n}','ru','Browser recommendation ${n}','public') RETURNING id`);
      books.push(book);
      sql(`INSERT INTO book_recommendation_audiences(book_id,audience) VALUES(${book},'general')`);
    }
    const book=books[2];
    const chapter=scalar(`INSERT INTO chapters(book_id,chapter_no,title,visibility) VALUES(${book},1,'Первое чтение','public') RETURNING id`);
    const version=scalar(`INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) VALUES(${chapter},1,'Readable text') RETURNING id`);
    sql(`INSERT INTO chapter_publications(chapter_id,version_id) VALUES(${chapter},${version})`);
    browser=await chromium.launch({headless:true});
    const desktop=await browser.newContext({viewport:{width:1440,height:900}});
    const page=await desktop.newPage();
    const errors=[];
    page.on('pageerror',error=>errors.push(error.message));
    await register(page,email);
    const user=scalar(`SELECT id FROM users WHERE email_normalized='${email}'`);
    await page.goto(base+'/me/recommendations');
    const checkbox=page.getByRole('checkbox');
    assert.equal(await checkbox.isChecked(),false);
    await checkbox.check();
    await page.getByRole('button',{name:'Сохранить выбор'}).click();
    await page.waitForURL('**/me/recommendations');
    assert.equal(sql(`SELECT enabled FROM rec_subjects WHERE user_id=${user}`),'t');
    // No-JS cards have canonical navigation but must not fabricate visible views.
    const disabledJS=await browser.newContext({javaScriptEnabled:false,viewport:{width:1440,height:900}});
    await disabledJS.addCookies(await desktop.cookies());
    const plain=await disabledJS.newPage();
    const viewsBefore=scalar(`SELECT count(*) FROM rec_assignments a JOIN rec_subjects s ON s.subject_id=a.subject_id WHERE s.user_id=${user} AND a.viewed_at IS NOT NULL`);
    await plain.goto(base+'/');
    assert.ok(await plain.locator(`a[data-rec-link][href='/books/${book}']`).count());
    await plain.waitForTimeout(1200);
    const viewsAfter=scalar(`SELECT count(*) FROM rec_assignments a JOIN rec_subjects s ON s.subject_id=a.subject_id WHERE s.user_id=${user} AND a.viewed_at IS NOT NULL`);
    assert.equal(viewsBefore,viewsAfter,'no-JS must not create view telemetry');
    await disabledJS.close();
    // Five real SSR surfaces carry permitted cards, never paid rights.
    for (const target of ['/', '/catalog', `/books/${book}`, '/me/library', `/read/${chapter}`]) {
      const response=await page.goto(base+target);
      assert.equal(response.status(),200,target);
      assert.ok(await page.locator('[data-rec-panel]').count(),'panel absent on '+target);
    }
    await page.goto(base+'/');
    const card=page.locator('[data-rec-card]').filter({has:page.locator(`a[href='/books/${book}']`)}).first();
    await card.scrollIntoViewIfNeeded();
    await waitFor(()=>scalar(`SELECT count(*) FROM rec_assignments a JOIN rec_subjects s ON s.subject_id=a.subject_id WHERE s.user_id=${user} AND a.book_id=${book} AND a.viewed_at IS NOT NULL`)>0,'actual visible impression');
    await page.screenshot({path:path.join(shots,'rec13-desktop.png'),fullPage:true});
    await card.locator('a[data-rec-link]').click();
    await page.waitForURL(base+`/books/${book}`);
    await waitFor(()=>scalar(`SELECT count(*) FROM rec_assignments a JOIN rec_subjects s ON s.subject_id=a.subject_id WHERE s.user_id=${user} AND a.book_id=${book} AND a.clicked_at IS NOT NULL AND a.book_opened_at IS NOT NULL`)>0,'server click and successful book open');
    // Actual browser invokes the validated HTTP progress route, not F telemetry.
    await page.goto(base+`/read/${chapter}`);
    const progress=await page.evaluate(async ({chapter,version})=>{
      const csrf=document.querySelector('input[name="csrf_token"]')?.value || document.querySelector('[data-rec-panel]')?.dataset.recCsrf;
      async function save(offset,revision) {
        const response=await fetch('/api/v1/me/progress/free',{method:'PUT',credentials:'same-origin',headers:{'Content-Type':'application/json','X-CSRF-Token':csrf},body:JSON.stringify({chapter_id:String(chapter),version_id:String(version),block_id:1,char_offset:offset,expected_revision:revision})});
        if (!response.ok) throw new Error('validated progress rejected: '+response.status);
        return response.json();
      }
      const first=await save(3,0);
      await save(13,first.revision);
      return true;
    },{chapter,version});
    assert.equal(progress,true);
    await waitFor(()=>scalar(`SELECT count(*) FROM rec_first_conversions c JOIN rec_subjects s ON s.subject_id=c.subject_id WHERE s.user_id=${user} AND c.book_id=${book} AND c.kind='chapter_finished'`)===1,'G delivers actual C completion once');
    // Own explicit preference hides this book; it must not alter another reader.
    await page.goto(base+'/');
    const hiding=page.locator('[data-rec-card]').filter({has:page.locator(`a[href='/books/${book}']`)}).first();
    await hiding.getByRole('button',{name:'Не интересно'}).click();
    await page.waitForURL('**/me/recommendations');
    await page.goto(base+'/');
    assert.equal(await page.locator(`a[data-rec-link][href='/books/${book}']`).count(),0);
    const mobile=await browser.newContext({viewport:{width:390,height:844}});
    const mobilePage=await mobile.newPage();
    await register(mobilePage,mobileEmail);
    await mobilePage.goto(base+'/');
    assert.ok(await mobilePage.locator(`a[data-rec-link][href='/books/${book}']`).count(),'preference leaked to another reader');
    await mobilePage.locator('[data-rec-panel]').scrollIntoViewIfNeeded();
    assert.ok(await mobilePage.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'horizontal overflow');
    await mobilePage.screenshot({path:path.join(shots,'rec13-mobile.png'),fullPage:true});
    await mobile.close();
    await page.goto(base+'/me/recommendations');
    const restoring=page.locator('article').filter({has:page.locator(`a[href='/books/${book}']`)}).first();
    await restoring.getByRole('button',{name:'Вернуть в рекомендации'}).click();
    await page.waitForURL('**/me/recommendations');
    // Re-saving the same enabled consent preserves the actor's current state.
    const subjectBefore=sql(`SELECT subject_id FROM rec_subjects WHERE user_id=${user}`);
    await page.getByRole('button',{name:'Сохранить выбор'}).click();
    await page.waitForURL('**/me/recommendations');
    assert.equal(sql(`SELECT subject_id FROM rec_subjects WHERE user_id=${user}`),subjectBefore);
    await page.getByRole('checkbox').uncheck();
    await page.getByRole('button',{name:'Сохранить выбор'}).click();
    await page.waitForURL('**/me/recommendations');
    assert.equal(sql(`SELECT enabled FROM rec_subjects WHERE user_id=${user}`),'f');
    assert.equal(scalar(`SELECT count(*) FROM rec_assignments a JOIN rec_subjects s ON s.subject_id=a.subject_id WHERE s.user_id=${user}`),0);
    assert.equal(scalar(`SELECT count(*) FROM reader_chapter_completions WHERE user_id=${user} AND book_id=${book}`),1);
    assert.deepEqual(errors,[]);
    console.log('REC13 browser PASS: five surfaces, actual view/click/C start/completion, own hide/clear, mobile, no-JS, opt-out');
  } finally {
    if (browser) await browser.close();
    const uuid=value=>value===null?'NULL':`'${value}'::uuid`;
    sql(`UPDATE rec_runtime_policy SET active_generation=${uuid(policy.active_generation)},previous_generation=${uuid(policy.previous_generation)},revision=${policy.revision},rollout_percent=${policy.rollout_percent},hold=${policy.hold?'true':'false'} WHERE singleton`);
    // Fixtures live only in tor_ci; preserve all unrelated test data and events.
    if (books.length) sql(`DELETE FROM books WHERE id IN(${books.join(',')})`);
    sql(`DELETE FROM users WHERE email_normalized IN('${email}','${mobileEmail}')`);
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
