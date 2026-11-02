// Active A acceptance on the real Rust server and disposable PostgreSQL.
const {chromium}=require('playwright');
const assert=require('node:assert/strict');
const fs=require('node:fs/promises');
const path=require('node:path');
const {createHash}=require('node:crypto');
const base='http://127.0.0.1:3098';
const password='OriginalSecurePassword2026';
const replacement='ReplacementSecurePassword2026';
const nonce=Date.now().toString();
const email=`a-browser-${nonce}@example.invalid`;
const shots=process.env.TOR_COVER_BROWSER_SHOTS;

async function register(browser,mail,name) {
  const context=await browser.newContext({viewport:{width:1440,height:1000}});
  const page=await context.newPage();
  await page.goto(`${base}/register`);
  await page.locator('[name=display_name]').fill(name);
  await page.locator('[name=email]').fill(mail);
  await page.locator('[name=password]').fill(password);
  await Promise.all([page.waitForURL(`${base}/me`),page.locator('form[action="/register"] button').click()]);
  return {context,page};
}

async function login(browser,mail,secret) {
  const context=await browser.newContext();const page=await context.newPage();
  await page.goto(`${base}/login`);
  await page.locator('[name=email]').fill(mail);
  await page.locator('[name=password]').fill(secret);
  await Promise.all([page.waitForURL(`${base}/me`),page.locator('form[action="/login"] button').click()]);
  return {context,page};
}

