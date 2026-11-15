'use strict';
// Real HTML, production worker, actual Chromium viewport geometry and native forms.
// Hidden-tab reset is separately exercised by exposure_client.test.cjs because
// Chromium headless-shell keeps all tabs visible even after bringToFront().
const {chromium}=require('playwright');
const {execFileSync}=require('node:child_process');
const {createHash}=require('node:crypto');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const db=process.env.APP_URL||'';
const base=process.env.TOR_EXPOSURE_BASE||process.env.TOR_REC_BASE||'http://127.0.0.1:3109';
const shots=process.env.TOR_EXPOSURE_SHOTS||process.env.TOR_REC_SHOTS||'/tmp/exposure-shots';
const parsed=new URL(db);
assert.equal(parsed.hostname,'127.0.0.1');assert.equal(parsed.port,'5432');assert.equal(parsed.pathname,'/tor_ci');assert.equal(parsed.protocol,'postgresql:');
assert.match(base,/^http:\/\/127\.0\.0\.1:\d+$/);fs.mkdirSync(shots,{recursive:true});
function sql(statement){return execFileSync('psql',['-XAtq','-v','ON_ERROR_STOP=1','--dbname',db,'-c',statement],{encoding:'utf8',env:{...process.env,PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'}}).trim();}
function num(statement){const value=Number(sql(statement));assert.ok(Number.isSafeInteger(value));return value;}
const pause=ms=>new Promise(r=>setTimeout(r,ms));
async function until(predicate,message){const end=Date.now()+7000;while(Date.now()<end){if(await predicate())return;await pause(40);}throw new Error(message);}
function digest(proof){assert.match(proof,/^[A-Za-z0-9_-]{43}$/);return createHash('sha256').update(Buffer.from(proof,'base64url')).digest('hex');}
function counts(proof){const raw=sql(`SELECT rendered::int||','||viewed::int||','||clicked::int FROM flow_observation_proofs WHERE digest=decode('${digest(proof)}','hex')`);return raw?raw.split(',').map(Number):null;}
async function privateResponse(response,status=200){assert.equal(response.status(),status);const h=await response.allHeaders();assert.equal(h['cache-control'],'no-store');assert.match(h['x-robots-tag'],/noindex/);}
async function position(card,left){await card.evaluate((element,x)=>{Object.assign(element.style,{position:'fixed',left:`${x}px`,top:'200px',width:'100px',height:'100px',overflow:'hidden',margin:'0',padding:'0',border:'0',transform:'none',zIndex:'9999'});},left);}
(async()=>{
  const stamp=Date.now(),label=`FlowObservation${stamp}`;let browser,actor,book;
  try{
    assert.equal(sql('SELECT count(*) FROM _sqlx_migrations WHERE version=202611286493 AND success'),'1');
    book=num(`INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres) VALUES('ci:exposure-browser:${stamp}','ru','${label} <script>safe</script>','public','basic',ARRAY['${label}']) RETURNING id`);
    browser=await chromium.launch({headless:true});
    const nojs=await browser.newContext({javaScriptEnabled:false,viewport:{width:390,height:844}});const np=await nojs.newPage();
    const response=await np.goto(base+`/catalog?q=${label}`);assert.equal(response.status(),200);assert.equal((await response.allHeaders())['cache-control'],'no-store');
    const nojsCard=np.locator('.catalog-grid article[data-flow-proof]').first();const nojsProof=await nojsCard.getAttribute('data-flow-proof');assert.ok(nojsProof);
    await until(()=>counts(nojsProof)!==null,'no-JS opportunity not committed');await pause(1200);assert.deepEqual(counts(nojsProof),[0,0,0]);assert.deepEqual(await nojs.cookies(),[]);
    await nojsCard.locator('a[data-flow-link]').click();await np.waitForURL(base+`/books/${book}`);assert.deepEqual(counts(nojsProof),[0,0,0]);
    // Genuine admin role and native no-JS forms; no operator password fallback.
    const admin=await browser.newContext({javaScriptEnabled:false,viewport:{width:1440,height:900}});const ap=await admin.newPage();await ap.goto(base+'/register');
    await ap.getByLabel('Отображаемое имя').fill('Observation operator');const email=`exposure-browser-${stamp}@tor-ci.example`;
    await ap.getByLabel('Электронная почта').fill(email);await ap.getByLabel('Пароль (12–128 символов)').fill('ObservationBrowserPassword2026');
    await ap.getByRole('button',{name:'Создать учётную запись',exact:true}).click();await ap.waitForURL('**/me');actor=num(`SELECT id FROM users WHERE email_normalized='${email}'`);
    await privateResponse(await ap.goto(base+'/admin/audience/exposure'),403);sql(`INSERT INTO account_roles(user_id,role) VALUES(${actor},'administrator')`);
    await privateResponse(await ap.goto(base+`/admin/audience/exposure?group=genre&label=${label}`));
    const row=ap.locator(`[data-testid="exposure-row"][data-book-id="${book}"]`);assert.equal(await row.locator('[data-metric="views"]').innerText(),'неизвестно');
    await ap.locator('[name="days"]').selectOption('1');await ap.getByRole('button',{name:'Показать',exact:true}).click();await ap.waitForURL(/days=1/);assert.equal(await row.locator('[data-testid="exposure-class"]').innerText(),'Недостаточно наблюдений');
    for(const width of [390,1440]){
      const visual=await browser.newContext({viewport:{width,height:900},storageState:await admin.storageState()});const page=await visual.newPage();
      await privateResponse(await page.goto(base+`/admin/audience/exposure?group=genre&label=${label}`));assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
      await page.screenshot({path:path.join(shots,`exposure-report-${width}.png`),fullPage:true,animations:'disabled'});await page.locator(`[data-book-id="${book}"]`).screenshot({path:path.join(shots,`exposure-row-${width}.png`)});await visual.close();
    }
    const observed=await browser.newContext({viewport:{width:1440,height:900}});const page=await observed.newPage();const payloads=[];
    page.on('request',request=>{if(request.url()===base+'/api/v1/flow/observations')payloads.push(request.postDataJSON());});
    await page.goto(base+`/catalog?q=${label}`,{waitUntil:'domcontentloaded'});const card=page.locator('.catalog-grid article[data-flow-proof]').first();const proof=await card.getAttribute('data-flow-proof');assert.notEqual(proof,nojsProof);
    await position(card,-60);await until(()=>counts(proof)?.[0]===1,'DOM acknowledgement missing');await pause(1200);assert.equal(counts(proof)[1],0,'40% visible cannot qualify');
    await position(card,0);await pause(550);await position(card,-60);await pause(650);assert.equal(counts(proof)[1],0,'separated intervals cannot add to one second');
    await position(card,0);await until(()=>counts(proof)?.[1]===1,'one continuous visible second did not qualify');
    await position(card,-60);await position(card,0);await pause(1200);assert.equal(payloads.filter(p=>p.proof===proof&&p.kind==='view').length,1);
    const qualified=payloads.find(p=>p.proof===proof&&p.kind==='view');assert.ok(qualified.visible_percent>=60&&qualified.continuous_visible_ms>=1000&&qualified.foreground===true);
    assert.equal(sql(`SELECT surface FROM flow_observation_proofs WHERE digest=decode('${digest(proof)}','hex')`),'search');
    assert.deepEqual(await observed.cookies(),[]);assert.equal(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
    for(const payload of payloads){assert.ok(Object.keys(payload).every(key=>['proof','kind','visible_percent','continuous_visible_ms','foreground'].includes(key)));}
    // Browser callback delivery may reorder; durable per-event bits stay one-shot.
    const click={proof,kind:'click'},view={...qualified};
    for(const body of [click,view,click])assert.equal((await page.request.post(base+'/api/v1/flow/observations',{headers:{Origin:base,'X-Tor-Observation':'1'},data:body})).status(),204);
    assert.deepEqual(counts(proof),[1,1,1]);
    // Hold the actual click request unresolved: ordinary navigation must finish first.
    await page.goto(base+`/catalog?q=${label}`,{waitUntil:'domcontentloaded'});const navigationCard=page.locator('.catalog-grid article[data-flow-proof]').first();const navigationProof=await navigationCard.getAttribute('data-flow-proof');await position(navigationCard,0);
    let held;
    await page.route('**/api/v1/flow/observations',async route=>{if(route.request().postDataJSON().kind==='click'){held=route;return;}await route.continue();});
    await navigationCard.locator('a[data-flow-link]').click({position:{x:20,y:20}});await page.waitForURL(base+`/books/${book}`,{timeout:4000});await until(()=>!!held,'click callback was not attempted');
    assert.equal(counts(navigationProof)?.[2]||0,0);await held.abort('failed').catch(()=>{});await page.unroute('**/api/v1/flow/observations');
    // A proof cannot keep a now-unavailable book observable.
    sql(`UPDATE books SET admin_hold=true WHERE id=${book}`);assert.equal((await page.request.post(base+'/api/v1/flow/observations',{headers:{Origin:base,'X-Tor-Observation':'1'},data:view})).status(),403);
    await privateResponse(await ap.goto(base+`/admin/audience/exposure?group=genre&label=${label}`));assert.equal(await ap.locator('[data-testid="exposure-row"]').count(),0);
    assert.equal(sql(`SELECT count(*) FROM rec_subjects WHERE user_id=${actor}`),'0');
    console.log('EXPOSURE_BROWSER_PASS noJS unknown; native admin forms; 390/1440; actual 60%/1s reset; one-shot; replay/reorder; no identity/storage; delayed callback cannot block navigation; live book withdrawal');
  }finally{
    if(book)sql(`UPDATE books SET admin_hold=true WHERE id=${book}`);
    if(browser)await browser.close();
    if(book)sql(`BEGIN;SELECT live_proofs FROM flow_observation_state WHERE singleton FOR UPDATE;WITH deleted AS (DELETE FROM flow_observation_proofs WHERE book_id=${book} RETURNING 1) UPDATE flow_observation_state SET live_proofs=live_proofs-(SELECT count(*) FROM deleted) WHERE singleton;DELETE FROM flow_observation_hourly WHERE book_id=${book};DELETE FROM books WHERE id=${book};COMMIT`);
    if(actor)sql(`DELETE FROM account_roles WHERE user_id=${actor};DELETE FROM users WHERE id=${actor}`);
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
