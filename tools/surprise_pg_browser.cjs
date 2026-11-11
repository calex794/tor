'use strict';
// F-SURPRISE: real Chromium + PostgreSQL + TOR binary. Never use production data.
const {chromium} = require('playwright');
const {execFileSync} = require('node:child_process');
const {randomBytes} = require('node:crypto');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const db = process.env.TEST_DATABASE_URL || '';
const base = process.env.TOR_SURPRISE_BASE || 'http://127.0.0.1:3108';
const shots = process.env.TOR_SURPRISE_SHOTS || '/tmp/tor-surprise-shots';
const parsed = new URL(db);
if (process.env.GITHUB_ACTIONS !== 'true' ||
    parsed.protocol !== 'postgresql:' || parsed.hostname !== '127.0.0.1' ||
    parsed.port !== '5432' || parsed.pathname !== '/tor_ci' ||
    !/^http:\/\/127\.0\.0\.1:3108$/.test(base)) {
  throw new Error('F-SURPRISE Chromium acceptance is restricted to CI loopback tor_ci');
}
fs.mkdirSync(shots, {recursive:true});

function sql(statement) {
  return execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1',
    '--dbname',db,'--command',statement], {
    encoding:'utf8', env:{...process.env,PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'},
  }).trim();
}
function scalar(statement) {
  const out=sql(statement);
  assert.match(out,/^\d+$/,'expected a numeric SQL result: '+out);
  return Number(out);
}
const literal=s => "'" + String(s).replace(/'/g,"''") + "'";
const one = (table,col,id) => scalar(`SELECT count(*) FROM ${table} WHERE ${col}=${id}`);
async function register(page,email) {
  assert.equal((await page.goto(base+'/register')).status(),200);
  await page.getByLabel('Отображаемое имя').fill('Surprise Chromium QA');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('SurpriseBrowserAcceptancePass2026');
  await page.getByRole('button',{name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
  return scalar(`SELECT id FROM users WHERE email_normalized=${literal(email)}`);
}
async function submit(page, form, route, label) {
  const response=page.waitForResponse(r=>new URL(r.url()).pathname===route
    && r.request().method()==='POST');
  await form.getByRole('button',{name:label}).click();
  assert.equal((await response).status(),303,'admin form must redirect after audited mutation');
  await page.waitForLoadState('networkidle');
}
async function planFor(page,url) {
  const planned=page.waitForResponse(r=>new URL(r.url()).pathname==='/api/v1/me/surprises/plan');
  assert.equal((await page.goto(base+url)).status(),200,'public surface must remain readable');
  const response=await planned;
  assert.equal(response.status(),200);
  return response.json();
}
async function activateIcon(page, user, plan) {
  assert.equal(plan.state,'planned');
  assert.ok(plan.active_after_seconds>=120 && plan.active_after_seconds<=1200);
  assert.ok(Number.isSafeInteger(plan.utc_day));
  // CI backdates only the disposable decision: browser still runs the real
  // unmodified timer, real safe-area selection and a real POST /claim.
  sql(`UPDATE surprise_daily_decisions SET created_at=now()-interval '30 minutes'
    WHERE user_id=${user} AND utc_day=${plan.utc_day} AND state='planned'`);
  await page.bringToFront();
  await page.evaluate(()=>document.dispatchEvent(new Event('pointerdown',{bubbles:true})));
  assert.equal(await page.evaluate(()=>document.visibilityState),'visible');
  assert.equal(await page.evaluate(()=>document.hasFocus()),true);
  const trigger=page.getByRole('button',{name:'Открыть случайную находку'});
  for (let sec=0;sec<=plan.active_after_seconds+60;sec+=30) {
    await page.clock.runFor(30000);
    if (await trigger.count()) break;
    await page.evaluate(()=>document.dispatchEvent(new Event('pointerdown',{bubbles:true})));
  }
  if (!(await trigger.count())) {
    const context=await page.evaluate(()=>{
      const main=document.querySelector('main'),bounds=main?.getBoundingClientRect();
      const paragraphs=Array.from(main?.querySelectorAll('p')||[]);
      return {url:location.pathname,visibility:document.visibilityState,
        focused:document.hasFocus(),selection:String(window.getSelection()).length,
        dialogs:Array.from(document.querySelectorAll('[role="dialog"]')).map(el=>({
          hidden:!!el.closest('[hidden], [inert], details:not([open])'),
          visible:el.getClientRects().length>0,display:getComputedStyle(el).display,
        })),
        main:{left:bounds?.left,right:bounds?.right},
        visibleParagraphs:paragraphs.filter(el=>{
          const r=el.getBoundingClientRect();
          return r.top>65 && r.bottom<innerHeight-90 &&
            !el.closest('form,nav,header,footer,.chapter-navigation,.support-widget');
        }).length};
    });
    const saved=sql(`SELECT state FROM surprise_daily_decisions
      WHERE user_id=${user} AND utc_day=${plan.utc_day}`);
    console.error('TOR_SURPRISE_DIAGNOSTIC '+JSON.stringify({plan,context,saved}));
    await page.screenshot({path:path.join(shots,'surprise-missing-icon.png')});
  }
  await trigger.waitFor({state:'visible',timeout:10000});
  assert.equal(await trigger.count(),1,'only one visible icon per page');
  const geometry=await page.locator('.tor-surprise').evaluate(el=>{
    const card=el.getBoundingClientRect(),main=document.querySelector('main').getBoundingClientRect();
    return {position:el.className,card:{left:card.left,right:card.right,top:card.top},
      main:{left:main.left,right:main.right},overflow:document.documentElement.scrollWidth>innerWidth+1,
      previous:el.previousElementSibling?.tagName || null,
      inMain:document.querySelector('main').contains(el)};
  });
  assert.equal(geometry.overflow,false,'surprise must not overflow the viewport');
  if (geometry.position.includes('--left')) assert.ok(geometry.card.right<=geometry.main.left);
  if (geometry.position.includes('--right')) assert.ok(geometry.card.left>=geometry.main.right);
  if (geometry.position.includes('--inline')) {
    assert.equal(geometry.inMain,true,'mobile gift must remain inside main');
    assert.equal(geometry.previous,'P','mobile gift must follow a real readable paragraph');
  }
  return trigger;
}
async function assertNoUnexpectedErrors(page,events) {
  assert.deepEqual(events,[],'unexpected browser errors or failed surprise requests');
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1),false);
}
function watch(page) {
  const errors=[];
  page.on('pageerror',e=>errors.push('pageerror: '+e.message));
  page.on('response',r=>{
    const route=new URL(r.url()).pathname;
    if (route.startsWith('/api/v1/me/surprises/') && r.status()>=500)
      errors.push(route+' returned '+r.status());
  });
  return errors;
}
async function openKeyboard(page,trigger,expectedTitle,book) {
  await trigger.focus();
  await page.keyboard.press('Enter');
  const card=page.locator('.tor-surprise__card');
  await card.waitFor({state:'visible',timeout:10000});
  assert.equal(await card.locator('h3').innerText(),expectedTitle);
  assert.equal(await card.getByRole('link',{name:'Открыть книгу'}).getAttribute('href'),'/books/'+book);
  assert.ok((await card.innerText()).includes('Выбор редакции'));
  assert.ok((await card.innerText()).includes('Тестовая цитата'));
  assert.equal(await page.evaluate(()=>document.activeElement?.textContent),'Закрыть');
  return card;
}

(async()=>{
  const run=Date.now()+'-'+randomBytes(4).toString('hex');
  const adminEmail=`surprise-admin-${run}@tor-ci.example`;
  const desktopEmail=`surprise-desktop-${run}@tor-ci.example`;
  const mobileEmail=`surprise-mobile-${run}@tor-ci.example`;
  const title=`Surprise-Chromium-${run}`;
  const ids={users:[],book:null,chapter:null,item:null};
  let browser;
  try {
    assert.equal(sql('SELECT enabled::text FROM surprise_runtime_policy WHERE singleton'),'false',
      'feature must be OFF by default before the opt-in test');
    ids.book=scalar(`INSERT INTO books(source_identity,target_language,title,visibility,access_level)
      VALUES(${literal('ci:surprise:'+run)},'ru',${literal('Книга для сюрприза '+run)},'public','basic') RETURNING id`);
    ids.chapter=scalar(`INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
      VALUES(${ids.book},1,'Проверка читательского текста','public','free') RETURNING id`);
    const paragraphs=Array.from({length:8},(_,i)=>
      `Тестовый материал главы. Абзац ${i+1}. Текст нельзя перекрывать подарком. Пример чтения без прерывания переходов и выделения.`).join('\n\n');
    const version=scalar(`INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
      VALUES(${ids.chapter},1,${literal(paragraphs)}) RETURNING id`);
    sql(`INSERT INTO chapter_publications(chapter_id,version_id) VALUES(${ids.chapter},${version})`);
    browser=await chromium.launch({headless:true});
    const adminContext=await browser.newContext({viewport:{width:1440,height:900}});
    const admin=await adminContext.newPage();
    ids.users.push(await register(admin,adminEmail));
    sql(`INSERT INTO account_roles(user_id,role,is_active) VALUES(${ids.users[0]},'administrator',true)`);
    assert.equal((await admin.goto(base+'/admin/surprises')).status(),200);
    const create=admin.locator('form[action="/admin/surprises/create"]');
    await create.locator('[name=kind]').selectOption('quote');
    await create.locator('[name=title]').fill(title);
    await create.locator('[name=body]').fill('Тестовая цитата, безопасная для публичного читателя.');
    await create.locator('[name=book_id]').fill(String(ids.book));
    await create.locator('[name=editorial]').check();
    await submit(admin,create,'/admin/surprises/create','Создать черновик');
    ids.item=scalar(`SELECT id FROM surprise_content WHERE title=${literal(title)} AND state='draft'`);
    const state=admin.locator(`form[action="/admin/surprises/${ids.item}/state"]`)
      .filter({has:admin.locator('input[value="approved"]')});
    await submit(admin,state,`/admin/surprises/${ids.item}/state`,'approved');
    const policy=admin.locator('form[action="/admin/surprises/policy"]');
    await policy.locator('input[name=enabled]').check();
    await policy.locator('input[name=probability_per_mille]').fill('1000');
    await submit(admin,policy,'/admin/surprises/policy','Сохранить настройки');
    assert.equal(sql('SELECT enabled::text FROM surprise_runtime_policy WHERE singleton'),'true');
    assert.equal(scalar(`SELECT count(*) FROM surprise_admin_events WHERE actor_user_id=${ids.users[0]}`),3);

    const guestContext=await browser.newContext({viewport:{width:390,height:844}});
    const guest=await guestContext.newPage();
    assert.equal((await guest.goto(base+'/')).status(),200);
    await guest.waitForLoadState('networkidle');
    assert.equal(await guest.locator('.tor-surprise').count(),0,'no gift for a guest');
    assert.equal((await guestContext.request.get(base+'/api/v1/me/surprises/plan')).status(),401);

    const desktopContext=await browser.newContext({viewport:{width:1440,height:900}});
    const desktop=await desktopContext.newPage();
    const desktopErrors=watch(desktop);
    ids.users.push(await register(desktop,desktopEmail));
    for (const url of ['/',`/books/${ids.book}`]) {
      const result=await planFor(desktop,url);
      assert.equal(result.state,'planned');
      assert.equal(await desktop.locator('script[src="/assets/surprise.js"]').count(),1);
      assert.equal(await desktop.locator('link[href="/assets/surprise.css"]').count(),1);
    }
    await desktop.clock.install();
    const plan=await planFor(desktop,`/reader/${ids.chapter}`);
    assert.ok((await desktop.locator('main').innerText()).includes('Тестовый материал главы'));
    await desktop.locator('article#chapter-text p').first().scrollIntoViewIfNeeded();
    const sibling=await desktopContext.newPage();
    const plan2=await planFor(sibling,`/books/${ids.book}`);
    assert.equal(plan.utc_day,plan2.utc_day);
    assert.equal(plan.active_after_seconds,plan2.active_after_seconds,'reload/tabs cannot reroll');
    const trigger=await activateIcon(desktop,ids.users[1],plan);
    await desktop.screenshot({path:path.join(shots,'surprise-desktop-icon.png'),animations:'disabled'});
    const card=await openKeyboard(desktop,trigger,title,ids.book);
    await desktop.screenshot({path:path.join(shots,'surprise-desktop-open.png'),animations:'disabled'});
    assert.equal(one('surprise_daily_decisions','user_id',ids.users[1]),1);
    assert.equal(sql(`SELECT state FROM surprise_daily_decisions WHERE user_id=${ids.users[1]}`),'opened');
    const siblingClaim=await sibling.evaluate(async input=>{
      const r=await fetch('/api/v1/me/surprises/claim',{
        method:'POST',credentials:'same-origin',
        headers:{'content-type':'application/json','x-csrf-token':input.csrf},
        body:JSON.stringify({utc_day:input.day,active_seconds:1200})});
      return {status:r.status,body:await r.json()};
    },{day:plan.utc_day,csrf:plan2.csrf_token});
    assert.deepEqual(siblingClaim,{status:200,body:{accepted:false}},'other tab cannot claim again');
    const disabled=desktop.waitForResponse(r=>new URL(r.url()).pathname==='/api/v1/me/surprises/disable');
    await card.getByRole('button',{name:'Больше не показывать'}).click();
    assert.equal((await disabled).status(),204);
    assert.equal(await desktop.locator('.tor-surprise').count(),0);
    assert.equal(sql(`SELECT disabled::text FROM surprise_user_preferences WHERE user_id=${ids.users[1]}`),'true');
    assert.equal((await planFor(desktop,'/')).state,'off');
    await assertNoUnexpectedErrors(desktop,desktopErrors);

    const mobileContext=await browser.newContext({viewport:{width:390,height:844},isMobile:true,
      hasTouch:true,deviceScaleFactor:1});
    const mobile=await mobileContext.newPage();
    const mobileErrors=watch(mobile);
    ids.users.push(await register(mobile,mobileEmail));
    await mobile.clock.install();
    const mobilePlan=await planFor(mobile,`/reader/${ids.chapter}`);
    await mobile.locator('article#chapter-text p').first().scrollIntoViewIfNeeded();
    const mobileIcon=await activateIcon(mobile,ids.users[2],mobilePlan);
    assert.ok((await mobile.locator('.tor-surprise').getAttribute('class')).includes('--inline'),
      'narrow viewport must not overlay book text');
    await mobile.screenshot({path:path.join(shots,'surprise-mobile-icon.png'),animations:'disabled'});
    await openKeyboard(mobile,mobileIcon,title,ids.book);
    await mobile.screenshot({path:path.join(shots,'surprise-mobile-open.png'),animations:'disabled'});
    await assertNoUnexpectedErrors(mobile,mobileErrors);

    const noJs=await browser.newContext({viewport:{width:390,height:844},javaScriptEnabled:false});
    await noJs.addCookies(await mobileContext.cookies());
    const noJsPage=await noJs.newPage();
    assert.equal((await noJsPage.goto(base+`/reader/${ids.chapter}`)).status(),200);
    assert.ok((await noJsPage.locator('main').innerText()).includes('Тестовый материал главы'));
    assert.equal(await noJsPage.locator('.tor-surprise').count(),0,'reading must work without JS');

    sql(`UPDATE books SET access_level='adult' WHERE id=${ids.book}`);
    const secret=await mobile.evaluate(async input=>{
      const r=await fetch('/api/v1/me/surprises/open',{
        method:'POST',credentials:'same-origin',
        headers:{'content-type':'application/json','x-csrf-token':input.csrf},
        body:JSON.stringify({utc_day:input.day})});
      return {status:r.status,text:await r.text()};
    },{csrf:mobilePlan.csrf_token,day:mobilePlan.utc_day});
    assert.equal(secret.status,404,'age escalation must stop subsequent content retrieval');
    assert.ok(!secret.text.includes(title));
    sql(`UPDATE books SET access_level='basic' WHERE id=${ids.book}`);
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${ids.users[0]} AND role='administrator'`);
    assert.equal((await admin.goto(base+'/admin/surprises')).status(),403,'revoked admin role must take effect');
    console.log('F-SURPRISE Chromium desktop/mobile/multi-tab/no-JS/ACL/admin: PASS');
  } finally {
    if (browser) await browser.close();
    // All data is disposable, and the SQL kill switch returns to OFF even on failure.
    sql("UPDATE surprise_runtime_policy SET enabled=false,probability_per_mille=350 WHERE singleton");
    if (ids.users.length) {
      const users=ids.users.join(',');
      sql(`DELETE FROM surprise_admin_events WHERE actor_user_id IN (${users})`);
    }
    if (ids.item) {
      sql(`DELETE FROM surprise_admin_events WHERE content_id=${ids.item}`);
      sql(`DELETE FROM surprise_content WHERE id=${ids.item}`);
    }
    if (ids.users.length) {
      const users=ids.users.join(',');
      sql(`DELETE FROM account_roles WHERE user_id IN (${users})`);
      sql(`DELETE FROM users WHERE id IN (${users})`);
    }
    if (ids.chapter) {
      sql(`DELETE FROM chapter_publications WHERE chapter_id=${ids.chapter}`);
      sql(`DELETE FROM chapter_versions WHERE chapter_id=${ids.chapter}`);
      sql(`DELETE FROM chapters WHERE id=${ids.chapter}`);
    }
    if (ids.book) sql(`DELETE FROM books WHERE id=${ids.book}`);
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
