'use strict';
// Consent-gated, real-session organic activation load. Disposable CI only.
// Measures POST alone (not SSR ticket issuance or unrelated catalogue reads).
const {randomBytes, createHash} = require('node:crypto');
const assert = require('node:assert/strict');
const fs = require('node:fs');

const RATE = 25;
const COUNT = 150;
const P95_BUDGET_MS = 750;

function digest(value) {
  return createHash('sha256').update(value).digest('hex');
}

async function benchmarkOrganicActivation({page, sql, user, book, base, db}) {
  assert.equal(process.env.GITHUB_ACTIONS, 'true', 'never benchmark outside isolated CI');
  assert.match(base, /^http:\/\/127\.0\.0\.1:\d+$/);
  assert.match(db, /^postgresql:\/\/tor_app:[^@]+@127\.0\.0\.1:5432\/tor_ci$/);
  assert(Number.isSafeInteger(user) && user > 0);
  assert(Number.isSafeInteger(book) && book > 0);

  // The browser must already be authenticated and the reader must have
  // explicitly enabled REC consent; no fake auth or hand-made CSRF headers.
  const response = await page.goto(base + '/catalog?sort=newest');
  assert.equal(response.status(), 200);
  const cookies = await page.context().cookies(base);
  const session = cookies.find(c => c.name === 'tor-dev-session')?.value;
  const csrf = cookies.find(c => c.name === 'tor-dev-csrf')?.value;
  assert.match(session || '', /^[A-Za-z0-9_-]{43}$/);
  assert.match(csrf || '', /^[A-Za-z0-9_-]{43}$/);
  const subject = sql('SELECT subject_id::text FROM rec_subjects WHERE user_id=' + user + ' AND enabled');
  assert.match(subject, /^[0-9a-f-]{36}$/i);
  assert.equal(sql('SELECT count(*) FROM rec_public_books WHERE id=' + book), '1');

  // Seed only opaque ticket digests in tor_ci. No browser query, session
  // token, CSRF secret or identifying user data is persisted.
  const sessionDigest = digest(session);
  const tickets = Array.from({length:COUNT}, () => randomBytes(32).toString('base64url'));
  const hashes = tickets.map(digest);
  const values = hashes.map(h => "('" + subject + "'::uuid,decode('" +
    sessionDigest + "','hex'),decode('" + h + "','hex')," + book + ",'catalog',1)");
  sql('INSERT INTO organic_discovery_touches ' +
      '(subject_id,session_digest,ticket_digest,book_id,source,position) VALUES ' +
      values.join(','));

  // Browser-origin fetch sends its real cookie, Origin and CSRF. Constant
  // 25 arrivals/s, independent of previous requests' completion.
  const sample = await page.evaluate(async ({tickets, csrf, rate}) => {
    const start = performance.now();
    const promises = tickets.map((ticket, i) => new Promise(resolve => {
      setTimeout(async () => {
        const begun = performance.now();
        try {
          const result = await fetch('/api/v1/discovery/activate', {
            method: 'POST',
            credentials: 'same-origin',
            headers: {'Content-Type': 'application/x-www-form-urlencoded'},
            body: new URLSearchParams({ticket, csrf_token: csrf})
          });
          resolve({status: result.status, latency: performance.now() - begun});
        } catch (err) {
          resolve({status: 0, latency: performance.now() - begun});
        }
      }, Math.round(i * 1000 / rate));
    }));
    const results = await Promise.all(promises);
    return {results, elapsed: performance.now() - start};
  }, {tickets, csrf, rate:RATE});

  const statuses = new Map();
  for (const r of sample.results) statuses.set(r.status, (statuses.get(r.status) || 0) + 1);
  const latencies = sample.results.map(r => r.latency).sort((a, b) => a - b);
  const p95 = latencies[Math.ceil(latencies.length * 0.95) - 1];
  const p50 = latencies[Math.ceil(latencies.length * 0.50) - 1];
  const actualRps = COUNT * 1000 / sample.elapsed;
  const hashed = hashes.map(h => "decode('" + h + "','hex')").join(',');
  const credited = Number(sql('SELECT count(*) FROM organic_discovery_touches ' +
    'WHERE book_id=' + book + ' AND ticket_digest IN (' + hashed +
    ') AND clicked_at IS NOT NULL'));
  const ok = sample.results.filter(r => r.status === 204).length;

  const metric = 'REC13 ORGANIC AUTH POST ' + COUNT + ' unique tickets @ target ' +
    RATE + '/s: 204=' + ok + ', p50=' + p50.toFixed(1) + 'ms, p95=' +
    p95.toFixed(1) + 'ms, actual=' + actualRps.toFixed(1) +
    '/s, SQL clicks=' + credited;
  console.log(metric);
  if (process.env.GITHUB_STEP_SUMMARY) {
    fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY,
      '\n### Consent-gated organic POST, disposable tor_ci\n\n' +
      '| Target | 204 responses | p50 | p95 | Achieved | SQL clicks |\n' +
      '|---:|---:|---:|---:|---:|---:|\n| ' + RATE + ' req/s | ' + ok +
      '/' + COUNT + ' | ' + p50.toFixed(1) + ' ms | ' +
      p95.toFixed(1) + ' ms | ' + actualRps.toFixed(1) +
      ' req/s | ' + credited + '/' + COUNT + ' |\n\n' +
      'CI-hosted benchmark of authenticated activation only; not a production capacity estimate.\n');
  }
  assert.equal(ok, COUNT, 'all real-session activations must succeed: ' + JSON.stringify([...statuses]));
  assert.equal(credited, COUNT, 'every successful activation must persist one click');
  assert(actualRps >= 20, 'auth POST throughput below 20 req/s: ' + metric);
  assert(p95 <= P95_BUDGET_MS, 'auth POST p95 exceeds 750ms: ' + metric);
}

module.exports = {benchmarkOrganicActivation};
