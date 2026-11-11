'use strict';
// Explicit legacy arithmetic fixture: accepted 190 plus primitives and anonymous observation, without retirement.
const assert = require('node:assert/strict');
const {execFileSync} = require('node:child_process');

module.exports = function legacyAnalyticsDatabase() {
  const db = process.env.LEGACY_ANALYTICS_DATABASE_URL;
  assert.ok(db, 'explicit LEGACY_ANALYTICS_DATABASE_URL is required');
  assert.equal(process.env.APP_URL, db, 'browser fixture and server database must match');
  const url = new URL(db);
  assert.equal(url.protocol, 'postgresql:');
  assert.equal(url.hostname, '127.0.0.1');
  assert.equal(url.port, '5432');
  assert.equal(url.username, 'tor_app');
  assert.equal(url.pathname, '/tor_ci_legacy_analytics');
  assert.equal(url.search, '');
  assert.equal(url.hash, '');
  const proof = execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1',
    '--dbname',db,'--command',`SELECT current_database()='tor_ci_legacy_analytics'
      AND current_user='tor_app'
      AND (SELECT count(*)=192 AND bool_and(success) AND max(version)=202611163827
           FROM _sqlx_migrations)
      AND to_regclass('public.rec_analytics_invalidations') IS NOT NULL
      AND to_regprocedure('public.rec_retire_analytics_batch(integer)') IS NULL
      AND NOT EXISTS(SELECT 1 FROM pg_trigger WHERE tgname='retired_collection_write_fence')`], {
    encoding:'utf8', env:{...process.env,PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'},
  }).trim();
  assert.equal(proof, 't', 'legacy suite must never write retired fixtures into the retirement schema');
  return db;
};
