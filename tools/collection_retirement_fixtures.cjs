'use strict';
// Explicit historical tor_ci fixtures and exact optional-store snapshots.
// These helpers never run inside a reading/navigation command.
const assert = require('node:assert/strict');
const {randomBytes} = require('node:crypto');

module.exports = function retirementFixtures(sql) {
  function legacyScope(user) {
    assert.ok(Number.isSafeInteger(user) && user>0);
    sql(`INSERT INTO rec_subjects(user_id,enabled,enabled_at,revision,flow_epoch,flow_enabled_at)
      VALUES(${user},true,now()-interval '1 day',7,gen_random_uuid(),now()-interval '1 day')`);
    return JSON.parse(sql(`SELECT json_build_object('subject',subject_id,'epoch',flow_epoch,'revision',revision)
      FROM rec_subjects WHERE user_id=${user}`));
  }
  function optionalSnapshot(user) {
    assert.ok(Number.isSafeInteger(user) && user>0);
    const fields=[`'subject',(SELECT to_jsonb(s) FROM rec_subjects s WHERE user_id=${user})`];
    for (const table of ['rec_assignments','rec_reader_signals','rec_first_conversions',
      'organic_discovery_touches','organic_discovery_first_conversions','organic_discovery_unattributed_reads',
      'flow_visits','flow_visit_receipts','flow_visit_account_days','flow_visit_continuations',
      'flow_book_cohorts','flow_book_action_receipts','flow_book_return_anchors',
      'reader_flow_book_actions','reader_flow_activity_days','reader_flow_read_receipts',
      'reader_activity_days','reader_return_subjects','reader_book_return_subjects']) {
      fields.push(`'${table}',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]'::jsonb)
        FROM ${table} t JOIN rec_subjects s USING(subject_id) WHERE s.user_id=${user})`);
    }
    fields.push(`'rec_events',(SELECT COALESCE(jsonb_agg(to_jsonb(e) ORDER BY to_jsonb(e)::text),'[]'::jsonb)
      FROM rec_attribution_events e JOIN rec_assignments a ON a.id=e.assignment_id
      JOIN rec_subjects s ON s.subject_id=a.subject_id WHERE s.user_id=${user})`);
    fields.push(`'deliveries',(SELECT COALESCE(jsonb_agg(to_jsonb(d) ORDER BY to_jsonb(d)::text),'[]'::jsonb)
      FROM event_deliveries d JOIN outbox_events e ON e.id=d.event_id
      WHERE e.payload->>'user_id'='${user}'
      AND d.consumer IN('recommendation-feedback-v1','flow-read-attribution-v1'))`);
    fields.push(`'ril_consent',(SELECT to_jsonb(c) FROM ril_behavior_consents c WHERE user_id=${user})`);
    return JSON.parse(sql('SELECT jsonb_build_object('+fields.join(',')+')'));
  }
  async function assertNoControls(page) {
    for (const selector of ["form[action='/me/recommendations/consent']",
      "form[action='/me/recommendations/flow-consent']",'form[data-flow-consent]',
      "form[action='/me/behavior']",'[data-rec-ticket]','[data-organic-ticket]',
      "script[src='/assets/recommendations.js']","script[src='/assets/organic_discovery.js']"]) {
      assert.equal(await page.locator(selector).count(),0,'retired UI hook '+selector);
    }
  }
  async function retiredConsent(page,endpoint,enabled) {
    const origin=new URL(page.url()).origin;
    assert.match(origin,/^http:\/\/127\.0\.0\.1:\d+$/);
    const csrf=(await page.context().cookies(origin)).find(cookie=>cookie.name==='tor-dev-csrf')?.value;
    assert.match(csrf || '',/^[A-Za-z0-9_-]{43}$/);
    const response=await page.request.post(origin+endpoint,{
      headers:{Origin:origin},
      form:{enabled:enabled?'1':'',expected_revision:'7',
        command_key:randomBytes(32).toString('base64url'),csrf_token:csrf},
      maxRedirects:0,
    });
    assert.equal(response.status(),410);
    assert.equal(response.headers()['cache-control'],'no-store');
    assert.match(response.headers()['x-robots-tag'],/noindex/);
    assert.match(response.headers()['x-robots-tag'],/nofollow/);
    assert.equal((await response.json()).error.code,'COLLECTION_RETIRED');
  }
  function historicalVisits(user) {
    // Two old page observations, never represented as current browser activity.
    sql(`BEGIN;
      WITH seed AS (
        SELECT subject_id,flow_epoch,gen_random_uuid() AS first_id,gen_random_uuid() AS last_id
        FROM rec_subjects WHERE user_id=${user}
      ), visit AS (
        INSERT INTO flow_visits(subject_id,flow_epoch,started_at,last_at,first_event_id,last_event_id,
          landing_surface,last_surface,acquisition_hint,pageviews)
        SELECT subject_id,flow_epoch,now()-interval '2 minutes',now()-interval '1 minute',
          first_id,last_id,1,2,0,2 FROM seed RETURNING *
      )
      INSERT INTO flow_visit_receipts(event_id,subject_id,flow_epoch,observed_at,surface,acquisition_hint)
        SELECT first_event_id,subject_id,flow_epoch,started_at,1,0 FROM visit
        UNION ALL SELECT last_event_id,subject_id,flow_epoch,last_at,2,0 FROM visit;
      INSERT INTO flow_visit_account_days(subject_id,flow_epoch,bucket_date)
        SELECT DISTINCT subject_id,flow_epoch,(observed_at AT TIME ZONE 'UTC')::date
        FROM flow_visit_receipts WHERE subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id=${user});
      COMMIT`);
  }
  function historicalFunnel(book) {
    assert.ok(Number.isSafeInteger(book) && book>0);
    sql(`INSERT INTO flow_book_daily(cohort_day,book_id,acquisition_hint,discovery_kind,
      cohorts,detail_opens,reader_opens,starts,completions,saves,follows,new_readers,
      d7_anchors,d7_returned,d7_unknown)
      SELECT (now() AT TIME ZONE 'UTC')::date-days,${book},0,2,
        3,3,2,2,1,1,1,2,2,1,1 FROM (VALUES(40),(100)) old(days)`);
  }
  return {legacyScope,optionalSnapshot,assertNoControls,retiredConsent,historicalVisits,historicalFunnel};
};
