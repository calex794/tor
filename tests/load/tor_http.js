// Isolated, read-only TOR HTTP load. Hardcoded loopback: no production traffic.
// Profiles separate expensive PostgreSQL paths from cheap HTTP and the generator.
import http from 'k6/http';
import { check } from 'k6';
import { Trend } from 'k6/metrics';

const allowedRates = [25, 50, 100, 200, 250, 300, 350, 400, 500];
const rate = Number(__ENV.TARGET_RPS || '25');
const seconds = Number(__ENV.STAGE_SECONDS || '15');
const profile = __ENV.TOR_LOAD_PROFILE || 'mixed';

if (!allowedRates.includes(rate)) {
  throw new Error('TARGET_RPS must be one of ' + allowedRates.join(','));
}
if (![10, 15, 30, 60].includes(seconds)) {
  throw new Error('STAGE_SECONDS must be 10,15,30,60');
}

const routes = [
  { end: 0.15, path: '/', label: 'home' },
  { end: 0.35, path: '/catalog', label: 'catalog' },
  { end: 0.50, path: '/api/v1/books', label: 'catalog_api' },
  { end: 0.70, path: '/api/v1/search/books?q=Load&limit=20', label: 'search' },
  { end: 0.85, path: '/books/1', label: 'book' },
  { end: 0.95, path: '/reader/101', label: 'reader' },
  { end: 1.00, path: '/api/v1/books/1/toc?limit=20', label: 'toc' },
];

// Isolate the app, query distribution and generator on the same CI runner.
// "search" deliberately matches all 10k test books; "search_rare" matches one.
const profileRoutes = {
  mixed: routes,
  health: [{ end: 1, path: '/healthz', label: 'health' }],
  cheap: [
    { end: 0.40, path: '/reader/101', label: 'reader' },
    { end: 0.75, path: '/api/v1/books', label: 'catalog_api' },
    { end: 1, path: '/', label: 'home' },
  ],
  catalog: [{ end: 1, path: '/catalog', label: 'catalog' }],
  catalog_cursor: [{ end: 1, path: '/catalog?after=20', label: 'catalog_cursor' }],
  search: [{ end: 1, path: '/api/v1/search/books?q=Load&limit=20', label: 'search' }],
  search_rare: [{ end: 1, path: '/api/v1/search/books?q=9999&limit=20', label: 'search_rare' }],
};
if (!(profile in profileRoutes)) {
  throw new Error('Unknown TOR_LOAD_PROFILE: ' + profile);
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
    http_req_duration: ['p(95)<500'],
    checks: ['rate>0.99'],
    dropped_iterations: ['count==0'],
  },
};

const routeDurations = {};
for (const route of routes.concat(
  profileRoutes.health, profileRoutes.catalog_cursor, profileRoutes.search_rare,
)) {
  routeDurations[route.label] = new Trend('tor_duration_' + route.label, true);
}

export default function () {
  const selected = profileRoutes[profile];
  const roll = Math.random();
  const route = selected.find((item) => roll < item.end) || selected[selected.length - 1];
  const response = http.get('http://127.0.0.1:3089' + route.path, {
    tags: { name: route.label },
    timeout: '5s',
  });
  check(response, { 'HTTP 200': (res) => res.status === 200 });
  routeDurations[route.label].add(response.timings.duration);
}
