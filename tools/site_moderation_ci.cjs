// CI-only approval fixture for unrelated public author/cover browser journeys.
// New real books start pending_moderation; test code never grants an author
// moderation authority. The HTTP role, CAS and audit proof lives in S020 PG.
const { execFileSync } = require('node:child_process');

function approvePendingBookForIsolatedCi(bookId) {
  const raw = process.env.TEST_DATABASE_URL;
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
  const sql = `UPDATE books
    SET visibility_status='active', visibility_status_revision=visibility_status_revision+1
    WHERE id=${id} AND visibility='public' AND visibility_status='pending_moderation'
    RETURNING id;`;
  const out = execFileSync('psql', [
    '-X', '-A', '-t', '-q', '-v', 'ON_ERROR_STOP=1',
    '--dbname', raw, '--command', sql,
  ], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  if (out !== id) {
    throw new Error('Expected exactly one previously pending public Book');
  }
}

module.exports = { approvePendingBookForIsolatedCi };
