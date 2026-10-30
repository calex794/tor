import http from 'k6/http';
import { check } from 'k6';
import { Counter } from 'k6/metrics';

const base = 'http://127.0.0.1:3089';
const failures = new Counter('journey_failures');
const completed = new Counter('journeys_written');
const savedComments = new Counter('comments_written');
export const options = {
  scenarios: { writers: {
    executor: 'constant-arrival-rate', rate: 25, timeUnit: '1s',
    duration: '60s', preAllocatedVUs: 32, maxVUs: 32,
  } },
  thresholds: { checks: ['rate>0.99'], dropped_iterations: ['count==0'], journey_failures: ['count==0'], comments_written: ['count>0'] },
};
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
  return { users };
}
export default function (data) {
  const user = data.users[(__VU - 1) % data.users.length];
  const headers = {
    Cookie: user.session + '; ' + user.csrfCookie,
    Origin: base,
  };
  const lookup = http.get(base + '/api/v1/me/progress/free?chapter_id=401', { headers });
  if (!check(lookup, { 'progress read': r => r.status === 200 })) { failures.add(1); return; }
  const prior = lookup.json();
  const position = Number(prior.revision) + 1;
  const saved = http.put(base + '/api/v1/me/progress/free', JSON.stringify({
    chapter_id: '401', version_id: '501', block_id: 1,
    char_offset: position, expected_revision: prior.revision,
  }), { headers: Object.assign({}, headers, { 'x-csrf-token': user.csrfValue, 'Content-Type': 'application/json' }) });
  if (!check(saved, { 'progress saved': r => r.status === 200 })) { failures.add(1); return; }
  const receipt = saved.json();
  if (receipt.revision !== position) { failures.add(1); return; }
  const verify = http.get(base + '/api/v1/me/progress/free?chapter_id=401', { headers });
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
