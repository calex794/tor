"use strict";
// Real native forms, production background builder, live F/B/A gates and no analytics.
const {chromium}=require('playwright');
const {execFileSync}=require('node:child_process');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const {performance}=require('node:perf_hooks');
const db=process.env.APP_URL||'';
const base=process.env.TOR_REC_BASE||'http://127.0.0.1:3108';
const shots=process.env.TOR_REC_SHOTS||'/tmp/contextual-shots';
const parsed=new URL(db);
assert.equal(parsed.hostname,'127.0.0.1');assert.equal(parsed.port,'5432');
assert.equal(parsed.pathname,'/tor_ci');assert.equal(parsed.protocol,'postgresql:');
assert.match(base,/^http:\/\/127\.0\.0\.1:\d+$/);
fs.mkdirSync(shots,{recursive:true});
function sql(statement){return execFileSync('psql',['-XAtq','-v','ON_ERROR_STOP=1','--dbname',db,'-c',statement],{encoding:'utf8',env:{...process.env,PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'}}).trim();}
function number(statement){const value=Number(sql(statement));assert.ok(Number.isSafeInteger(value));return value;}
const {optionalSnapshot,assertNoControls}=require('./collection_retirement_fixtures.cjs')(sql);
const PAGE='/me/recommendations/contextual',API='/api/v1/admin/recommendations/contextual';
async function privateResponse(response,status=200){assert.equal(response.status(),status);const h=await response.allHeaders();assert.equal(h['cache-control'],'no-store');assert.match(h['x-robots-tag'],/noindex/);}
async function submit(page,form){const [response]=await Promise.all([page.waitForResponse(r=>r.url()===base+PAGE&&r.request().method()==='POST'),form.getByRole('button').click()]);await privateResponse(response,303);await page.waitForURL(base+PAGE);}
async function poll(page,id,seeds){
 // The production limit is per seed, whereas this waits for the entire catalogue.
 // Include its 2s page deadline, 250ms inter-page delay and 5s idle wake-up without
 // altering any runtime limit. A stalled cursor fails well before the hard ceiling.
 const budget=Math.min(600000,15000+(seeds+1)*2250);
 const started=performance.now();let progressed=started,cursor=-1n;
 while(performance.now()-started<budget){
  const response=await page.request.get(base+API,{timeout:5000});assert.equal(response.status(),200);
  const state=await response.json(),row=state.generations.find(g=>g.id===id);
  assert.ok(row,'owned generation disappeared during its build');
  assert.match(row.cursor_book_id,/^\d+$/);const next=BigInt(row.cursor_book_id);
  assert.ok(next>=cursor,'production build cursor moved backwards');
  if(next>cursor){cursor=next;progressed=performance.now();}
  if(row.status==='validated'){
   console.log(`CONTEXTUAL_BROWSER_BUILD seeds=${seeds} elapsed_ms=${Math.round(performance.now()-started)} budget_ms=${budget}`);
   return;
  }
  assert.equal(row.status,'building');
  assert.ok(performance.now()-progressed<20000,`production metadata builder stalled at cursor ${cursor}`);
  await new Promise(r=>setTimeout(r,200));
 }
 throw new Error(`production metadata builder exceeded whole-catalogue budget: seeds=${seeds}, cursor=${cursor}, budget_ms=${budget}`);
}
async function revision(page){return (await (await page.request.get(base+API)).json()).revision;}
async function build(page,generations){
 const before=await (await page.request.get(base+API)).json();
 assert.ok(before.generations.every(g=>g.status!=='building'),'another fixture left a building generation');
 const seeds=number('SELECT count(*) FROM rec_materializable_books');
 await page.goto(base+PAGE);await submit(page,page.locator('form[data-action="refresh"]'));
 const state=await (await page.request.get(base+API)).json(),id=state.generations[0].id;
 assert.match(id,/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i);
 assert.ok(!before.generations.some(g=>g.id===id),'refresh must create an owned generation');
 // Register immediately so a failed wait also cancels and deletes its partial build.
 generations.push(id);await poll(page,id,seeds);await page.goto(base+PAGE);return id;
}
async function activate(page,id){await submit(page,page.locator(`[data-generation="${id}"] form[data-action="activate"]`));assert.equal((await (await page.request.get(base+API)).json()).active_generation,id);}
async function feed(context,seed){const response=await context.request.get(base+`/api/v1/books/${seed}/recommendations?limit=20`);assert.equal(response.status(),200);return response.json();}
(async()=>{
 const stamp=Date.now();let browser,actor;const books=[],generations=[],backgroundBooks=[];
 try {
  assert.equal(sql('SELECT count(*) FROM _sqlx_migrations WHERE version=202611198437 AND success'),'1');
  browser=await chromium.launch({headless:true});
  const admin=await browser.newContext({viewport:{width:1440,height:900},javaScriptEnabled:false});
  const page=await admin.newPage();
  await page.goto(base+'/register');
  await page.getByLabel('\u041e\u0442\u043e\u0431\u0440\u0430\u0436\u0430\u0435\u043c\u043e\u0435 \u0438\u043c\u044f').fill('Contextual model operator');
  const email=`contextual-browser-${stamp}@tor-ci.example`;
  await page.getByLabel('\u042d\u043b\u0435\u043a\u0442\u0440\u043e\u043d\u043d\u0430\u044f \u043f\u043e\u0447\u0442\u0430').fill(email);
  await page.getByLabel('\u041f\u0430\u0440\u043e\u043b\u044c (12\u2013128 \u0441\u0438\u043c\u0432\u043e\u043b\u043e\u0432)').fill('ContextualBrowserPassword2026');
  await page.getByRole('button',{name:'\u0421\u043e\u0437\u0434\u0430\u0442\u044c \u0443\u0447\u0451\u0442\u043d\u0443\u044e \u0437\u0430\u043f\u0438\u0441\u044c'}).click();await page.waitForURL('**/me');
  actor=number(`SELECT id FROM users WHERE email_normalized='${email}'`);
  await privateResponse(await page.goto(base+PAGE),403);
  sql(`INSERT INTO account_roles(user_id,role) VALUES(${actor},'administrator')`);
  const optional=optionalSnapshot(actor);
  for(let i=0;i<5;i++)books.push(number(`INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id,genres,tags) VALUES('ci:contextual:${stamp}:${i}','ctx-${stamp}','Contextual <script>safe</script> ${i}','public','basic',${actor},ARRAY['${i===3?'other':'system'}'],ARRAY['${i===3?'other':'cultivation'}']) RETURNING id`));
  const [seed,manual,content,unrelated,hidden]=books;
  sql(`UPDATE books SET admin_hold=true WHERE id=${hidden}; INSERT INTO book_recommendation_audiences(book_id,audience) VALUES(${seed},'general')`);
  // Manual selection uses the existing real B form, not an invented HTTP fixture.
  await page.goto(base+`/me/books/${seed}/recommendations`);
  await page.locator('input[name="related"]').fill(String(manual));
  await page.getByRole('button',{name:'\u0421\u043e\u0445\u0440\u0430\u043d\u0438\u0442\u044c',exact:true}).click();await page.waitForURL(base+`/me/books/${seed}/recommendations`);
  // Reproduce the full-CI catalogue even in standalone REC: these owned public
  // seeds need >40s just for the unchanged 250ms worker pacing. Their unique
  // language and empty labels do not change the recommendations asserted below.
  const extra=sql(`BEGIN; LOCK TABLE books IN SHARE ROW EXCLUSIVE MODE;
   INSERT INTO books(id,source_identity,target_language,title,visibility,access_level)
   SELECT (SELECT coalesce(max(id),0) FROM books)+n,'ci:contextual-background:${stamp}:'||n,
    'ctx-background-${stamp}','Contextual background '||n,'public','basic'
   FROM generate_series(1,161) n RETURNING id; COMMIT;`);
  for(const value of extra.split('\n')){const id=Number(value);assert.ok(Number.isSafeInteger(id)&&id>0);backgroundBooks.push(id);}
  assert.equal(backgroundBooks.length,161);
  assert.equal(number(`SELECT count(*) FROM rec_materializable_books WHERE id IN (${backgroundBooks.join(',')})`),161);
  const first=await build(page,generations);
  // One complete large build proves the regression; the rollback build need
  // not repeat the artificial delay. Retain IDs for idempotent failure cleanup.
  sql(`DELETE FROM books WHERE id IN (${backgroundBooks.join(',')})`);
  let state=await (await page.request.get(base+API)).json();assert.equal(state.active_generation,null);
  await activate(page,first);
  const anonymous=await browser.newContext({viewport:{width:390,height:844}});
  let result=await feed(anonymous,seed);
  assert.equal(result.generation_id,first);assert.equal(result.personalized,false);
  assert.deepEqual(result.items.map(i=>Number(i.book_id)),[manual,content]);
  assert.equal(result.items[0].reason,'manually_related');
  assert.ok(result.items.every(i=>i.algorithm_version==='metadata_manual_v1' && !Object.hasOwn(i,'ticket')));
  for(const width of [390,1440]){
   await page.setViewportSize({width,height:900});await page.goto(base+PAGE);
   // Layout proof uses a separate JS-capable reader for DOM width measurements.
   const visual=await browser.newContext({viewport:{width,height:900},storageState:await admin.storageState()});const vp=await visual.newPage();
   await privateResponse(await vp.goto(base+PAGE));assert.equal(await vp.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
   await vp.screenshot({path:path.join(shots,`contextual-admin-${width}.png`),fullPage:true,animations:'disabled'});
   await vp.locator(`[data-generation="${first}"]`).screenshot({path:path.join(shots,`contextual-version-${width}.png`)});
   await vp.goto(base+`/books/${seed}`);await assertNoControls(vp);
   const block=vp.locator('[data-rec-panel]');assert.equal(await block.count(),1);
   await block.screenshot({path:path.join(shots,`contextual-feed-${width}.png`)});
   await visual.close();
  }
  // Real explicit user choice still filters only its own list.
  await page.goto(base+`/books/${seed}`);
  const card=page.locator('[data-rec-card]').filter({has:page.locator(`a[href="/books/${manual}"]`)});
  await card.locator('form').getByRole('button').click();await page.waitForURL('**/me/recommendations');
  assert.ok(!(await feed(admin,seed)).items.some(i=>Number(i.book_id)===manual));
  assert.ok((await feed(anonymous,seed)).items.some(i=>Number(i.book_id)===manual));
  // Removing a manual link is live; content may still qualify, but never with a stale manual reason.
  await page.goto(base+`/me/books/${seed}/recommendations`);await page.locator('input[name="related"]').fill('');
  await page.getByRole('button',{name:'\u0421\u043e\u0445\u0440\u0430\u043d\u0438\u0442\u044c',exact:true}).click();await page.waitForURL(base+`/me/books/${seed}/recommendations`);
  assert.ok((await feed(anonymous,seed)).items.every(i=>i.reason==='similar_by_content'));
  sql(`UPDATE books SET admin_hold=true WHERE id=${content}`);
  assert.ok(!(await feed(anonymous,seed)).items.some(i=>Number(i.book_id)===content));
  const second=await build(page,generations);await activate(page,second);
  await submit(page,page.locator('form[data-action="rollback"]'));assert.equal((await (await page.request.get(base+API)).json()).active_generation,first);
  await submit(page,page.locator('form[data-action="hold"]'));assert.equal((await feed(anonymous,seed)).generation_id,null);
  const rev=await revision(page);const csrf=(await admin.cookies()).find(c=>c.name==='tor-dev-csrf').value;
  const forged=await page.request.post(base+API,{headers:{Origin:base,'x-csrf-token':'wrong'},data:{action:'refresh',expected_revision:rev,generation:null}});assert.equal(forged.status(),403);
  assert.equal(await revision(page),rev);
  const legacy=await page.request.post(base+'/api/v1/admin/recommendations/control',{headers:{Origin:base,'x-csrf-token':csrf},data:{action:'refresh',expected_revision:rev}});assert.equal(legacy.status(),410);
  sql(`UPDATE account_roles SET is_active=false WHERE user_id=${actor} AND role='administrator'`);
  await privateResponse(await page.goto(base+PAGE),403);
  const revoked=await page.request.post(base+API,{headers:{Origin:base,'x-csrf-token':csrf},data:{action:'refresh',expected_revision:rev,generation:null}});assert.equal(revoked.status(),403);
  assert.deepEqual(optionalSnapshot(actor),optional);
  assert.equal(number(`SELECT count(*) FROM rec_subjects WHERE user_id=${actor}`),0);
  assert.ok((await admin.cookies()).every(c=>['tor-dev-session','tor-dev-csrf'].includes(c.name)));
  await anonymous.close();await admin.close();
  console.log('PASS contextual metadata/manual native forms, actual worker, activation/rollback, current ACL/P1 and no individual collection');
 }finally{
  if(browser)await browser.close();
  sql('UPDATE rec_runtime_policy SET active_generation=NULL,previous_generation=NULL,hold=true,rollout_percent=0,revision=revision+1 WHERE singleton');
  if(generations.length){
   const owned=generations.map(id=>`'${id}'`).join(',');
   sql(`BEGIN;
    UPDATE rec_generations SET status='rejected',finished_at=now()
     WHERE id IN (${owned}) AND model_version='metadata_manual_v1' AND status='building';
    DELETE FROM rec_generations WHERE id IN (${owned}); COMMIT;`);
  }
  if(backgroundBooks.length)sql(`DELETE FROM books WHERE id IN (${backgroundBooks.join(',')})`);
  if(actor)sql(`DELETE FROM account_roles WHERE user_id=${actor} AND role='administrator'`);
  if(books.length)sql(`UPDATE books SET admin_hold=true WHERE id IN (${books.join(',')})`);
 }
})().catch(error=>{console.error(error);process.exitCode=1;});

