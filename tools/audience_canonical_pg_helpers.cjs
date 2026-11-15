'use strict';
// Reuses the parent real C progress journey; no new browser/runtime event collector.
const assert=require('node:assert/strict');
const path=require('node:path');
async function count(page,key){return Number(await page.locator(`[data-testid="canonical-totals"] [data-metric="${key}"]`).textContent());}
async function check({adminPage,readerPage,guestPage,base,shots,book,run,sql,privateResponse,event}){
  const tag='c-browser-'+run;
  let extras=[];
  try {
    sql(`UPDATE books SET tags=ARRAY['${tag}'],genres=ARRAY['canonical-browser'] WHERE id=${book}`);
    extras=sql(`INSERT INTO books(source_identity,target_language,title,visibility,access_level,tags,genres)
      SELECT 'ci:canonical:${run}:'||n,'ru','Canonical empty book '||n,'public','basic',ARRAY['${tag}'],
        CASE WHEN n=1 THEN ARRAY[]::text[] ELSE ARRAY['canonical-browser'] END
      FROM generate_series(1,22)n RETURNING id`).split('\n').filter(Boolean).map(Number);
    assert.equal(extras.length,22);
    await privateResponse(await guestPage.goto(base+'/admin/audience/reading?days=bad'),401);
    await privateResponse(await readerPage.goto(base+'/admin/audience/reading?days=bad'),403);
    await adminPage.goto(base+'/admin/audience?days=7');
    await adminPage.getByTestId('audience-canonical-link').click();
    await adminPage.waitForURL(url=>url.pathname==='/admin/audience/reading');
    const scoped=base+'/admin/audience/reading?days=7&tag='+encodeURIComponent(tag);
    for(const width of [390,1440]){
      await adminPage.setViewportSize({width,height:width===390?844:900});
      await privateResponse(await adminPage.goto(scoped),200);
      assert.equal(await count(adminPage,'books'),23);
      assert.equal(await count(adminPage,'current'),1);
      assert.equal(await count(adminPage,'previous'),0);
      assert.equal(await count(adminPage,'library'),1);
      assert.equal(await count(adminPage,'follows'),1);
      assert.equal(await adminPage.getByTestId('canonical-row').count(),20);
      assert.ok(await adminPage.getByTestId('canonical-small').count()>0);
      assert.equal(await adminPage.getByTestId('canonical-delta').count(),0);
      assert.ok(!(await adminPage.content()).includes(event));
      assert.ok(await adminPage.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
      await adminPage.screenshot({path:path.join(shots,`audience-canonical-reading-${width}.png`)});
      await adminPage.getByTestId('canonical-totals').screenshot({path:path.join(shots,`audience-canonical-totals-${width}.png`)});
      await adminPage.getByTestId('canonical-row').first().screenshot({path:path.join(shots,`audience-canonical-row-${width}.png`)});
      await adminPage.getByTestId('canonical-next').click();
      await adminPage.waitForURL(url=>url.searchParams.has('after_id'));
      assert.equal(await adminPage.getByTestId('canonical-row').count(),3);
      assert.equal(await count(adminPage,'books'),23);
      assert.equal(await count(adminPage,'current'),1);
      assert.equal(await adminPage.getByTestId('canonical-next').count(),0);
      await adminPage.getByTestId('canonical-first').click();
      await adminPage.waitForURL(url=>!url.searchParams.has('after_id'));
      assert.equal(await adminPage.getByTestId('canonical-row').count(),20);
    }
    for(const days of [1,30,90]){
      await adminPage.locator('#canonical-days').selectOption(String(days));
      await adminPage.locator('form[action="/admin/audience/reading"]').getByRole('button').click();
      await adminPage.waitForURL(url=>url.searchParams.get('days')===String(days));
      assert.equal(await count(adminPage,'current'),1);
      assert.equal(await count(adminPage,'previous'),0);
    }
    for(const kind of ['genres','tags']){
      await adminPage.locator('#canonical-kind').selectOption(kind);
      await adminPage.locator('form[action="/admin/audience/reading"]').getByRole('button').click();
      await adminPage.waitForURL(url=>url.searchParams.get('kind')===kind);
      assert.equal(await adminPage.getByTestId('canonical-row').count(),kind==='genres'?2:1);
      assert.equal(await count(adminPage,'books'),23);assert.equal(await count(adminPage,'current'),1);
    }
    sql(`UPDATE books SET admin_hold=true WHERE id=${book}`);
    await privateResponse(await adminPage.goto(scoped),200);
    assert.equal(await count(adminPage,'books'),22);assert.equal(await count(adminPage,'current'),0);
    assert.equal(await count(adminPage,'library'),0);assert.equal(await count(adminPage,'follows'),0);
    assert.ok(!(await adminPage.content()).includes('AUDIENCE-PRIVATE-SENTINEL'));
    assert.equal(Number(sql(`SELECT count(*) FROM reader_chapter_completions WHERE book_id=${book}`)),1);
    sql(`UPDATE books SET admin_hold=false WHERE id=${book}`);
    await privateResponse(await adminPage.goto(scoped),200);assert.equal(await count(adminPage,'current'),1);
    console.log('FLOW canonical browser PASS: real C fact, views do not add facts, all periods, filters, missing group, 20-row keysets, live hiding, 390/1440');
  } finally {
    if(extras.length)sql(`DELETE FROM books WHERE id IN(${extras.join(',')})`);
    sql(`UPDATE books SET admin_hold=false WHERE id=${book}`);
  }
}
module.exports={check};
