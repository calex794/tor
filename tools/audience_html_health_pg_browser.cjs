'use strict';
// Real Chromium, native account/report forms, and a fresh disposable PostgreSQL.
// Synthetic historical aggregates/ACKs exercise presentation, not past traffic.
const {chromium} = require('playwright');
const {execFileSync} = require('node:child_process');
const {randomBytes,createHash} = require('node:crypto');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const db = process.env.APP_URL || '';
const base = process.env.TOR_AUDIENCE_BASE || 'http://127.0.0.1:3108';
const shots = process.env.TOR_AUDIENCE_SHOTS || '/tmp/audience-health-shots';
const parsed = new URL(db);
if (parsed.protocol !== 'postgresql:' || parsed.hostname !== '127.0.0.1'
    || parsed.port !== '5432' || parsed.pathname !== '/tor_ci'
    || !/^http:\/\/127\.0\.0\.1:\d+$/.test(base)) {
  throw new Error('HTML health browser acceptance requires disposable loopback tor_ci');
}
fs.mkdirSync(shots,{recursive:true});
function sql(statement) {
  return execFileSync('psql',['-XAtq','-v','ON_ERROR_STOP=1','--dbname',db,'--command',statement],{
    encoding:'utf8',env:{...process.env,PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'},
  }).trim();
}
function integer(statement) {
  const raw=sql(statement);assert.match(raw,/^\d+$/);return Number(raw);
}
async function privateResponse(response,status=200) {
  assert.equal(response.status(),status);
  const headers=await response.allHeaders();
  assert.equal(headers['cache-control'],'no-store');
  assert.match(headers['x-robots-tag'],/noindex/);
  assert.match(headers['x-robots-tag'],/nofollow/);
  assert.match(headers['content-security-policy'],/script-src 'none'/);
}
async function total(page,period) {
  return (await page.getByTestId('audience-observed-'+period)
    .locator('[data-metric="observed-total"]').innerText()).trim();
}
async function quality(page,current,previous,days=1) {
  const report=page.getByTestId('audience-durable-quality');
  assert.equal(await report.count(),1);
  assert.match(await report.innerText(),/зарегистрированных процессов/);
  for (const [period,state] of [['current',current],['previous',previous]]) {
    assert.equal(await report.locator('[data-health-period="'+period+'"]')
      .getAttribute('data-health-state'),state);
  }
  const bounds={};
  for (const period of ['current','previous']) {
    bounds[period]=await page.getByTestId('audience-observed-'+period)
      .locator('time[datetime]').evaluateAll(nodes=>nodes.map(node=>Date.parse(node.dateTime)/1000));
    assert.equal(bounds[period].length,2);
    assert.equal(bounds[period][0]%3600,0);
    assert.equal(bounds[period][1]-bounds[period][0],days*86400);
  }
  assert.equal(bounds.previous[1],bounds.current[0]);
  assert.ok(bounds.current[1]<=Date.now()/1000);
  return bounds;
}
async function shot(page,name) {
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),
    'health report must not overflow the viewport');
  const bytes=await page.getByTestId('audience-durable-quality').screenshot({path:path.join(shots,name)});
  if (process.env.TOR_FLOW_EMIT_SCREENSHOTS==='1') {
    console.log('FLOW_PNG_BEGIN '+JSON.stringify({name,sha256:createHash('sha256').update(bytes).digest('hex'),bytes:bytes.length}));
    const encoded=bytes.toString('base64');
    for(let i=0;i<encoded.length;i+=3000)console.log('FLOW_PNG_DATA '+encoded.slice(i,i+3000));
    console.log('FLOW_PNG_END');
  }
}
(async()=>{
  const run=Date.now()+'-'+randomBytes(4).toString('hex');
  const email='html-health-browser-'+run+'@tor-ci.example';
  let browser,administrator,epoch,bucket;
  try {
    browser=await chromium.launch({headless:true});
    const context=await browser.newContext({javaScriptEnabled:false,viewport:{width:1440,height:900}});
    const page=await context.newPage();
    const guest=await browser.newContext({javaScriptEnabled:false});
    const anonymous=await guest.newPage();
    await privateResponse(await anonymous.goto(base+'/admin/audience'),401);
    assert.equal(await anonymous.locator('[data-metric]').count(),0);
    await page.goto(base+'/register');
    await page.getByLabel('Отображаемое имя').fill('Проверка качества HTML');
    await page.getByLabel('Электронная почта').fill(email);
    await page.getByLabel('Пароль (12–128 символов)').fill('HtmlHealthBrowserPassword2026');
    await page.getByRole('button',{name:'Создать учётную запись'}).click();
    await page.waitForURL('**/me');
    administrator=integer("SELECT id FROM users WHERE email_normalized='"+email+"'");
    await privateResponse(await page.goto(base+'/admin/audience'),403);
    sql("INSERT INTO account_roles(user_id,role) VALUES("+administrator+",'administrator')");
    await page.goto(base+'/me');
    await page.getByRole('link',{name:'Аудитория',exact:true}).click();
    await page.waitForURL(base+'/admin/audience');
    await privateResponse(await page.goto(base+'/admin/audience?days=1'));
    const bounds=await quality(page,'unknown','unknown');
    const anchor=bounds.current[1];
    assert.equal(await total(page,'previous'),'Нет измерения');
    // This script runs before other fixture suites on a fresh database.
    assert.equal(integer("SELECT count(*) FROM ril_flow_observed_hourly WHERE bucket>=to_timestamp("+(anchor-172800)+") AND bucket<to_timestamp("+anchor+")"),0);
    bucket=anchor-3600;
    sql("INSERT INTO ril_flow_observed_hourly VALUES(to_timestamp("+bucket+"),0,3,7)");
    await privateResponse(await page.reload());
    assert.equal(await total(page,'current'),'7','known saved count remains available while completeness is unknown');
    for(const width of [390,1440]) {
      await page.setViewportSize({width,height:width===390?844:900});
      await privateResponse(await page.reload());
      await quality(page,'unknown','unknown');
      await shot(page,'audience-html-health-unknown-'+width+'.png');
    }
    // A fixture can certify all old hours, but an existing unresolved worker
    // overlapping them must still prevent a full state. Never rewrite that worker.
    assert.equal(integer("SELECT count(*) FROM ril_flow_health_epochs WHERE NOT retired AND started_ms<"+(anchor*1000)),0,
      'server startup crossed the report hour; rerun the fresh-database scenario');
    epoch=integer("INSERT INTO ril_flow_health_epochs(started_ms,confirmed_ms,lease_until_ms,retired) VALUES("+
      ((anchor-172800)*1000)+","+(anchor*1000)+","+(anchor*1000)+",true) RETURNING epoch");
    const before=sql("SELECT generation,confirmed_ms,lease_until_ms,retired FROM ril_flow_health_epochs WHERE epoch="+epoch);
    for(const width of [390,1440]) {
      await page.setViewportSize({width,height:width===390?844:900});
      await privateResponse(await page.reload());
      await quality(page,'confirmed','confirmed');
      assert.equal(await total(page,'current'),'7');
      assert.equal(await total(page,'previous'),'0','empty fully acknowledged period can be zero');
      assert.equal((await page.locator('[data-health-period="previous"] [data-metric="html-confirmed-hours"]').innerText()).trim(),'24 из 24');
      await shot(page,'audience-html-health-confirmed-'+width+'.png');
    }
    assert.equal(sql("SELECT generation,confirmed_ms,lease_until_ms,retired FROM ril_flow_health_epochs WHERE epoch="+epoch),before,
      'report GET must not advance a worker acknowledgement');
    for(const days of [7,30,90]) {
      await page.locator('select[name="days"]').selectOption(String(days));
      await page.locator('form[action="/admin/audience"]').getByRole('button').click();
      await page.waitForURL(url=>url.searchParams.get('days')===String(days));
      await quality(page,'unknown','unknown',days);
      assert.equal(await total(page,'previous'),'Нет измерения');
    }
    await page.locator('select[name="days"]').selectOption('1');
    await page.locator('select[name="time_zone"]').selectOption('Europe/Amsterdam');
    await page.locator('form[action="/admin/audience"]').getByRole('button').click();
    await page.waitForURL(url=>url.searchParams.get('days')==='1'&&url.searchParams.get('time_zone')==='Europe/Amsterdam');
    assert.deepEqual(await quality(page,'confirmed','confirmed'),bounds,'display timezone does not change the closed UTC windows');
    sql("INSERT INTO ril_flow_health_hourly(epoch,bucket_ms,had_loss) VALUES("+epoch+","+
      ((anchor-25*3600)*1000)+",true)");
    await privateResponse(await page.reload());
    await quality(page,'confirmed','unknown');
    assert.equal(await total(page,'previous'),'Нет измерения');
    assert.equal((await page.locator('[data-health-period="previous"] [data-metric="html-loss-hours"]').innerText()).trim(),'1');
    assert.ok(!(await page.content()).includes(email),'health page never exposes the actor identity');
    sql("UPDATE account_roles SET is_active=false WHERE user_id="+administrator+" AND role='administrator'");
    await privateResponse(await page.reload(),403);
    assert.equal(await page.getByTestId('audience-durable-quality').count(),0);
    sql("UPDATE sessions SET revoked_at=now() WHERE user_id="+administrator);
    await privateResponse(await page.reload(),401);
    console.log('FLOW HTML health browser PASS: no JavaScript, native 1/7/30/90 and timezone forms, 390/1440, unknown vs acknowledged zero, persisted losses, readonly GET, live authority revocation');
  } finally {
    if(browser)await browser.close();
    if(epoch)sql("BEGIN; DELETE FROM ril_flow_health_hourly WHERE epoch="+epoch+"; DELETE FROM ril_flow_health_epochs WHERE epoch="+epoch+"; COMMIT");
    if(bucket)sql("DELETE FROM ril_flow_observed_hourly WHERE bucket=to_timestamp("+bucket+") AND surface_code=0 AND acquisition_hint=3");
    if(administrator)sql("BEGIN; DELETE FROM account_roles WHERE user_id="+administrator+"; DELETE FROM users WHERE id="+administrator+"; COMMIT");
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
