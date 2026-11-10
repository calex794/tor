'use strict';
const assert = require('node:assert/strict');

module.exports = function installedRetirementFixture(sql) {
  const db = process.env.COLLECTION_RETIREMENT_APP_DATABASE_URL;
  assert.ok(db, 'the actual installed-upgrade database is required');
  assert.equal(process.env.APP_URL, db);
  const url = new URL(db);
  assert.equal(url.protocol, 'postgresql:');
  assert.equal(url.hostname, '127.0.0.1');
  assert.equal(url.port, '5432');
  assert.equal(url.username, 'tor_app');
  assert.equal(url.pathname, '/tor_ci_collection_retirement_upgrade');
  assert.equal(url.search, '');
  assert.equal(url.hash, '');
  require('./collection_retirement_fixtures.cjs')(sql).assertCurrentSchema();
  const books = JSON.parse(sql(`SELECT jsonb_agg(to_jsonb(b) ORDER BY id) FROM (
    SELECT id,source_identity,title,genres,tags,admin_hold FROM books
    WHERE source_identity IN('ci:retirement:linked','ci:retirement:untouched',
      'ci:retirement:queued')) b`));
  assert.equal(books.length, 3, 'use fixtures created before the installed 192 barrier');
  const byKey = Object.fromEntries(books.map(book => [book.source_identity.split(':').at(-1), book]));
  const actor = Number(sql("SELECT id FROM users WHERE email_normalized='retirement-main@ci.invalid'"));
  assert.ok(Number.isSafeInteger(actor) && actor > 0);
  const ids = books.map(book => book.id).join(',');
  function snapshot() {
    const fields = [];
    for (const table of ['reader_chapter_starts','reader_chapter_completions',
      'reader_library_items','reader_book_follows','rec_preference_accounts',
      'rec_book_preferences','rec_user_preference_commands']) {
      fields.push(`'${table}',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]'::jsonb)
        FROM ${table} t WHERE user_id=${actor})`);
    }
    for (const table of ['rec_analytics_invalidations','rec_quality_daily',
      'organic_discovery_quality_daily']) {
      fields.push(`'${table}',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]'::jsonb)
        FROM ${table} t WHERE book_id IN(${ids})
        AND bucket_date>=(now() AT TIME ZONE 'UTC')::date-89)`);
    }
    return JSON.parse(sql('SELECT jsonb_build_object('+fields.join(',')+')'));
  }
  function noRaw() {
    for (const table of ['rec_subjects','rec_assignments','rec_attribution_events',
      'rec_reader_signals','rec_first_conversions','organic_discovery_touches',
      'organic_discovery_first_conversions','organic_discovery_unattributed_reads',
      'flow_visits','flow_visit_receipts','flow_book_cohorts',
      'reader_flow_book_actions','reader_flow_activity_days','reader_flow_read_receipts',
      'reader_activity_days','reader_return_subjects','reader_book_return_subjects',
      'ril_behavior_consents']) {
      assert.equal(sql('SELECT count(*) FROM '+table), '0', 'retired store '+table);
    }
    assert.equal(sql(`SELECT count(*) FROM event_deliveries
      WHERE consumer IN('recommendation-feedback-v1','flow-read-attribution-v1')`), '0');
  }
  noRaw();
  const before = snapshot();
  assert.ok(before.reader_chapter_starts.length > 0, 'canonical starts survived the upgrade');
  assert.ok(before.reader_chapter_completions.length > 0, 'canonical completions remain usable');
  assert.ok(before.rec_preference_accounts.length > 0, 'independent preferences survived');
  return {books, byKey, actor, snapshot, before, noRaw};
};
