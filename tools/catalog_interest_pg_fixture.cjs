'use strict';
// Disposable fixture data only. Production C commands are sent by the browser.
const {execFileSync} = require('node:child_process');
const {randomBytes} = require('node:crypto');
const assert = require('node:assert/strict');
const db = process.env.APP_URL || process.env.TEST_DATABASE_URL || '';
const owner = process.env.MIGRATOR_URL || '';
const base = process.env.TOR_CATALOG_INTEREST_BASE || 'http://127.0.0.1:3118';
const shots = process.env.TOR_CATALOG_INTEREST_SHOTS || '/tmp/catalog-interest-shots';
const port = process.env.FLOW_TEST_PG_PORT || '5432';
const schema = process.env.TOR_CATALOG_INTEREST_SCHEMA || '';
assert.match(schema, /^flow_catalog_browser_[0-9]+_[0-9]+$/);
assert.match(port, /^[1-9][0-9]{0,4}$/);
assert.ok(Number(port) <= 65535);
for (const [url, username] of [[db, 'tor_app'], [owner, 'tor_migrator']]) {
  const parsed = new URL(url);
  assert.equal(parsed.protocol, 'postgresql:');
  assert.equal(parsed.hostname, '127.0.0.1');
  assert.equal(parsed.port, port);
  assert.equal(parsed.pathname, '/tor_ci');
  assert.equal(parsed.username, username);
  assert.equal(parsed.search, '');
  assert.equal(parsed.hash, '');
}
assert.match(base, /^http:\/\/127\.0\.0\.1:[1-9][0-9]*$/);
assert.ok(Number(new URL(base).port) <= 65535);
function sql(statement, url = db) {
  return execFileSync('psql', ['-XAtq', '-v', 'ON_ERROR_STOP=1', '--dbname', url, '-c', statement], {
    encoding: 'utf8',
    env: {...process.env, PGOPTIONS: '-c statement_timeout=5000 -c lock_timeout=2000 -c search_path=' + schema + ',public'},
  }).trim();
}
function scalar(statement, url = db) {
  const value = sql(statement, url);
  assert.match(value, /^\d+$/);
  const number = Number(value);
  assert.ok(Number.isSafeInteger(number));
  return number;
}
class Fixture {
  constructor() {
    this.run = Date.now() + '-' + randomBytes(4).toString('hex');
    this.tag = 'flow05-' + this.run;
    this.prefix = 'ci:catalog-interest:' + this.run + ':';
    this.emails = ['admin', 'reader'].map(role => 'catalog-' + role + '-' + this.run + '@tor-ci.example');
    this.books = [];
    this.actors = [];
    this.epoch = null;
    this.anchor = scalar('SELECT floor((extract(epoch FROM now())-900)/3600)::bigint*3600');
    // Only F history is isolated in real tables copied from the current migration.
    // A/B/C use the public production tables and commands. Other workers' history
    // is neither read as this fixture's evidence nor deleted or reset.
    assert.equal(scalar('SELECT count(*) FROM flow_observation_epochs WHERE started_ms<' +
      (this.anchor + 3600 + 902) * 1000 + ' AND (NOT retired OR ended_ms>' +
      (this.anchor - 2 * 86400) * 1000 + ')'), 0, 'requires fresh historical observation windows');
    assert.equal(scalar('SELECT count(*) FROM flow_observation_health_hourly WHERE bucket_ms>=' +
      (this.anchor - 2 * 86400) * 1000 + ' AND bucket_ms<' + this.anchor * 1000), 0);
  }
  seed() {
    this.books = sql("INSERT INTO books(source_identity,target_language,title,visibility,access_level,tags,genres)" +
      " SELECT '" + this.prefix + "'||n,'ru','Каталог: книга '||n,'public','basic',ARRAY['" + this.tag +
      "'],ARRAY['Каталог проверки'] FROM generate_series(1,53)n RETURNING id").split('\n').map(Number);
    assert.equal(this.books.length, 53);
    assert.ok(this.books.every(id => Number.isSafeInteger(id) && id > 0));
    sql("UPDATE books SET title='Старая пара, новая глава' WHERE id=" + this.books[0] + ";" +
      "UPDATE books SET title='Наблюдаемые показы без записей чтения' WHERE id=" + this.books[1] + ";" +
      "UPDATE books SET title='Малая база наблюдения' WHERE id=" + this.books[3] + ";" +
      "UPDATE books SET title='Рейтинг <img src=x onerror=alert(1)>',genres=ARRAY['Общий жанр','Первый жанр'] WHERE id=" + this.books[51] + ";" +
      "UPDATE books SET title='Рейтинг: равное число первых пар',genres=ARRAY['Общий жанр','Второй жанр'] WHERE id=" + this.books[52]);
    sql("INSERT INTO flow_observation_hourly(bucket,book_id,surface,opportunities,rendered,views)" +
      " SELECT to_timestamp(" + (this.anchor - 7200) + "),id,'catalog',25,CASE WHEN id=" +
      this.books[3] + " THEN 19 ELSE 20 END,5 FROM books WHERE source_identity LIKE '" + this.prefix + "%'", owner);
  }
  chapter(index, number) {
    const chapter = scalar("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)" +
      " VALUES(" + this.books[index] + "," + number + ",'Глава проверки','public','free',0) RETURNING id");
    const version = scalar("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)" +
      " VALUES(" + chapter + ",1,'Audience text') RETURNING id");
    sql('INSERT INTO chapter_publications(chapter_id,version_id) VALUES(' + chapter + ',' + version + ')');
    return {chapter, version};
  }
  placeVerifiedFact(chapter, ageSeconds) {
    // Position facts made by actual C commands in a historical test window.
    // This does not claim that the browser measured traffic in the past.
    const at = this.anchor - ageSeconds;
    sql('UPDATE reader_chapter_starts SET started_at=to_timestamp(' + at + ') WHERE user_id=' +
      this.actors[1] + ' AND chapter_id=' + chapter + ';' +
      'UPDATE reader_chapter_completions SET completed_at=to_timestamp(' + at + ') WHERE user_id=' +
      this.actors[1] + ' AND chapter_id=' + chapter);
  }
  certify() {
    // Synthetic history covers both possible settled boundaries during this run.
    // Source queries still choose the actual DB boundary; there is no error retry.
    assert.equal(this.epoch, null);
    const from = (this.anchor - 2 * 86400) * 1000;
    const through = (this.anchor + 3600 + 902) * 1000;
    this.epoch = scalar('INSERT INTO flow_observation_epochs(started_ms,confirmed_ms,lease_until_ms,retired,ended_ms)' +
      ' VALUES(' + [from, through, through, 'true', through].join(',') + ') RETURNING epoch', owner);
  }
  assertBoundary() {
    const current = scalar("SELECT floor((extract(epoch FROM now())-900)/3600)::bigint*3600");
    assert.ok([this.anchor, this.anchor + 3600].includes(current), "fixture exceeded two-boundary coverage");
  }
  fingerprint() {
    return sql("SELECT md5(jsonb_build_array(" +
      "(SELECT jsonb_agg(to_jsonb(t) ORDER BY user_id,chapter_id) FROM reader_chapter_starts t WHERE book_id=ANY(ARRAY[" + this.books + "]))," +
      "(SELECT jsonb_agg(to_jsonb(t) ORDER BY user_id,chapter_id) FROM reader_chapter_completions t WHERE book_id=ANY(ARRAY[" + this.books + "]))," +
      "(SELECT jsonb_agg(jsonb_build_array(id,event_type,schema_version,aggregate_kind,aggregate_id,payload,required_consumers) ORDER BY id) FROM outbox_events t WHERE id IN(SELECT source_event_id FROM reader_chapter_starts WHERE book_id=ANY(ARRAY[" + this.books +
      "]) UNION SELECT source_event_id FROM reader_chapter_completions WHERE book_id=ANY(ARRAY[" + this.books + "])))," +
      "(SELECT jsonb_agg(to_jsonb(t) ORDER BY book_id,bucket) FROM flow_observation_hourly t WHERE book_id=ANY(ARRAY[" + this.books + "]))" +
      ")::text)");
  }
  cleanup() {
    if (this.epoch !== null) sql('DELETE FROM flow_observation_health_hourly WHERE epoch=' + this.epoch +
      ';DELETE FROM flow_observation_epochs WHERE epoch=' + this.epoch, owner);
    if (this.books.length) sql("DELETE FROM flow_observation_hourly WHERE book_id=ANY(ARRAY[" + this.books + "])", owner);
    // Remove only this fixture's rows. No historical shared source is reset.
    if (this.books.length) sql("BEGIN;" +
      "CREATE TEMP TABLE catalog_owned_events ON COMMIT DROP AS SELECT source_event_id AS id FROM reader_chapter_starts WHERE book_id=ANY(ARRAY[" + this.books + "]) UNION SELECT source_event_id FROM reader_chapter_completions WHERE book_id=ANY(ARRAY[" + this.books + "]);" +
      "DELETE FROM event_deliveries WHERE event_id IN(SELECT id FROM catalog_owned_events);" +
      "DELETE FROM outbox_events WHERE id IN(SELECT id FROM catalog_owned_events);" +
      "DELETE FROM reader_book_positions WHERE book_id=ANY(ARRAY[" + this.books + "]);" +
      "DELETE FROM reader_chapter_starts WHERE book_id=ANY(ARRAY[" + this.books + "]);" +
      "DELETE FROM reader_chapter_completions WHERE book_id=ANY(ARRAY[" + this.books + "]);" +
      "DELETE FROM reader_saved_free_chapters WHERE chapter_id IN(SELECT id FROM chapters WHERE book_id=ANY(ARRAY[" + this.books + "]));" +
      "DELETE FROM chapter_publications WHERE chapter_id IN(SELECT id FROM chapters WHERE book_id=ANY(ARRAY[" + this.books + "]));" +
      "DELETE FROM book_members WHERE book_id=ANY(ARRAY[" + this.books + "]);" +
      "DELETE FROM books WHERE source_identity LIKE '" + this.prefix + "%';COMMIT", owner);
    for (const id of this.actors) sql('DELETE FROM account_roles WHERE user_id=' + id);
    sql("DELETE FROM users WHERE email_normalized IN('" + this.emails.join("','") + "')");
  }
}
function prepare() {
  let statement = 'CREATE SCHEMA ' + schema + ';GRANT USAGE ON SCHEMA ' + schema + ' TO tor_app;';
  for (const table of ['flow_observation_hourly', 'flow_observation_epochs', 'flow_observation_health_hourly']) {
    statement += 'CREATE TABLE ' + schema + '.' + table + ' (LIKE public.' + table + ' INCLUDING ALL);';
  }
  statement += 'REVOKE ALL ON ALL TABLES IN SCHEMA ' + schema + ' FROM tor_app;' +
    'GRANT SELECT ON ALL TABLES IN SCHEMA ' + schema + ' TO tor_app;';
  sql(statement, owner);
}
if (require.main === module) {
  const action = process.argv[2];
  if (action === 'prepare') prepare();
  else if (action === 'cleanup') sql('DROP SCHEMA ' + schema + ' CASCADE', owner);
  else if (action === 'url') {
    const scoped = new URL(db);
    scoped.searchParams.set('options', '-c search_path=' + schema + ',public');
    process.stdout.write(scoped.toString());
  } else throw new Error('expected prepare, url, or cleanup');
}
module.exports = {Fixture, sql, scalar, db, owner, base, shots};

