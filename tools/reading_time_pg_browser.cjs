'use strict';
// Actual no-JS forms and real C commands; only the readonly clock/report copy is synthetic.
const {chromium}=require('playwright');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const {Fixture,sql,scoped,scalar,clock,owner,schema,base,writer,shots,FALL,SPRING}=require('./reading_time_pg_fixture.cjs');
fs.mkdirSync(shots,{recursive:true});
const route=base+'/admin/audience?view=reading_hours&days=1&time_zone=UTC';
async function privateResponse(response,expected=200){
  assert.equal(response.status(),expected);
  const h=await response.allHeaders();
  assert.equal(h['cache-control'],'no-store');
  assert.match(h['x-robots-tag'],/noindex/);assert.match(h['x-robots-tag'],/nofollow/);
}
async function register(page,email){
  await page.goto(writer+'/register');
  await page.getByLabel('Отображаемое имя').fill('Часы чтения');
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('ReadingTimeBrowserPassword2026');
  await page.getByRole('button',{name:'Создать учётную запись',exact:true}).click();
  await page.waitForURL('**/me');
  return scalar("SELECT id FROM users WHERE email_normalized='"+email+"'");
}
async function complete(context,page,pair){
  assert.equal((await page.goto(writer+'/reader/'+pair.chapter)).status(),200);
  const csrf=(await context.cookies()).find(cookie=>cookie.name==='tor-dev-csrf')?.value;assert.ok(csrf);
  const state=await context.request.get(writer+'/api/v1/me/progress/free?chapter_id='+pair.chapter);
  assert.equal(state.status(),200);const revision=Number((await state.json()).revision);
  async function save(offset,expectedRevision,status=200){
    const response=await context.request.put(writer+'/api/v1/me/progress/free',{
      headers:{'Content-Type':'application/json','X-CSRF-Token':csrf,Origin:writer},
      data:{chapter_id:String(pair.chapter),version_id:String(pair.version),block_id:1,char_offset:offset,expected_revision:expectedRevision},
    });
    assert.equal(response.status(),status,'actual C response: '+await response.text());
    return status===200?Number((await response.json()).revision):null;
  }
  const started=await save(3,revision);const ended=await save(13,started);
  await save(13,ended,409);
  for(const table of ['reader_chapter_starts','reader_chapter_completions']){
    assert.equal(scalar('SELECT count(*) FROM '+table+' WHERE chapter_id='+pair.chapter),1);
  }
  return revision;
}
async function metric(locator,name){
  return (await locator.locator('[data-metric="'+name+'"]').first().innerText()).trim();
}
async function totals(page,period){
  const p=page.getByTestId('reading-time-'+period);
  return {starts:await metric(p,'chapter-starts'),completions:await metric(p,'chapter-completions'),hours:Number(await metric(p,'utc-hours'))};
}
async function submit(page,expected){
  const [response]=await Promise.all([
    page.waitForNavigation({waitUntil:'domcontentloaded'}),
    page.locator('form[action="/admin/audience"]').getByRole('button',{name:'Показать',exact:true}).click(),
  ]);
  await privateResponse(response);
  for(const [key,value] of Object.entries(expected))assert.equal(new URL(page.url()).searchParams.get(key),String(value));
}
async function snapshot(page,name,target){
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'no horizontal page overflow');
  if(target)await target.scrollIntoViewIfNeeded();
  const bytes=target?await target.screenshot({path:path.join(shots,name),animations:'disabled'}):
    await page.screenshot({path:path.join(shots,name),animations:'disabled'});
  if(process.env.TOR_BROWSER_PROOF_IMAGES==='1'){
    console.log('FLOW_VISUAL_BEGIN '+JSON.stringify({path:name,mime:'image/png',bytes:bytes.length}));
    const encoded=bytes.toString('base64');
    for(let at=0;at<encoded.length;at+=3000)console.log('FLOW_VISUAL_DATA '+encoded.slice(at,at+3000));
    console.log('FLOW_VISUAL_END');
  }
}
(async()=>{
  let browser,f;
  const receipt={javaScriptEnabled:false,viewports:[390,1440],source:'actual C HTTP commands copied into isolated readonly report schema',clock:'fixed fixture clock; production report SQL'};
  try{
    f=new Fixture();f.seedBook();
    browser=await chromium.launch({headless:true});
    const admin=await browser.newContext({javaScriptEnabled:false,viewport:{width:1440,height:900}});
    const reader=await browser.newContext({javaScriptEnabled:false});
    const guest=await browser.newContext({javaScriptEnabled:false});
    const page=await admin.newPage();const rp=await reader.newPage();const gp=await guest.newPage();
    const errors=[];page.on('pageerror',error=>errors.push(String(error)));
    f.actors.push(await register(page,f.emails[0]),await register(rp,f.emails[1]));
    await privateResponse(await gp.goto(route),401);
    await privateResponse(await page.goto(route),403);
    sql("INSERT INTO account_roles(user_id,role) VALUES("+f.actors[0]+",'administrator')");
    await privateResponse(await page.goto(base+'/admin/audience'));
    await page.getByLabel('Представление',{exact:true}).selectOption('reading_hours');
    await page.getByLabel('Длительность периода',{exact:true}).selectOption('1');
    await page.getByLabel('Часовой пояс',{exact:true}).selectOption('UTC');
    await submit(page,{view:'reading_hours',days:1,time_zone:'UTC'});
    assert.deepEqual(await totals(page,'current'),{starts:'0',completions:'0',hours:24});
    assert.equal(await page.getByTestId('audience-observed-current').count(),0);
    for(const [index,pair] of f.chapters.entries()){
      const revision=await complete(reader,rp,pair);
      if(index>0)assert.ok(revision>0,'same-book revision must survive the next chapter');
    }
    receipt.realChapterStarts=scalar('SELECT count(*) FROM reader_chapter_starts WHERE book_id='+f.book);
    receipt.realChapterCompletions=scalar('SELECT count(*) FROM reader_chapter_completions WHERE book_id='+f.book);
    assert.equal(receipt.realChapterStarts,5);assert.equal(receipt.realChapterCompletions,5);
    const source=f.publicFingerprint();f.copy();
    assert.equal(f.publicFingerprint(),source,'historical placement never changes the actual writer facts');
    const reportBefore=f.reportFingerprint();
    await privateResponse(await page.goto(route));
    receipt.current=await totals(page,'current');receipt.previous=await totals(page,'previous');
    assert.deepEqual(receipt.current,{starts:'2',completions:'2',hours:24});
    assert.deepEqual(receipt.previous,{starts:'1',completions:'1',hours:24});
    const anchor=page.getByTestId('reading-time-anchor');
    receipt.until=Number(await anchor.getAttribute('data-until'));
    assert.equal(receipt.until,Date.parse('2026-10-25T04:00:00Z')/1000);
    // The first account/book pair is outside the report period; later chapters still count.
    await privateResponse(await page.goto(base+'/admin/audience/reading?days=90&tag='+encodeURIComponent(f.tag)));
    const pairRow=page.locator('[data-testid="canonical-row"][data-book-id="'+f.book+'"]');
    assert.equal(await metric(pairRow,'current'),'0');
    receipt.firstAccountBookPairs=0;
    await privateResponse(await page.goto(route));
    for(const days of [7,30,90,1]){
      await page.getByLabel('Длительность периода',{exact:true}).selectOption(String(days));
      await submit(page,{view:'reading_hours',days});
      const current=await totals(page,'current');const previous=await totals(page,'previous');
      assert.equal(current.hours,days*24);assert.equal(previous.hours,days*24);
      assert.equal(current.completions,days===1?'2':'3');assert.equal(previous.completions,days===1?'1':'0');
      assert.equal(await page.locator('[data-reading-epoch]').count(),2*Math.min(days*24,168));
    }
    const utcSaturday=page.getByTestId('reading-time-weekday-profile').locator('[data-reading-weekday="5"] [data-period="current"]');
    assert.equal(await metric(utcSaturday,'chapter-starts'),'1','UTC Saturday contains the late start');
    for(const zone of ['Europe/Moscow','Asia/Yekaterinburg','Europe/Amsterdam']){
      await page.getByLabel('Часовой пояс',{exact:true}).selectOption(zone);
      await submit(page,{time_zone:zone,days:1,view:'reading_hours'});
      assert.deepEqual(await totals(page,'current'),receipt.current);
      assert.equal(Number(await page.getByTestId('reading-time-anchor').getAttribute('data-until')),receipt.until);
      const saturday=page.getByTestId('reading-time-weekday-profile').locator('[data-reading-weekday="5"] [data-period="current"]');
      const sunday=page.getByTestId('reading-time-weekday-profile').locator('[data-reading-weekday="6"] [data-period="current"]');
      assert.equal(await metric(saturday,'chapter-starts'),'0');assert.equal(await metric(sunday,'chapter-starts'),'2');
    }
    const repeated=page.getByTestId('reading-time-current-hours').locator('tr').filter({hasText:'2026-10-25 02:00'});
    assert.equal(await repeated.count(),2);
    receipt.repeatedOffsets=await repeated.evaluateAll(rows=>rows.map(row=>Number(row.dataset.offsetMinutes)));
    assert.deepEqual(receipt.repeatedOffsets,[120,60]);
    const repeatedBin=page.getByTestId('reading-time-hour-profile').locator('[data-reading-hour="2"] [data-period="current"]');
    assert.equal(await repeatedBin.getAttribute('data-utc-hours'),'2');assert.equal(await metric(repeatedBin,'chapter-completions'),'2');
    for(const width of [390,1440]){
      await page.setViewportSize({width,height:width===390?844:900});
      await privateResponse(await page.goto(base+'/admin/audience?view=reading_hours&days=1&time_zone=Europe%2FAmsterdam'));
      await snapshot(page,'reading-time-form-'+width+'.png');
      await snapshot(page,'reading-time-hours-'+width+'.png',page.getByTestId('reading-time-hour-profile').locator('xpath=ancestor::section[1]'));
      await snapshot(page,'reading-time-weekdays-'+width+'.png',page.getByTestId('reading-time-weekday-profile').locator('xpath=ancestor::section[1]'));
      const details=page.getByTestId('reading-time-current-details');
      await details.locator('summary').click();
      await details.locator('tr').filter({hasText:'2026-10-25 02:00'}).first().scrollIntoViewIfNeeded();
      await snapshot(page,'reading-time-dst-'+width+'.png',details);
    }
    assert.equal(f.reportFingerprint(),reportBefore,'report GET must be readonly');
    assert.equal(f.publicFingerprint(),source);
    const html=await page.content();
    for(const secret of [...f.emails,'PRIVATE-TIME-CONTENT','Audience text','user_id','book_id'])assert.ok(!html.includes(secret));
    assert.equal(await page.locator('[data-flow-proof]').count(),0);
    clock(SPRING);await privateResponse(await page.reload());
    const absent=page.getByTestId('reading-time-hour-profile').locator('[data-reading-hour="2"] [data-period="current"]');
    assert.equal(await absent.getAttribute('data-utc-hours'),'0');
    assert.equal(await absent.getByTestId('reading-time-no-slot').innerText(),'Не входит в выбранный период');
    assert.equal(await absent.locator('[data-metric]').count(),0);
    receipt.springMissingHour=true;
    clock(FALL);
    const missing=f.chapters[2].chapter;
    scoped('DELETE FROM outbox_events WHERE id=(SELECT source_event_id FROM reader_chapter_completions WHERE chapter_id='+missing+')');
    await privateResponse(await page.reload());
    assert.equal((await totals(page,'current')).starts,'Неизвестно');
    assert.equal((await totals(page,'previous')).completions,'1');
    assert.equal(await page.getByTestId('reading-time-comparison-unknown').count(),1);
    receipt.missingProof='unknown';
    scoped('REVOKE SELECT ON '+schema+'.outbox_events FROM tor_app');
    await privateResponse(await page.reload(),503);
    assert.equal(await page.getByTestId('reading-time-unavailable').count(),1);
    assert.equal(await page.locator('[data-metric]').count(),0);
    scoped('GRANT SELECT ON '+schema+'.outbox_events TO tor_app');
    await privateResponse(await page.reload());
    for(const query of ['view=invalid','view=reading_hours&time_zone=Asia%2FKolkata','view=reading_hours&time_zone=Asia%2FKathmandu']){
      await privateResponse(await page.goto(base+'/admin/audience?'+query),400);
    }
    sql('UPDATE account_roles SET is_active=false WHERE user_id='+f.actors[0]);
    await privateResponse(await page.goto(route),403);
    sql('UPDATE account_roles SET is_active=true WHERE user_id='+f.actors[0]);
    sql('UPDATE sessions SET revoked_at=now() WHERE user_id='+f.actors[0]);
    await privateResponse(await page.goto(route),401);
    assert.deepEqual(errors,[]);
    receipt.sourceFactsUnchanged=true;
    console.log('READING_TIME_BODY_RECEIPT '+JSON.stringify(receipt));
    console.log('READING_TIME_BROWSER_PASS actual C start/complete/replay; next chapter old pair; native no-JS view/1/7/30/90/four zones; closed windows; UTC weekday shift; DST repeated/missing hour; proof unknown/source503; readonly/live ACL; 390/1440');
  }finally{
    if(browser)await browser.close();
    if(f)f.cleanup();
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
