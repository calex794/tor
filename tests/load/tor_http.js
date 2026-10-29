// Isolated, read-only HTTP benchmark for the local TOR + PostgreSQL CI stack.
// IMPORTANT: intentionally hardcoded loopback; do not point at production.
import http from 'k6/http';
import { check } from 'k6';
import { Trend } from 'k6/metrics';

const allowedRates = [25, 50, 100, 250, 500];
const rate = Number(__ENV.TARGET_RPS || '25');
const seconds = Number(__ENV.STAGE_SECONDS || '15');

if (!allowedRates.includes(rate)) {
  throw new Error('TARGET_RPS must be one of 25,50,100,250,500');
}
if (![10, 15, 30, 60].includes(seconds)) {
  throw new Error('STAGE_SECONDS must be 10,15,30,60');
}

export const options = {
  discardResponseBodies: true,
  summaryTrendStats: ['avg', 'min', 'med', 'max', 'p(90)', 'p(95)', 'p(99)'],
  scenarios: {
    public_read_http: {
      executor: 'constant-arrival-rate',
      rate: rate,
      timeUnit: '1s',
      duration: seconds + 's',
      preAllocatedVUs: Math.max(20, Math.ceil(rate * 0.35)),
      maxVUs: Math.max(50, Math.ceil(rate * 1.25)),
      gracefulStop: '5s',
    },
  },
  thresholds: {
    http_req_failed: ['rate<0.01'],
    checks: ['rate>0.99'],
    dropped_iterations: ['count==0'],
  },
};

const routes = [
  { end: 0.15, path: '/', label: 'home' },
  { end: 0.35, path: '/catalog', label: 'catalog' },
  { end: 0.50, path: '/api/v1/books', label: 'catalog_api' },
  { end: 0.70, path: '/api/v1/search/books?q=Load&limit=20', label: 'search' },
  { end: 0.85, path: '/books/1', label: 'book' },
  { end: 0.95, path: '/reader/101', label: 'reader' },
  { end: 1.00, path: '/api/v1/books/1/toc?limit=20', label: 'toc' },
];

const routeDurations = {};
for (const route of routes) {
  routeDurations[route.label] = new Trend('tor_duration_' + route.label, true);
}

export default function () {
  const roll = Math.random();
  const route = routes.find((item) => roll < item.end) || routes[routes.length - 1];
  const response = http.get('http://127.0.0.1:3089' + route.path, {
    tags: { name: route.label },
    timeout: '5s',
  });
  check(response, { 'HTTP 200': (res) => res.status === 200 });
  routeDurations[route.label].add(response.timings.duration);
}
