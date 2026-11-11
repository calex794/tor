'use strict';
// Current migration-193 fixture guards and exact optional-store snapshots.
// Installed legacy data is exercised only by the separate migration harness.
const assert = require('node:assert/strict');
const {randomBytes} = require('node:crypto');

module.exports = function retirementFixtures(sql) {
  function assertCurrentSchema() {
    assert.equal(sql(`SELECT EXISTS(SELECT 1 FROM _sqlx_migrations
      WHERE version=202611168903 AND success)
      AND EXISTS(SELECT 1 FROM pg_trigger WHERE tgrelid='rec_subjects'::regclass
        AND tgname='retired_collection_write_fence' AND tgenabled='O')`),'t',
      'browser acceptance requires the current database retirement barrier');
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
  return {assertCurrentSchema,optionalSnapshot,assertNoControls,retiredConsent};
};
