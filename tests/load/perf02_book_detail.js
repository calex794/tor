// PERF-02f: comparable isolated book-detail GETs, with and without a live session.
// Only 127.0.0.1 is contacted; registered accounts exist only in disposable tor_ci.
import http from 'k6/http';
import { check } from 'k6';
import { Counter, Trend } from 'k6/metrics';

const BASE = 'http://127.0.0.1:3089';
const targetRps = Number(__ENV.PERF02_BOOK_RPS || '20');
const duration = Number(__ENV.PERF02_BOOK_SECONDS || '15');
if (![20, 50, 100, 200].includes(targetRps) || ![10, 15, 30].includes(duration)) {
  throw new Error('Unsupported PERF-02 isolated rate or duration');
}

const guestInvalid = new Counter('perf02_book_guest_invalid');
const readerInvalid = new Counter('perf02_book_reader_invalid');
const guestDuration = new Trend('perf02_book_guest_ms', true);
const readerDuration = new Trend('perf02_book_reader_ms', true);
const guestRate = Math.floor(targetRps / 2);
const readerRate = targetRps - guestRate;

export const options = {
  summaryTrendStats: ['avg', 'min', 'med', 'max', 'p(90)', 'p(95)', 'p(99)'],
  scenarios: {
    book_guests: {
      executor: 'constant-arrival-rate', exec: 'guestBook',
      rate: guestRate, timeUnit: '1s', duration: duration + 's',
      preAllocatedVUs: Math.max(8, Math.ceil(guestRate / 5)),
      maxVUs: Math.max(16, guestRate),
      gracefulStop: '5s',
    },
    book_readers: {
      executor: 'constant-arrival-rate', exec: 'readerBook',
      rate: readerRate, timeUnit: '1s', duration: duration + 's',
      preAllocatedVUs: Math.max(8, Math.ceil(readerRate / 5)),
      maxVUs: Math.max(16, readerRate),
      gracefulStop: '5s',
    },
  },
  thresholds: {
    checks: ['rate>0.99'],
    http_req_failed: ['rate<0.01'],
    dropped_iterations: ['count==0'],
    perf02_book_guest_invalid: ['count==0'],
    perf02_book_reader_invalid: ['count==0'],
  },
};

function cookie(response, name) {
  const stored = response.cookies[name];
  if (stored && stored.length) return name + '=' + stored[0].value;
  const raw = response.headers['Set-Cookie'];
  const values = Array.isArray(raw) ? raw : [raw || ''];
  for (const value of values) {
    const found = value.match(new RegExp('(?:^|,\\s*)(' + name + '=[^;]+)'));
    if (found) return found[1];
  }
  return null;
}

function csrf(html) {
  const found = html.match(/name="csrf_token" value="([^"]+)"/);
  return found && found[1];
}

function isBook(response) {
  return response.status === 200 &&
    typeof response.body === 'string' &&
    response.body.includes('Load Test Book');
}

export function setup() {
  const anonymous = http.get(BASE + '/books/1', { timeout: '10s' });
  if (!isBook(anonymous)) throw new Error('Disposable guest book fixture not available');

  const sessions = [];
  for (let n = 1; n <= 12; n++) {
    const registration = http.get(BASE + '/register', { timeout: '10s' });
    const csrfCookie = cookie(registration, 'tor-dev-csrf');
    const token = csrf(registration.body || '');
    if (registration.status !== 200 || !csrfCookie || !token) {
      throw new Error('PERF-02 registration form or CSRF missing');
    }
    // The setup VU cookie jar owns the CSRF cookie. Never duplicate it in POST.
    const registered = http.post(BASE + '/register', {
      email: 'perf02-book-' + targetRps + '-' + n + '@example.invalid',
      display_name: 'Perf Book Reader ' + n,
      password: 'IsolatedStrongPassword2026',
      csrf_token: token,
    }, { headers: { Origin: BASE }, redirects: 0, timeout: '10s' });
    const session = cookie(registered, 'tor-dev-session');
    if (registered.status !== 303 || !session) {
      throw new Error('PERF-02 disposable account registration failed: ' + registered.status);
    }
    // Check that this is a real authenticated Book request, not a guest 200.
    const authenticated = http.get(BASE + '/books/1', {
      headers: { Cookie: session }, timeout: '10s',
    });
    if (!isBook(authenticated) ||
        !authenticated.body.includes('book-hero-actions__follow') ||
        !authenticated.body.includes('/me/books/1/')) {
      throw new Error('PERF-02 session did not render authenticated book controls');
    }
    sessions.push(session);
  }
  // Session cookies stay within k6 setup/VU memory; no bearer reaches artifacts.
  return { sessions };
}

export function guestBook() {
  const response = http.get(BASE + '/books/1', {
    timeout: '10s', tags: { name: 'perf02_book_guest' },
  });
  guestDuration.add(response.timings.duration);
  if (!check(response, { 'guest Book renders': isBook })) guestInvalid.add(1);
}

export function readerBook(data) {
  const session = data.sessions[(__VU - 1) % data.sessions.length];
  const response = http.get(BASE + '/books/1', {
    headers: { Cookie: session },
    timeout: '10s', tags: { name: 'perf02_book_reader' },
  });
  readerDuration.add(response.timings.duration);
  if (!check(response, { 'authenticated Book renders': r =>
    isBook(r) && r.body.includes('book-hero-actions__follow') &&
    r.body.includes('/me/books/1/'),
  })) readerInvalid.add(1);
}
