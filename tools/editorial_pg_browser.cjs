"use strict";
// Native administration, the three approved live slots, and shared observation integration.
const {chromium}=require('playwright');
const {execFileSync}=require('node:child_process');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const db=process.env.APP_URL||'';
const base=process.env.TOR_EDITORIAL_BASE||'http://127.0.0.1:3108';
const shots=process.env.TOR_EDITORIAL_SHOTS||'/tmp/editorial-shots';
const expectObservations=process.env.TOR_EDITORIAL_EXPECT_OBSERVATIONS==='1';
const parsed=new URL(db);
assert.equal(parsed.protocol,'postgresql:');assert.equal(parsed.hostname,'127.0.0.1');
assert.equal(parsed.port,'5432');assert.equal(parsed.pathname,'/tor_ci');
assert.match(base,/^http:\/\/127\.0\.0\.1:\d+$/);
fs.mkdirSync(shots,{recursive:true});
function sql(query){return execFileSync('psql',['-XAtq','-v','ON_ERROR_STOP=1','--dbname',db,'-c',query],{encoding:'utf8',env:{...process.env,PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'}}).trim();}
function id(query){const result=Number(sql(query));assert.ok(Number.isSafeInteger(result)&&result>0);return result;}
function sums(campaign){return sql(`SELECT coalesce(sum(opportunities),0)||'|'||coalesce(sum(rendered),0)||'|'||coalesce(sum(views),0)||'|'||coalesce(sum(clicks),0) FROM flow_observation_hourly WHERE campaign_id=${campaign}`).split('|').map(Number);}
async function poll(assertion){let last;for(let n=0;n<40;n++){try{return assertion();}catch(error){last=error;await new Promise(r=>setTimeout(r,150));}}throw last;}
const PAGE='/admin/promotion/editorial';
async function privateResponse(response,status=200){assert.equal(response.status(),status);assert.equal((await response.allHeaders())['cache-control'],'no-store');assert.match((await response.allHeaders())['x-robots-tag'],/noindex/);}
async function submit(page,form){const [response]=await Promise.all([page.waitForResponse(r=>r.url()===base+PAGE&&r.request().method()==='POST'),form.getByRole('button').click()]);await privateResponse(response,303);await page.waitForURL(url=>url.pathname===PAGE&&url.searchParams.has('changed'));}
async function action(page,campaign,value){await page.goto(base+PAGE);const form=page.locator(`[data-editorial-state="${campaign}"]`);await form.locator('select[name="action"]').selectOption(value);await submit(page,form);}
async function watchSecurity(context){
 await context.addInitScript(()=>{
  window.editorialCspViolations=[];
  document.addEventListener('securitypolicyviolation',event=>window.editorialCspViolations.push({directive:event.effectiveDirective,blocked:event.blockedURI}));
 });
}
function observationResponses(page){
 const events=[];
 page.on('response',response=>{
  if(response.url()!==base+'/api/v1/flow/observations')return;
  const body=response.request().postDataJSON();
  events.push({proof:body.proof,kind:body.kind,status:response.status()});
 });
 return events;
}
async function observedCard(page,card,campaign,before,response,events){
 const proof=await card.getAttribute('data-flow-proof');assert.match(proof,/^[A-Za-z0-9_-]{43}$/);
 const geometry=await card.evaluate(element=>new Promise(resolve=>{
  const observer=new IntersectionObserver(([entry])=>{
   observer.disconnect();resolve({ratio:entry.intersectionRatio,foreground:document.visibilityState});
  });observer.observe(element);
 }));
 assert.equal(geometry.foreground,'visible');assert.ok(geometry.ratio>=0.6,`Card visibility: ${JSON.stringify(geometry)}`);
 const violations=()=>page.evaluate(()=>window.editorialCspViolations);
 try{
  assert.deepEqual(await violations(),[],`CSP violations on ${page.url()}`);
  await poll(()=>{
   assert.ok(events.some(event=>event.proof===proof&&event.kind==='view'&&event.status===204),'This card has no accepted view callback');
   assert.ok(sums(campaign)[2]>before,'This navigation did not increase its campaign view aggregate');
  });
  assert.deepEqual(await violations(),[],`CSP violations on ${page.url()}`);
 }catch(error){
  const callbacks=events.filter(event=>event.proof===proof).map(({kind,status})=>({kind,status}));
  const csp=(await response.allHeaders())['content-security-policy'];
  throw new Error(`Observation diagnostics: ${JSON.stringify({url:page.url(),geometry,callbacks,csp,violations:await violations(),counts:sums(campaign)})}`,{cause:error});
 }
}
(async()=>{
 let browser,actor,host,promoted,chapter;const campaigns=[];
 const stamp=Date.now();
 try {
  browser=await chromium.launch({headless:true});
  const admin=await browser.newContext({javaScriptEnabled:false,viewport:{width:1440,height:900}});const page=await admin.newPage();
  await page.goto(base+'/register');
  await page.getByLabel('Отображаемое имя').fill('Редактор продвижения');
  const email=`editorial-browser-${stamp}@example.invalid`;
  await page.getByLabel('Электронная почта').fill(email);await page.getByLabel('Пароль (12–128 символов)').fill('EditorialBrowserPassword2026');
  await page.getByRole('button',{name:'Создать учётную запись'}).click();await page.waitForURL('**/me');
  actor=id(`SELECT id FROM users WHERE email_normalized='${email}'`);
  await privateResponse(await page.goto(base+PAGE),403);
  sql(`INSERT INTO account_roles(user_id,role) VALUES(${actor},'administrator')`);
  await page.goto(base+'/me');await page.getByRole('link',{name:'Редакционное продвижение',exact:true}).click();await privateResponse(await page.goto(base+PAGE));
  promoted=id(`INSERT INTO books(source_identity,target_language,title,visibility,access_level) VALUES('ci:editorial-browser:${stamp}:p','ru','Editorial <script>safe</script>','public','basic') RETURNING id`);
  host=id(`INSERT INTO books(source_identity,target_language,title,visibility,access_level) VALUES('ci:editorial-browser:${stamp}:h','ru','Editorial host','public','basic') RETURNING id`);
  chapter=id(`INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) VALUES(${host},1,'Editorial free chapter','public','free',0) RETURNING id`);
  const version=id(`INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) VALUES(${chapter},1,'EDITORIAL FREE CHAPTER TEXT') RETURNING id`);
  sql(`INSERT INTO chapter_publications(chapter_id,version_id) VALUES(${chapter},${version})`);
  const baseline=sql('SELECT (SELECT count(*) FROM reader_wallet_entries)||\'|\'||(SELECT count(*) FROM reader_chapter_completions)||\'|\'||(SELECT count(*) FROM rec_subjects)');
  const start=new Date(Date.now()-60000).toISOString().replace(/\.\d{3}Z$/,'Z');const end=new Date(Date.now()+3600000).toISOString().replace(/\.\d{3}Z$/,'Z');
  for(const slot of ['home_featured','book_detail_related','chapter_end_free']){
   await page.goto(base+PAGE);const form=page.locator('[data-editorial-create]');
   await form.getByLabel('ID книги',{exact:true}).fill(String(promoted));await form.locator('select[name="slot"]').selectOption(slot);
   await form.getByLabel('Начало, UTC RFC3339',{exact:true}).fill(start);await form.getByLabel('Окончание, UTC RFC3339',{exact:true}).fill(end);
   await form.getByLabel('Приоритет (0–100)',{exact:true}).fill('20');await submit(page,form);
   const campaign=Number(new URL(page.url()).searchParams.get('changed'));assert.ok(campaign>0);campaigns.push(campaign);
   const card=page.locator(`[data-editorial-management="${campaign}"]`);await card.getByText('Предпросмотр карточки',{exact:true}).click();
   assert.equal(await card.locator('[data-flow-proof]').count(),0);
   if(expectObservations)assert.deepEqual(sums(campaign),[0,0,0,0]);
   await action(page,campaign,'activate');
  }
  const guest=await browser.newContext({viewport:{width:1440,height:900}});await watchSecurity(guest);const publicPage=await guest.newPage();const publicEvents=observationResponses(publicPage);
  const slots=[['/',campaigns[0]], [`/books/${host}`,campaigns[1]],[`/reader/${chapter}`,campaigns[2]]];
  for(const width of [390,1440]){
   const visual=await browser.newContext({viewport:{width,height:900},storageState:await admin.storageState()});await watchSecurity(visual);const vp=await visual.newPage();const privateEvents=observationResponses(vp);
   await privateResponse(await vp.goto(base+PAGE));assert.equal(await vp.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
   await vp.screenshot({path:path.join(shots,`editorial-admin-${width}.png`),fullPage:true,animations:'disabled'});
   const privateBefore=expectObservations?sums(campaigns[2])[2]:0;
   await vp.bringToFront();const privatePageResponse=await vp.goto(base+`/read/${chapter}`);assert.equal(privatePageResponse.status(),200);
   const privateCard=vp.locator(`[data-editorial-campaign="${campaigns[2]}"]`);assert.equal(await privateCard.count(),1);await privateCard.scrollIntoViewIfNeeded();
   assert.equal(await vp.locator('[data-rec-panel]').count(),0,'active /read editorial must preempt the organic shelf');
   assert.match(await vp.locator('.reader-private__content').innerText(),/EDITORIAL FREE CHAPTER TEXT/);
   assert.equal(await vp.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
   if(expectObservations)await observedCard(vp,privateCard,campaigns[2],privateBefore,privatePageResponse,privateEvents);
   await privateCard.screenshot({path:path.join(shots,`editorial-free-account-${width}.png`)});
   await visual.close();await publicPage.bringToFront();await publicPage.setViewportSize({width,height:900});
   for(const [route,campaign] of slots){
    const before=expectObservations?sums(campaign)[2]:0;
    const publicResponse=await publicPage.goto(base+route);assert.equal(publicResponse.status(),200);
    const card=publicPage.locator(`[data-editorial-campaign="${campaign}"]`);assert.equal(await card.count(),1);await card.scrollIntoViewIfNeeded();
    assert.equal(await publicPage.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
    assert.equal(await card.locator('script').count(),0);assert.match(await card.innerText(),/<script>safe<\/script>/);
    if(route.startsWith('/reader/')){
     assert.equal(await publicPage.locator('[data-rec-panel]').count(),0,'active /reader editorial must preempt the organic shelf');
     assert.equal(await publicPage.evaluate(()=>Boolean(document.getElementById('chapter-text').compareDocumentPosition(document.querySelector('.editorial-promotion'))&Node.DOCUMENT_POSITION_FOLLOWING)),true);
    }
    if(expectObservations)await observedCard(publicPage,card,campaign,before,publicResponse,publicEvents);
    await card.screenshot({path:path.join(shots,`editorial-${campaign}-${width}.png`)});
   }
  }
  await publicPage.goto(base+'/');await publicPage.locator(`[data-editorial-campaign="${campaigns[0]}"] a`).click();await publicPage.waitForURL(base+`/books/${promoted}`);
  assert.equal(await publicPage.locator('.editorial-promotion').count(),0);
  if(expectObservations){await poll(()=>{const [opportunities,rendered,views,clicks]=sums(campaigns[0]);assert.ok(opportunities>=1&&rendered>=1&&views>=1&&clicks>=1);});await privateResponse(await page.goto(base+`/admin/audience/exposure?source=editorial&campaign=${campaigns[0]}&days=7`));}
  await publicPage.goto(base+'/catalog');assert.equal(await publicPage.locator('.editorial-promotion').count(),0);
  await action(page,campaigns[0],'pause');await publicPage.goto(base+'/');assert.equal(await publicPage.locator(`[data-editorial-campaign="${campaigns[0]}"]`).count(),0);
  await page.goto(base+PAGE);const editable=page.locator(`[data-editorial-management="${campaigns[0]}"]`);await editable.getByText('Изменить расписание и приоритет',{exact:true}).click();
  const edit=editable.locator('form').filter({has:page.locator('input[name="action"][value="update"]')});await edit.getByLabel('Приоритет (0–100)').fill('25');await submit(page,edit);await action(page,campaigns[0],'activate');
  sql(`UPDATE books SET admin_hold=true WHERE id=${promoted}`);await publicPage.goto(base+`/reader/${chapter}`);assert.equal(await publicPage.locator('.editorial-promotion').count(),0);assert.match(await publicPage.locator('#chapter-text').innerText(),/EDITORIAL FREE CHAPTER TEXT/);
  sql(`UPDATE books SET admin_hold=false WHERE id=${promoted}`);await action(page,campaigns[0],'end');
  await page.goto(base+PAGE);assert.equal(await page.locator(`[data-editorial-state="${campaigns[0]}"]`).count(),0);
  sql(`UPDATE account_roles SET is_active=false WHERE user_id=${actor} AND role='administrator'`);await privateResponse(await page.goto(base+PAGE),403);
  assert.equal(sql('SELECT (SELECT count(*) FROM reader_wallet_entries)||\'|\'||(SELECT count(*) FROM reader_chapter_completions)||\'|\'||(SELECT count(*) FROM rec_subjects)'),baseline);
  assert.ok((await guest.cookies()).length===0);assert.ok((await admin.cookies()).every(c=>['tor-dev-session','tor-dev-csrf'].includes(c.name)));
  await guest.close();await admin.close();
  console.log(`PASS editorial native no-JS create/activate/pause/update/end; three real slots;390/1440;live safety;shared observations=${expectObservations}`);
 } finally {
  const fixtureBooks=[promoted,host].filter(Boolean);
  if(fixtureBooks.length)sql(`UPDATE books SET admin_hold=true WHERE id IN (${fixtureBooks.join(',')})`);
  if(browser)await browser.close();
  if(chapter)sql(`DELETE FROM chapter_publications WHERE chapter_id=${chapter}`);
  // These fixtures allocate explicit ids. Remove their anonymous measurements
  // before another suite can reuse those ids; do not leave historical rows
  // attached to an unrelated fixture book on a repeated disposable-DB run.
  if(fixtureBooks.length&&expectObservations)sql(`BEGIN;SELECT live_proofs FROM flow_observation_state WHERE singleton FOR UPDATE;WITH deleted AS (DELETE FROM flow_observation_proofs WHERE book_id IN (${fixtureBooks.join(',')}) RETURNING 1) UPDATE flow_observation_state SET live_proofs=live_proofs-(SELECT count(*) FROM deleted) WHERE singleton;DELETE FROM flow_observation_hourly WHERE book_id IN (${fixtureBooks.join(',')});DELETE FROM books WHERE id IN (${fixtureBooks.join(',')});COMMIT`);
  else if(fixtureBooks.length)sql(`DELETE FROM books WHERE id IN (${fixtureBooks.join(',')})`);
  if(actor)sql(`DELETE FROM account_roles WHERE user_id=${actor}`);
 }
})().catch(error=>{console.error(error);process.exitCode=1;});
