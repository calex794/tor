"use strict";
const assert=require('node:assert/strict');
const path=require('node:path');
async function count(page,key){return (await page.locator(`[data-testid="seo-current"] [data-metric="${key}"]`).textContent()).trim();}
async function check({adminPage,readerPage,guestPage,base,shots,book,run,sql,privateResponse}) {
  let source, extras=[];
  try {
    await privateResponse(await guestPage.goto(base+'/admin/audience/seo?days=bad'),401);
    await privateResponse(await readerPage.goto(base+'/admin/audience/seo?days=bad'),403);
    assert.equal(sql('SELECT count(*) FROM flow_seo_sources WHERE enabled'),'0');
    await adminPage.goto(base+'/admin/audience?days=7');
    await adminPage.getByTestId('audience-seo-link').click();
    await adminPage.waitForURL(u=>u.pathname==='/admin/audience/seo');
    assert.equal(await adminPage.getByTestId('seo-unconfigured').count(),1);
    assert.notEqual(await count(adminPage,'clicks'),'0');
    source=Number(sql(`INSERT INTO flow_seo_sources(provider,property,origin) VALUES('google','https://seo-browser-${run}.test/','https://seo-browser-${run}.test') RETURNING id`));
    extras=sql(`INSERT INTO books(source_identity,target_language,title,visibility,access_level)
      SELECT 'ci:seo:${run}:'||n,'ru','SEO visible page '||n,'public','basic' FROM generate_series(1,52)n RETURNING id`).split('\n').map(Number);
    assert.equal(extras.length,52);
    sql(`INSERT INTO flow_seo_daily(source_id,day,status,total_clicks,total_impressions,total_position,provider_rows,omitted_rows,limit_reached)
      VALUES(${source},(now() AT TIME ZONE 'UTC')::date-3,'available',300,1000,2.5,53,0,false);
      INSERT INTO flow_seo_pages(source_id,day,path,clicks,impressions,position)
      SELECT ${source},(now() AT TIME ZONE 'UTC')::date-3,'/books/'||b,2,10,4 FROM unnest(ARRAY[${[book,...extras].join(',')}]) b;`);
    const url=base+'/admin/audience/seo?provider=google&days=7';
    const before=sql('SELECT (SELECT count(*) FROM rec_subjects)||\'|\'||(SELECT count(*) FROM reader_chapter_completions)||\'|\'||(SELECT count(*) FROM outbox_events)');
    for(const width of [390,1440]) {
      await adminPage.setViewportSize({width,height:width===390?844:900});
      await privateResponse(await adminPage.goto(url),200);
      assert.equal(await count(adminPage,'clicks'),'300');assert.equal(await count(adminPage,'impressions'),'1000');
      assert.equal(await adminPage.getByTestId('seo-page').count(),50);
      assert.equal(await adminPage.getByTestId('seo-comparison-unavailable').count(),1);
      assert.ok(await adminPage.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
      await adminPage.screenshot({path:path.join(shots,`audience-seo-${width}.png`)});
      await adminPage.getByTestId('seo-current').screenshot({path:path.join(shots,`audience-seo-totals-${width}.png`)});
      await adminPage.getByTestId('seo-page').first().screenshot({path:path.join(shots,`audience-seo-page-${width}.png`)});
      await adminPage.getByTestId('seo-next').click();await adminPage.waitForURL(u=>u.searchParams.has('after'));
      assert.equal(await adminPage.getByTestId('seo-page').count(),3);assert.equal(await count(adminPage,'clicks'),'300');
    }
    for(const days of [1,7,30,90]) {
      await adminPage.goto(url);await adminPage.locator('select[name="days"]').selectOption(String(days));
      await adminPage.locator('form[action="/admin/audience/seo"]').getByRole('button').click();
      await adminPage.waitForURL(u=>u.searchParams.get('days')===String(days));assert.equal(await count(adminPage,'clicks'),'300');
    }
    await adminPage.locator('select[name="provider"]').selectOption('yandex');
    await adminPage.locator('form[action="/admin/audience/seo"]').getByRole('button').click();
    await adminPage.waitForURL(u=>u.searchParams.get('provider')==='yandex');
    assert.equal(await adminPage.getByTestId('seo-unconfigured').count(),1);assert.notEqual(await count(adminPage,'clicks'),'300');
    await adminPage.goto(url);
    for(let i=0;i<2;i++) {await adminPage.locator('form[action="/admin/audience/seo/refresh"]').getByRole('button').click();await adminPage.waitForURL(u=>u.pathname==='/admin/audience/seo');}
    assert.equal(sql(`SELECT count(*) FROM flow_seo_jobs WHERE source_id=${source}`),'7');
    sql(`UPDATE books SET admin_hold=true WHERE id IN(${[book,...extras].join(',')})`);
    await privateResponse(await adminPage.goto(url),200);
    assert.equal(await adminPage.getByTestId('seo-page').count(),0);assert.equal(await count(adminPage,'clicks'),'300');
    assert.equal(sql('SELECT (SELECT count(*) FROM rec_subjects)||\'|\'||(SELECT count(*) FROM reader_chapter_completions)||\'|\'||(SELECT count(*) FROM outbox_events)'),before);
    console.log('FLOW SEO browser PASS: native navigation/refresh, missing source not zero, provider separation, 50-row keysets, live hiding, no identity writes, 390/1440');
  } finally {
    if(source)sql(`DELETE FROM flow_seo_sources WHERE id=${source}`);
    if(extras.length)sql(`DELETE FROM books WHERE id IN(${extras.join(',')})`);
    sql(`UPDATE books SET admin_hold=false WHERE id=${book}`);
  }
}
module.exports={check};
