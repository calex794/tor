// S015: live notification privacy with a real worker, PostgreSQL and Chromium.
const assert = require('node:assert/strict');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
function fixture(sql, values) {
  const db = process.env.TEST_DATABASE_URL || process.env.APP_URL || '';
  assert(db.endsWith('/tor_ci'), 'disposable notification database required');
  const args = ['-X', '-A', '-t', '-q', '-v', 'ON_ERROR_STOP=1', '--dbname', db];
  for (const [key, value] of Object.entries(values)) args.push('-v', key + '=' + value);
  args.push('--file', '-');
  return execFileSync('psql', args, { input: sql, encoding: 'utf8' }).trim();
}
module.exports = async (author, reader, outsider, base, out, name, conversation) => {
  const values = {
    author: author.id, reader: reader.id,
    source: 'ci:s015:privacy:' + require('node:crypto').randomUUID(),
  };
  const ids = fixture(`BEGIN;
INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
VALUES(:'source','ru','S015 notification book','public',:'author','basic')
RETURNING id AS book_id
\\gset
INSERT INTO reader_book_follows(user_id,book_id) VALUES(:'reader',:'book_id');
INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
VALUES(:'book_id',1,'S015 notification chapter','public','free',0)
RETURNING id AS chapter_id
\\gset
INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
VALUES(:'chapter_id',1,'S015-PRIVATE-READ-MARKER')
RETURNING id AS version_id
\\gset
INSERT INTO chapter_publications(chapter_id,version_id) VALUES(:'chapter_id',:'version_id');
INSERT INTO chapter_first_release_events(chapter_id,version_id,creator_user_id)
VALUES(:'chapter_id',:'version_id',:'author');
INSERT INTO outbox_events(event_type,schema_version,aggregate_kind,aggregate_id,
                          dedupe_key,payload,required_consumers)
VALUES('reader.chapter.released',1,'chapter',:'chapter_id',
       :'source' || ':release',jsonb_build_object('actor_user_id',:'author'::bigint,
                          'recipient_user_id',:'reader'::bigint,
                          'object_id',:'chapter_id'::bigint,
                          'book_id',:'book_id'::bigint,
                          'version_id',:'version_id'::bigint),
       ARRAY['user_notifications']::text[]);
COMMIT;
SELECT :'book_id' || ':' || :'chapter_id';`, values).split(':');
  assert(ids.length === 2 && ids.every(id => /^[1-9][0-9]*$/.test(id)));
  const [book, chapter] = ids;
  const bookValues = { book, reader: reader.id };
  await reader.page.goto(base + '/notifications?tab=releases');
  const card = reader.page.locator('[data-notification-kind="chapter_created"]')
    .filter({ has: reader.page.locator('input[name="next"][value="/reader/' + chapter + '"]') });
  for (let attempt = 0; attempt < 20 && await card.count() === 0; attempt++) {
    await reader.page.waitForTimeout(500);
    await reader.page.reload();
  }
  assert.equal(await card.count(), 1, 'real worker delivered eligible release');
  const notice = await card.getAttribute('data-notification-id');
  const saved = await card.locator('form').evaluate(el => Object.fromEntries(new FormData(el)));
  const route = base + '/notifications/' + notice + '/read';
  const post = (client, form) => client.context.request.post(route, {
    form, headers: { Origin: base }, maxRedirects: 0,
  });
  assert.equal((await post(reader, { ...saved, csrf_token: 'forged' })).status(), 403);
  await outsider.page.goto(base + '/notifications');
  const otherCsrf = await outsider.page.locator('form[action="/notifications/read-all"] [name="csrf_token"]').inputValue();
  assert.equal((await post(outsider, { ...saved, csrf_token: otherCsrf })).status(), 404);
  async function hidden() {
    for (const tab of ['', '?tab=releases']) {
      await reader.page.goto(base + '/notifications' + tab);
      assert.equal(await reader.page.locator('[data-notification-id="' + notice + '"]').count(), 0);
    }
    assert.equal((await post(reader, saved)).status(), 404, 'stale Open form is no access grant');
  }
  async function restored() {
    await reader.page.goto(base + '/notifications?tab=releases');
    assert.equal(await reader.page.locator('[data-notification-id="' + notice + '"]').count(), 1);
  }
  await reader.page.goto(base + conversation);
  await reader.page.getByRole('button', { name: 'Заблокировать собеседника', exact: true }).click();
  await reader.page.waitForURL(base + conversation);
  await hidden();
  // Social blocking is not an administrative read ban on a public free chapter.
  const publicRead = await reader.context.request.get(base + '/read/' + chapter);
  assert.equal(publicRead.status(), 200);
  assert((await publicRead.text()).includes('S015-PRIVATE-READ-MARKER'));
  await reader.page.goto(base + conversation);
  await reader.page.getByRole('button', { name: 'Снять мою блокировку', exact: true }).click();
  await reader.page.waitForURL(base + conversation);
  await restored();

  // Friendship must not become a private-book reading entitlement.
  const authorCsrf = await author.page.locator('form[action$="/friend"] [name="csrf_token"]').inputValue();
  for (const [client, peer, token] of [[author, reader.id, authorCsrf], [reader, author.id, saved.csrf_token]]) {
    const request = await client.context.request.post(base + '/users/' + peer + '/friend', {
      form: { csrf_token: token }, headers: { Origin: base }, maxRedirects: 0,
    });
    assert.equal(request.status(), 303);
  }
  fixture("UPDATE books SET visibility='draft' WHERE id=:'book';", bookValues);
  await hidden();
  for (const client of [reader, outsider]) {
    const denied = await client.context.request.get(base + '/read/' + chapter);
    assert.equal(denied.status(), 404);
    assert(!(await denied.text()).includes('S015-PRIVATE-READ-MARKER'));
  }
  fixture("UPDATE books SET visibility='public',access_level='adult' WHERE id=:'book';", bookValues);
  await hidden();
  assert.equal((await reader.context.request.get(base + '/read/' + chapter)).status(), 404);
  fixture("UPDATE users SET adult_enabled=true WHERE id=:'reader';", bookValues);
  await restored();
  fixture("UPDATE users SET adult_enabled=false WHERE id=:'reader';", bookValues);
  await hidden();
  fixture("UPDATE books SET access_level='basic' WHERE id=:'book';", bookValues);
  await restored();
  await reader.page.screenshot({ path: path.join(out, 's015-notification-restored-' + name + '.png'), fullPage: true });
  assert.equal((await post(reader, saved)).status(), 303);
  assert.equal((await author.context.request.post(base + '/users/' + reader.id + '/unfriend', {
    form: { csrf_token: authorCsrf }, headers: { Origin: base }, maxRedirects: 0,
  })).status(), 303);
  console.log('S015 ' + name + ': live release notification block/hide/age/stale-form/friend-private-read PASS');
};

