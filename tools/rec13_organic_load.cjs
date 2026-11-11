'use strict';
// Retired organic activation: real-session compatibility load. Disposable CI only.
// Measures private410 POST alone, with no click/write acknowledged as successful.
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

  // The browser is genuinely authenticated without an analytics subject.
  // Use an authenticated page with a deliberate same-origin connect policy.
  // Catalogue HTML keeps its strict CSP after the analytics scripts are retired.
  const response = await page.goto(base + '/me/recommendations');
  assert.equal(response.status(), 200);
  const cookies = await page.context().cookies(base);
  const session = cookies.find(c => c.name === 'tor-dev-session')?.value;
  const csrf = cookies.find(c => c.name === 'tor-dev-csrf')?.value;
  assert.match(session || '', /^[A-Za-z0-9_-]{43}$/);
  assert.match(csrf || '', /^[A-Za-z0-9_-]{43}$/);
  assert.equal(sql('SELECT count(*) FROM rec_subjects WHERE user_id=' + user), '0');
  assert.equal(sql('SELECT count(*) FROM rec_public_books WHERE id=' + book), '1');

  // Valid opaque old-client payloads are not represented as issued tickets.
  // Pre-barrier stored assignments belong to the installed migration test.
  const tickets = Array.from({length:COUNT}, () => randomBytes(32).toString('base64url'));
  const hashes = tickets.map(digest);

  const {assertCurrentSchema,optionalSnapshot}=require('./collection_retirement_fixtures.cjs')(sql);
  assertCurrentSchema();
  const historical=optionalSnapshot(user);

  // Browser-origin fetch sends its real cookie, Origin and CSRF. Constant
  // 25 arrivals/s, independent of previous requests' completion.
  const sample = await page.evaluate(async ({tickets, csrf, rate}) => {
    // Correlate each network timing record with a request index, not its
    // sensitive ticket. Bounded to the disposable browser load fixture.
    performance.setResourceTimingBufferSize(1000);
    performance.clearResourceTimings();
    const start = performance.now();
    let inFlight = 0;
    let peakInFlight = 0;
    const promises = tickets.map((ticket, i) => new Promise(resolve => {
      const scheduledMs = Math.round(i * 1000 / rate);
      setTimeout(async () => {
        const begun = performance.now();
        const dispatchLag = Math.max(0, begun - start - scheduledMs);
        inFlight += 1;
        peakInFlight = Math.max(peakInFlight, inFlight);
        try {
          const result = await fetch('/api/v1/discovery/activate?rec13_slot=' + i, {
            method: 'POST',
            credentials: 'same-origin',
            headers: {'Content-Type': 'application/x-www-form-urlencoded'},
            body: new URLSearchParams({ticket, csrf_token: csrf})
          });
          const body=await result.json();
          const privateRetired=result.headers.get('cache-control')==='no-store'
            && /noindex/.test(result.headers.get('x-robots-tag') || '')
            && /nofollow/.test(result.headers.get('x-robots-tag') || '')
            && body.error?.code==='COLLECTION_RETIRED';
          const finished = performance.now();
          resolve({slot: i, status: result.status, privateRetired, latency: finished - begun,
            dispatchLag, dispatchedAt: begun - start, finishedAt: finished - start});
        } catch (err) {
          const finished = performance.now();
          resolve({slot: i, status: 0, latency: finished - begun,
            dispatchLag, dispatchedAt: begun - start, finishedAt: finished - start});
        } finally {
          inFlight -= 1;
        }
      }, scheduledMs);
    }));
    const results = await Promise.all(promises);
    const elapsed = performance.now() - start;
    // Resource Timing entries may be published a task after fetch() resolves
    // after the response body. Wait only after measuring latency/throughput; never
    // silently drop a slow request from the independent HTTP p95 gate.
    const resourcePrefix = location.origin + '/api/v1/discovery/activate?rec13_slot=';
    let entries = [];
    for (let attempt = 0; attempt < 10; attempt += 1) {
      entries = performance.getEntriesByType('resource');
      if (entries.filter(e => e.name.startsWith(resourcePrefix)).length >= tickets.length) {
        break;
      }
      await new Promise(resolve => setTimeout(resolve, 20));
    }
    const network = new Map();
    for (const entry of entries) {
      const url = new URL(entry.name);
      if (url.origin !== location.origin ||
          url.pathname !== '/api/v1/discovery/activate') continue;
      const slotParam = url.searchParams.get('rec13_slot');
      if (slotParam === null) continue;
      const slot = Number(slotParam);
      if (Number.isSafeInteger(slot) && slot >= 0 && slot < tickets.length) {
        network.set(slot, {
          queueMs: entry.requestStart - entry.fetchStart,
          firstByteMs: entry.responseStart - entry.requestStart
        });
      }
    }
    return {
      results: results.map(r => ({...r, network: network.get(r.slot) || null})),
      resourceCount: network.size,
      elapsed,
      peakInFlight
    };
  }, {tickets, csrf, rate:RATE});

  const statuses = new Map();
  for (const r of sample.results) statuses.set(r.status, (statuses.get(r.status) || 0) + 1);
  const latencies = sample.results.map(r => r.latency).sort((a, b) => a - b);
  const p95 = latencies[Math.ceil(latencies.length * 0.95) - 1];
  const p50 = latencies[Math.ceil(latencies.length * 0.50) - 1];
  const actualRps = COUNT * 1000 / sample.elapsed;
  // The HTTP p95 below is deliberately unchanged. Dispatch diagnostics
  // distinguish timer stalls/bunched arrivals from requests that waited after
  // dispatch. No tickets, cookies, CSRF values or user IDs leave the browser.
  const dispatchLag = sample.results.map(r => r.dispatchLag).sort((a, b) => a - b);
  const p95DispatchLag = dispatchLag[Math.ceil(dispatchLag.length * 0.95) - 1];
  const maxDispatchLag = dispatchLag[dispatchLag.length - 1];
  // Chromium Resource Timing separates fetch queuing/connection setup from
  // first-byte wait (server + loopback). It is advisory: absence never bypasses
  // the original HTTP latency or correctness assertions.
  const percentile95 = values => {
    const sorted = values.slice().sort((a, b) => a - b);
    return sorted.length ? sorted[Math.ceil(sorted.length * 0.95) - 1].toFixed(1) : 'n/a';
  };
  const queueTimes = sample.results
    .filter(r => r.network && r.network.queueMs >= 0)
    .map(r => r.network.queueMs);
  const firstByteTimes = sample.results
    .filter(r => r.network && r.network.firstByteMs >= 0)
    .map(r => r.network.firstByteMs);
  const networkTiming = 'REC13 NETWORK timing (diagnostic, not gate): samples=' +
    sample.resourceCount + '/' + COUNT + ', browser-queue-p95=' +
    percentile95(queueTimes) + 'ms, first-byte-p95=' +
    percentile95(firstByteTimes) + 'ms';
  const dispatched = sample.results.slice().sort((a, b) => a.dispatchedAt - b.dispatchedAt);
  const bunchedGaps = dispatched.slice(1).filter(
    (r, i) => r.dispatchedAt - dispatched[i].dispatchedAt < 5
  ).length;
  const worst = sample.results.slice().sort((a, b) => b.latency - a.latency).slice(0, 12)
    .map(r => 'slot=' + r.slot + ':status=' + r.status +
      ':http=' + r.latency.toFixed(1) + 'ms:dispatch-lag=' +
      r.dispatchLag.toFixed(1) + 'ms:at=' + r.dispatchedAt.toFixed(1) +
      'ms:queue=' + (r.network ? r.network.queueMs.toFixed(1) : 'n/a') +
      'ms:first-byte=' + (r.network ? r.network.firstByteMs.toFixed(1) : 'n/a') + 'ms')
    .join(', ');
  const timing = 'REC13 CLIENT timing (diagnostic, not gate): dispatch-lag-p95=' +
    p95DispatchLag.toFixed(1) + 'ms, dispatch-lag-max=' +
    maxDispatchLag.toFixed(1) + 'ms, bunched-gaps-under-5ms=' +
    bunchedGaps + '/' + (COUNT - 1) + ', peak-in-flight=' + sample.peakInFlight;
  const hashed = hashes.map(h => "decode('" + h + "','hex')").join(',');
  const credited = Number(sql('SELECT count(*) FROM organic_discovery_touches ' +
    'WHERE book_id=' + book + ' AND ticket_digest IN (' + hashed +
    ') AND clicked_at IS NOT NULL'));
  const ok = sample.results.filter(r => r.status === 410 && r.privateRetired).length;

  const metric = 'REC13 RETIRED ORGANIC AUTH POST ' + COUNT + ' unique tickets @ target ' +
    RATE + '/s: private410=' + ok + ', p50=' + p50.toFixed(1) + 'ms, p95=' +
    p95.toFixed(1) + 'ms, actual=' + actualRps.toFixed(1) +
    '/s, SQL clicks=' + credited;
  console.log(metric);
  console.log(timing);
  console.log(networkTiming);
  if (p95 > P95_BUDGET_MS) {
    // Indices only: enough to locate a burst without leaking signed tickets.
    console.error('REC13 SLOWEST HTTP slots: ' + worst);
  }
  if (process.env.GITHUB_STEP_SUMMARY) {
    fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY,
      '\n### Retired organic POST, disposable tor_ci\n\n' +
      '| Target | Private410 responses | p50 | p95 | Achieved | SQL clicks |\n' +
      '|---:|---:|---:|---:|---:|---:|\n| ' + RATE + ' req/s | ' + ok +
      '/' + COUNT + ' | ' + p50.toFixed(1) + ' ms | ' +
      p95.toFixed(1) + ' ms | ' + actualRps.toFixed(1) +
      ' req/s | ' + credited + ' |\n\n' +
      'CI compatibility benchmark: every authenticated request is rejected as retired; no production capacity estimate.\n\n' +
      timing + '\n' + networkTiming + '\n');
  }
  assert.equal(ok, COUNT, 'all real-session legacy commands must return private410: ' + JSON.stringify([...statuses]));
  assert.equal(credited, 0, 'a retired command must never persist a click');
  assert.deepEqual(optionalSnapshot(user),historical,'retired client payloads introduced an analytics subject or optional store');
  assert(actualRps >= 20, 'auth POST throughput below 20 req/s: ' + metric);
  assert(p95 <= P95_BUDGET_MS, 'auth POST p95 exceeds 750ms: ' + metric);
}

module.exports = {benchmarkOrganicActivation};
