import http from 'k6/http';
import exec from 'k6/execution';
import { check } from 'k6';
import { Counter } from 'k6/metrics';

const base = 'http://127.0.0.1:3089';
const failures = new Counter('journey_failures');
const completed = new Counter('journeys_written');
const savedComments = new Counter('comments_written');
const publicJourneys = new Counter('public_journeys');
const publicFailures = new Counter('public_journey_failures');
const purchased = new Counter('purchased_journeys');
const purchaseFailures = new Counter('purchase_journey_failures');
// 25 full journeys/s: 13 progress/comment, 10 public reading, 2 paid.
export const options = {
  summaryTrendStats: ['avg', 'min', 'med', 'max', 'p(90)', 'p(95)', 'p(99)'],
  scenarios: {
    writers: {
      executor: 'constant-arrival-rate', exec: 'authenticatedWriter',
      rate: 13, timeUnit: '1s', duration: '60s',
      preAllocatedVUs: 24, maxVUs: 32,
    },
    readers: {
      executor: 'constant-arrival-rate', exec: 'publicReader',
      rate: 10, timeUnit: '1s', duration: '60s',
      preAllocatedVUs: 16, maxVUs: 24,
    },
    buyers: {
      executor: 'constant-arrival-rate', exec: 'paidReader',
      rate: 2, timeUnit: '1s', duration: '60s',
      preAllocatedVUs: 4, maxVUs: 8,
    },
  },
  thresholds: {
    checks: ['rate>0.99'],
    dropped_iterations: ['count==0'],
    journey_failures: ['count==0'],
    public_journey_failures: ['count==0'],
    comments_written: ['count>0'],
    journeys_written: ['count>0'],
    public_journeys: ['count>0'],
    purchased_journeys: ['count>0'],
    purchase_journey_failures: ['count==0'],
  },
};
function pageLink(html, rel) {
  const match = html.match(new RegExp('rel="' + rel + '" href="([^"]+)"'));
  if (!match) return null;
  const path = match[1].replace(/&amp;/g, '&');
  return path.startsWith('/catalog?') ? path : null;
}
function countCards(html) {
  return (html.match(/class="book-mini-card book-mini-card--compact"/g) || []).length;
}
export function publicReader() {
  const first = http.get(base + '/catalog?sort=oldest', { tags: { name: 'catalog_first' }, timeout: '10s' });
  const next = pageLink(first.body || '', 'next');
  if (!check(first, { 'first page is bounded': r =>
    r.status === 200 && countCards(r.body) === 20,
  }) || !next) { publicFailures.add(1); return; }
  const second = http.get(base + next, { tags: { name: 'catalog_second' }, timeout: '10s' });
  const back = pageLink(second.body || '', 'prev');
  if (!check(second, { 'next page is bounded': r =>
    r.status === 200 && countCards(r.body) === 20 && r.body.includes('Страница 2'),
  }) || !back) { publicFailures.add(1); return; }
  const previous = http.get(base + back, { tags: { name: 'catalog_previous' }, timeout: '10s' });
  if (!check(previous, { 'reverse navigation works': r =>
    r.status === 200 && r.body.includes('Страница 1'),
  })) { publicFailures.add(1); return; }
  for (const path of ['/books/1', '/reader/101', '/api/v1/books/1/toc']) {
    const response = http.get(base + path, { tags: { name: 'public_reading' }, timeout: '10s' });
    if (!check(response, { 'public chapter 200': r => r.status === 200 })) {
      publicFailures.add(1); return;
    }
  }
  publicJourneys.add(1);
}
function cookie(response, name) {
  const stored = response.cookies[name];
  if (stored && stored.length) return name + '=' + stored[0].value;
  const items = response.headers['Set-Cookie'];
  const values = Array.isArray(items) ? items : [items || ''];
  for (const item of values) {
    const match = item.match(new RegExp('(?:^|,\\s*)(' + name + '=[^;]+)'));
    if (match) return match[1];
  }
  return null;
}
function csrf(html) {
  const match = html.match(/name="csrf_token" value="([^"]+)"/);
  return match && match[1];
}
export function setup() {
  const users = [];
  for (let n = 1; n <= 32; n++) {
    const page = http.get(base + '/register');
    const csrfCookie = cookie(page, 'tor-dev-csrf');
    const key = csrf(page.body);
    if (!csrfCookie || !key) throw new Error('registration CSRF failed');
    const form = {
      email: 'perf252-' + n + '@example.invalid',
      display_name: 'Perf Reader ' + n,
      password: 'IsolatedStrongPassword2026',
      csrf_token: key,
    };
    // k6's setup cookie jar already holds the CSRF cookie from GET /register.
    // A second explicit Cookie header would cause TOR's duplicate-cookie guard
    // to reject the registration with HTTP 403.
    const registered = http.post(base + '/register', form, {
      headers: { Origin: base },
      redirects: 0,
    });
    const session = cookie(registered, 'tor-dev-session');
    if (registered.status !== 303 || !session) throw new Error('registration failed ' + registered.status);
    // The registration token remains valid after the session is issued.
    // /me normally reuses the same token without setting another cookie.
    users.push({ session, csrfCookie, csrfValue: key });
  }
  const buyers = [];
  for (let n = 1; n <= 8; n++) {
    const page = http.get(base + '/login');
    const csrfCookie = cookie(page, 'tor-dev-csrf');
    const token = csrf(page.body);
    if (!csrfCookie || !token) throw new Error('buyer login CSRF missing');
    const logged = http.post(base + '/login', {
      email: 'perf252-paid-buyer-' + n + '@tor-ci.example',
      password: 'D04VeryStrongBuyerPassword2026', csrf_token: token,
    }, { headers: { Origin: base }, redirects: 0 });
    const session = cookie(logged, 'tor-dev-session');
    if (logged.status !== 303 || !session) {
      throw new Error('buyer login failed: ' + logged.status);
    }
    buyers.push({ session, csrfCookie, csrfValue: token });
  }
  return { users, buyers };
}
export function paidReader(data) {
  // Unique across every VU, even when the scheduler reuses VUs under load.
  const ordinal = exec.scenario.iterationInTest + 1;
  if (ordinal > 300) { purchaseFailures.add(1); return; }
  const actor = data.buyers[(ordinal - 1) % data.buyers.length];
  const chapter = (2000000 + ordinal).toString();
  const version = (3000000 + ordinal).toString();
  const headers = {
    Cookie: actor.session + '; ' + actor.csrfCookie,
    Origin: base,
    'x-csrf-token': actor.csrfValue,
    'Content-Type': 'application/json',
    'idempotency-key': ('P' + String(ordinal).padStart(8, '0')).padEnd(42, 'A') + '8',
  };
  const body = JSON.stringify({
    chapter_id: chapter, expected_version_id: version, expected_price_minor: 100,
  });
  const first = http.post(base + '/api/v1/purchases', body, {
    headers, tags: { name: 'paid_checkout' }, timeout: '10s',
  });
  if (!check(first, { 'paid checkout succeeds once': r => r.status === 200 })) {
    purchaseFailures.add(1); return;
  }
  const receipt = first.json();
  if (receipt.replayed || receipt.chapter_id !== chapter ||
      receipt.version_id !== version || !receipt.purchase_id) {
    purchaseFailures.add(1); return;
  }
  if (ordinal % 10 === 0) {
    const replay = http.post(base + '/api/v1/purchases', body, {
      headers, tags: { name: 'paid_idempotent_replay' }, timeout: '10s',
    });
    if (!check(replay, { 'retry does not charge again': r =>
      r.status === 200 && r.json().replayed === true &&
      r.json().purchase_id === receipt.purchase_id,
    })) { purchaseFailures.add(1); return; }
  }
  const uri = base + '/api/v1/me/progress/purchased?purchase_id=' +
    receipt.purchase_id + '&chapter_id=' + chapter + '&version_id=' + version;
  const access = http.get(uri, {
    headers, tags: { name: 'paid_pinned_read' }, timeout: '10s',
  });
  if (!check(access, { 'pinned paid edition is accessible': r =>
    r.status === 200 && r.json().version_id === version,
  })) { purchaseFailures.add(1); return; }
  purchased.add(1);
}
export function authenticatedWriter(data) {
  const user = data.users[(__VU - 1) % data.users.length];
  const headers = {
    Cookie: user.session + '; ' + user.csrfCookie,
    Origin: base,
  };
  const lookup = http.get(base + '/api/v1/me/progress/free?chapter_id=401', { headers, tags: { name: 'progress_read' } });
  if (!check(lookup, { 'progress read': r => r.status === 200 })) { failures.add(1); return; }
  const prior = lookup.json();
  const position = Number(prior.revision) + 1;
  const saved = http.put(base + '/api/v1/me/progress/free', JSON.stringify({
    chapter_id: '401', version_id: '501', block_id: 1,
    char_offset: position, expected_revision: prior.revision,
  }), { headers: Object.assign({}, headers, { 'x-csrf-token': user.csrfValue, 'Content-Type': 'application/json' }), tags: { name: 'progress_write' } });
  if (!check(saved, { 'progress saved': r => r.status === 200 })) { failures.add(1); return; }
  const receipt = saved.json();
  if (receipt.revision !== position) { failures.add(1); return; }
  const verify = http.get(base + '/api/v1/me/progress/free?chapter_id=401', { headers, tags: { name: 'progress_verify' } });
  if (!check(verify, { 'progress persisted': r => r.status === 200 })) {
    failures.add(1);
    return;
  }
  const persisted = verify.json();
  if (persisted.revision !== receipt.revision || persisted.char_offset !== position) {
    failures.add(1);
    return;
  }
  // Isolated test book: every fifth journey reads and publishes a comment.
  if ((__ITER + __VU) % 5 === 0) {
    const discussion = http.get(base + '/books/1/comments', {
      headers,
      tags: { name: 'reader_discussion_list' },
    });
    if (!check(discussion, { 'comments readable': r => r.status === 200 })) {
      failures.add(1);
      return;
    }
    // Use the real server-issued one-use command key from the HTML form.
    const issuedKey = discussion.body.match(/name="command_key" value="([A-Za-z0-9_-]{43})"/);
    if (!issuedKey) {
      failures.add(1);
      return;
    }
    const body = 'PERF252 LOAD reader ' + __VU + ' iteration ' + __ITER;
    const posted = http.post(base + '/books/1/comments', {
      body,
      command_key: issuedKey[1],
      csrf_token: user.csrfValue,
    }, {
      headers,
      redirects: 0,
      tags: { name: 'reader_comment_create' },
    });
    if (!check(posted, { 'comment created': r => r.status === 303 })) {
      failures.add(1);
      return;
    }
    const reloaded = http.get(base + '/books/1/comments', {
      headers,
      tags: { name: 'reader_comment_verify' },
    });
    if (!check(reloaded, {
      'comment persisted in rendered list': r => r.status === 200 && r.body.includes(body),
    })) {
      failures.add(1);
      return;
    }
    savedComments.add(1);
  }
  completed.add(1);
}
