// PERF-252: isolated read-only mixed baseline; real write profiles follow.
// All requests are hard-coded to the local test server.
import http from 'k6/http';
import { check } from 'k6';
import { Counter } from 'k6/metrics';

const BASE = 'http://127.0.0.1:3089';
const completed = new Counter('tor_completed_journeys');
const failed = new Counter('tor_failed_journeys');
const rate = Number(__ENV.TOR_JOURNEYS_PER_SECOND || '25');
const duration = Number(__ENV.TOR_JOURNEY_SECONDS || '60');
if (![25, 50, 100, 250].includes(rate) || ![60, 120, 300].includes(duration)) {
  throw new Error('Unsupported isolated load settings');
}
export const options = {
  scenarios: {
    public_journeys: {
      executor: 'constant-arrival-rate',
      rate, timeUnit: '1s', duration: duration + 's',
      preAllocatedVUs: Math.max(30, Math.ceil(rate * 0.5)),
      maxVUs: Math.max(60, rate * 2),
    },
  },
  thresholds: {
    checks: ['rate>0.99'],
    dropped_iterations: ['count==0'],
    http_req_failed: ['rate<0.01'],
    tor_failed_journeys: ['count==0'],
    tor_completed_journeys: ['count>0'],
  },
};
// Exercise B-18 page 1 -> 2 -> 1 against 10k books; no browser receives 10k cards.
function pageLink(html, rel) {
  const match = html.match(new RegExp('rel="' + rel + '" href="([^"]+)"'));
  if (!match) return null;
  const href = match[1].replace(/&amp;/g, '&');
  return href.startsWith('/catalog?') ? href : null;
}
function countCards(html) {
  return (html.match(/class="book-mini-card book-mini-card--compact"/g) || []).length;
}
function request(path) {
  return http.get(BASE + path, { timeout: '10s' });
}
export default function () {
  const useFilter = (__VU + __ITER) % 5 === 0;
  const first = request(useFilter ? '/catalog?genre=fantasy&sort=title' : '/catalog?sort=oldest');
  const next = pageLink(first.body || '', 'next');
  if (!check(first, { 'page 1: exactly 20 books': r =>
    r.status === 200 && countCards(r.body) === 20,
  }) || !next) { failed.add(1); return; }
  const second = request(next);
  const previous = pageLink(second.body || '', 'prev');
  if (!check(second, { 'page 2: exactly 20 books': r =>
    r.status === 200 && countCards(r.body) === 20 && r.body.includes('Страница 2'),
  }) || !previous || (useFilter && !next.includes('genre=fantasy'))) { failed.add(1); return; }
  const back = request(previous);
  if (!check(back, { 'page 2 -> 1': r =>
    r.status === 200 && countCards(r.body) === 20 && r.body.includes('Страница 1'),
  })) { failed.add(1); return; }
  for (const path of ['/books/1', '/reader/101', '/api/v1/books/1/toc']) {
    if (!check(request(path), { 'book/reader/toc 200': r => r.status === 200 })) { failed.add(1); return; }
  }
  completed.add(1);
}