(async()=>{
  const browser=await chromium.launch({headless:true});
  try {
    const alice=await register(browser,email,'A browser reader');
    const bob=await register(browser,`b-browser-${nonce}@example.invalid`,'B browser reader');
    const second=await login(browser,email,password);
    const guest=await browser.newContext();
    assert.equal((await guest.request.get(`${base}/api/v1/me`)).status(),401);
    await alice.page.goto(`${base}/me/settings`);
    await alice.page.locator('[name=display_name]').fill('Updated A reader');
    await alice.page.locator('[name=handle]').fill(`reader_${nonce}`);
    await alice.page.locator('[name=adult_enabled]').selectOption('true');
    await Promise.all([alice.page.waitForResponse(r=>r.request().method()==='POST'&&r.url()===`${base}/me/settings`),alice.page.locator('form[action="/me/settings"] button').click()]);
    await alice.page.waitForLoadState('networkidle');
    let profile=await (await alice.context.request.get(`${base}/api/v1/me`)).json();
    assert.equal(profile.display_name,'Updated A reader');assert.equal(profile.adult_enabled,true);
    assert.equal(profile.revision,1);assert(!('password_hash' in profile));
    const stale={expected_revision:0,display_name:'Stale request',csrf_token:profile.csrf_token};
    assert.equal((await alice.context.request.patch(`${base}/api/v1/me`,{headers:{Origin:base},data:stale})).status(),409);
    assert.equal((await alice.context.request.patch(`${base}/api/v1/me`,{headers:{Origin:'https://evil.invalid'},data:{...stale,expected_revision:1}})).status(),403);
    assert.equal((await alice.context.request.patch(`${base}/api/v1/me`,{headers:{Origin:base},data:{...stale,user_id:'1'}})).status(),422);
    assert.equal((await (await bob.context.request.get(`${base}/api/v1/me`)).json()).display_name,'B browser reader');
    await fs.mkdir(shots,{recursive:true});
    for(const width of [1440,390]) {
      await alice.page.setViewportSize({width,height:1000});
      assert(await alice.page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
      await alice.page.screenshot({path:path.join(shots,`account-settings-${width}.png`),fullPage:true});
    }
    await alice.page.goto(`${base}/me/notification-settings`);
    await alice.page.locator('[name=delivery_mode][value=daily_digest]').check();
    await Promise.all([alice.page.waitForResponse(r=>r.request().method()==='POST'&&r.url()===`${base}/me/notification-settings`),alice.page.locator('form button').click()]);
    await alice.page.waitForLoadState('networkidle');
    assert.equal((await (await alice.context.request.get(`${base}/api/v1/me`)).json()).delivery_settings,'daily_digest');
    await alice.page.goto(`${base}/me/security/sessions`);
    assert(await alice.page.locator('form[action="/me/security/sessions"]').count()>=2);
    assert(await alice.page.getByText('это устройство').count()>=1);
    const remote=alice.page.locator('li.me-now-item').filter({hasNotText:'это устройство'}).first();
    await remote.locator('input[name=current_password]').fill(password);
    const [sessionRevoked]=await Promise.all([alice.page.waitForResponse(r=>r.request().method()==='POST'&&r.url()===`${base}/me/security/sessions`),remote.locator('button').click()]);
    assert.equal(sessionRevoked.status(),303);
    await alice.page.waitForLoadState('networkidle');
    assert.equal((await second.context.request.get(`${base}/api/v1/me`)).status(),401);
    assert.equal((await alice.context.request.get(`${base}/api/v1/me`)).status(),200);
    await alice.page.screenshot({path:path.join(shots,'account-active-sessions.png'),fullPage:true});
    await alice.page.goto(`${base}/forgot-password`);
    const csrf=await alice.page.locator('[name=csrf_token]').inputValue();
    const known=await alice.context.request.post(`${base}/forgot-password`,{headers:{Origin:base},form:{email,csrf_token:csrf}});
    const unknown=await alice.context.request.post(`${base}/forgot-password`,{headers:{Origin:base},form:{email:`missing-${nonce}@example.invalid`,csrf_token:csrf}});
    assert.equal(known.status(),200);assert.equal(unknown.status(),200);assert.equal(await known.text(),await unknown.text());
    const mailbox=path.join(process.env.TOR_RESET_TEST_MAIL_DIR,createHash('sha256').update(email).digest('hex'));
    let message='';
    for(let attempt=0;attempt<100;attempt++) {
      try {message=await fs.readFile(mailbox,'utf8');if(message.includes('/reset-password#'))break;}catch(error){if(error.code!=='ENOENT')throw error;}
      await new Promise(resolve=>setTimeout(resolve,100));
    }
    const match=message.match(/\/reset-password#([A-Za-z0-9_-]{43})/);assert(match,'test MTA must receive reset proof');
    const code=match[1];let leaked=false;
    alice.page.on('request',request=>{if(request.url().includes(code))leaked=true;});
    await alice.page.goto(`${base}/reset-password#${code}`);
    assert.equal(new URL(alice.page.url()).hash,'');
    assert.equal(await alice.page.locator('[name=token]').inputValue(),code);
    await alice.page.locator('[name=new_password]').fill(replacement);
    await alice.page.locator('[name=confirm_password]').fill(replacement);
    await Promise.all([alice.page.waitForURL(`${base}/login`),alice.page.locator('form[action="/reset-password"] button').click()]);
    assert(!leaked,'reset proof must never enter network URLs');
    assert.equal((await second.context.request.get(`${base}/api/v1/me`)).status(),401);
    assert.equal((await bob.context.request.get(`${base}/api/v1/me`)).status(),200);
    await alice.page.goto(`${base}/reset-password`);
    await alice.page.locator('[name=token]').fill(code);
    await alice.page.locator('[name=new_password]').fill(replacement);
    await alice.page.locator('[name=confirm_password]').fill(replacement);
    const [replay]=await Promise.all([alice.page.waitForResponse(r=>r.request().method()==='POST'&&r.url()===`${base}/reset-password`),alice.page.locator('form button').click()]);
    assert.equal(replay.status(),422);
    const fresh=await login(browser,email,replacement);
    assert.equal((await (await fresh.context.request.get(`${base}/api/v1/me`)).json()).display_name,'Updated A reader');
    await fresh.page.screenshot({path:path.join(shots,'account-reset-complete.png'),fullPage:true});
    await guest.close();await alice.context.close();await bob.context.close();await second.context.close();await fresh.context.close();
    console.log('A: profile isolation, revisions, delivery preferences, one-time mail reset and revoked sessions PASS');
  } finally {await browser.close();}
})().catch(error=>{console.error(error.message);process.exit(1);});
