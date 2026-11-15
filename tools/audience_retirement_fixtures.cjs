'use strict';
// Explicit legacy fixtures for retirement acceptance on disposable tor_ci.
// A real progress request never inserts a historical fixture through this helper.
const assert = require('node:assert/strict');
const {randomBytes} = require('node:crypto');

module.exports = function fixtures({sql,scalar,base}) {
  function positive(value) {
    assert.ok(Number.isSafeInteger(value) && value>0,'positive fixture identifier');
    return value;
  }
  function coverage(kind) {
    assert.ok(['global','book'].includes(kind));
    const table=kind==='global'?'reader_return_coverage':'reader_book_return_coverage';
    const id=scalar(`INSERT INTO ${table}(started_at,last_heartbeat_at,ended_at)
      VALUES(date_trunc('day',now())-interval '120 days',
        now()-interval '1 minute',now()-interval '1 minute') RETURNING id`);
    return {table,id};
  }
  function subject(user) {
    positive(user);
    const value=sql(`INSERT INTO rec_subjects(user_id,enabled,enabled_at)
      VALUES(${user},true,now()-interval '2 minutes') RETURNING subject_id`);
    assert.match(value,/^[0-9a-f-]{36}$/);
    return value;
  }
  function global(user,epoch) {
    positive(user); positive(epoch);
    sql(`INSERT INTO reader_return_subjects(subject_id,cohort_day,coverage_epoch_id,
      returned_d1,returned_d7,returned_d30)
      SELECT subject_id,(now() AT TIME ZONE 'UTC')::date,${epoch},false,false,false
      FROM rec_subjects WHERE user_id=${user}`);
  }
  function book(user,id) {
    positive(user); positive(id);
    sql(`INSERT INTO reader_book_return_subjects(subject_id,book_id,cohort_day)
      SELECT subject_id,${id},(now() AT TIME ZONE 'UTC')::date
      FROM rec_subjects WHERE user_id=${user}`);
  }
  function snapshot(user) {
    positive(user);
    return JSON.parse(sql(`SELECT jsonb_build_object(
      'subject',(SELECT to_jsonb(s) FROM rec_subjects s WHERE s.user_id=${user}),
      'days',COALESCE((SELECT jsonb_agg(to_jsonb(d) ORDER BY d.book_id,d.activity_day)
        FROM reader_activity_days d JOIN rec_subjects s USING(subject_id)
        WHERE s.user_id=${user}),'[]'::jsonb),
      'global',COALESCE((SELECT jsonb_agg(to_jsonb(r))
        FROM reader_return_subjects r JOIN rec_subjects s USING(subject_id)
        WHERE s.user_id=${user}),'[]'::jsonb),
      'books',COALESCE((SELECT jsonb_agg(to_jsonb(b) ORDER BY b.book_id,b.cohort_day)
        FROM reader_book_return_subjects b JOIN rec_subjects s USING(subject_id)
        WHERE s.user_id=${user}),'[]'::jsonb),
      'commands',(SELECT count(*) FROM rec_user_preference_commands WHERE user_id=${user})
    )`));
  }
  async function retiredConsent(page,enabled) {
    const response=await page.goto(base+'/me/recommendations');
    assert.equal(response.status(),200);
    assert.equal(await page.locator("form[action='/me/recommendations/consent']").count(),0);
    assert.equal(await page.locator('form[data-flow-consent]').count(),0);
    const csrf=(await page.context().cookies()).find(c=>c.name==='tor-dev-csrf')?.value;
    assert.ok(csrf);
    const retired=await page.context().request.post(base+'/me/recommendations/consent',{
      headers:{Origin:base},
      form:{enabled:enabled?'1':'0',expected_revision:'0',
        command_key:randomBytes(32).toString('base64url'),csrf_token:csrf},
      maxRedirects:0,
    });
    assert.equal(retired.status(),410);
    assert.equal(retired.headers()['cache-control'],'no-store');
    assert.match(retired.headers()['x-robots-tag'],/noindex.*nofollow/);
    assert.equal((await retired.json()).error.code,'COLLECTION_RETIRED');
  }
  async function labelled(page,reportId) {
    const notice=page.getByTestId('collection-retired');
    assert.equal(await notice.count(),1);
    assert.equal(await notice.locator('[data-current-measurement]').innerText(),'Не измеряется');
    assert.equal(await page.getByTestId(reportId).getAttribute('data-measurement'),'legacy-consented');
  }
  function removeCoverage(value) {
    if (value) sql(`DELETE FROM ${value.table} WHERE id=${positive(value.id)}`);
  }
  return {coverage,subject,global,book,snapshot,retiredConsent,labelled,removeCoverage};
};
