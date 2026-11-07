// CI-only approval fixture for unrelated public author/cover browser journeys.
// New real books start pending_moderation; test code never grants an author
// moderation authority. The HTTP role, CAS and audit proof lives in S020 PG.
const { execFileSync } = require('node:child_process');

function approvePendingBookForIsolatedCi(bookId) {
  const raw = process.env.TEST_DATABASE_URL || process.env.DATABASE_URL || process.env.APP_URL;
  if (process.env.GITHUB_ACTIONS !== 'true' || !raw) {
    throw new Error('Site approval fixture requires disposable GitHub CI');
  }
  const url = new URL(raw);
  if (!['postgres:', 'postgresql:'].includes(url.protocol)
    || url.hostname !== '127.0.0.1'
    || (url.port && url.port !== '5432')
    || url.pathname !== '/tor_ci'
    || url.username !== 'tor_app'
    || url.search || url.hash) {
    throw new Error('Refusing site approval fixture outside tor_app loopback tor_ci');
  }
  const id = String(bookId);
  if (!/^[1-9][0-9]{0,18}$/.test(id)) {
    throw new Error('Invalid Book ID for isolated approval fixture');
  }
  // This helper is isolated to disposable GitHub CI. It models a complete
  // independent moderation decision for unrelated author/reader scenarios.
  // The actual moderator endpoint is exercised separately by MOD-REG PG/HTTP.
  const sql = `DO $ci$
DECLARE
  old_rev bigint;
  owner_id bigint;
  reviewer_id bigint;
  decision_id bigint;
BEGIN
  SELECT visibility_status_revision,owner_user_id
    INTO old_rev,owner_id
    FROM books WHERE id=${id} AND visibility='public'
      AND visibility_status='pending_moderation'
      AND registration_requires_review
      AND registration_approval_decision_id IS NULL FOR UPDATE;
  IF old_rev IS NULL THEN
    RAISE EXCEPTION 'not an unapproved pending Book';
  END IF;
  -- Dedicated Argon2id hash of an unknown random test-only secret.
  -- Do not copy the owner's password hash into a site moderator account.
  INSERT INTO users(email_normalized,display_name,password_hash)
    VALUES('ci-registration-reviewer-${id}@tor-ci.example',
           'CI independent site reviewer',
           '$argon2id$v=19$m=65536,t=3,p=4$z7ZJQEv2POl28XE25rqZ0Q$6q/xM5vkRnX22eHDRTcjiepWi8G9o+LseO6uLgIObf4')
    RETURNING id INTO reviewer_id;
  IF reviewer_id=owner_id THEN
    RAISE EXCEPTION 'site reviewer must be independent';
  END IF;
  INSERT INTO account_roles(user_id,role,is_active)
    VALUES(reviewer_id,'moderator',TRUE);
  INSERT INTO book_registration_decisions
    (book_id,reviewer_user_id,outcome,expected_visibility_revision,
     resulting_visibility_revision)
    VALUES(${id},reviewer_id,'approved',old_rev,old_rev+1)
    RETURNING id INTO decision_id;
  UPDATE books SET visibility_status='active',
    visibility_status_revision=old_rev+1,
    registration_approval_decision_id=decision_id
    WHERE id=${id} AND visibility_status='pending_moderation'
      AND visibility_status_revision=old_rev;
  IF NOT FOUND THEN
    RAISE EXCEPTION 'lost the exact pending registration revision';
  END IF;
  INSERT INTO moderation_book_visibility_events
    (actor_user_id,book_id,before_status,after_status,before_revision,after_revision)
    VALUES(reviewer_id,${id},'pending_moderation','active',old_rev,old_rev+1);
END
$ci$;
SELECT id FROM books WHERE id=${id} AND visibility_status='active'
  AND registration_approval_decision_id IS NOT NULL;`;
  const out = execFileSync('psql', [
    '-X', '-A', '-t', '-q', '-v', 'ON_ERROR_STOP=1',
    '--dbname', raw, '--command', sql,
  ], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  if (out !== id) {
    throw new Error('Expected exactly one previously pending public Book');
  }
}

module.exports = { approvePendingBookForIsolatedCi };
