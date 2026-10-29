// PERF-252: isolated read-only mixed baseline; real write profiles follow.
// All requests are hard-coded to the local test server.
import http from 'k6/http';
import { check } from 'k6';
import { Counter } from 'k6/metrics';

const BASE = 'http://127.0.0.1:3089';
const completed = new Counter('tor_completed_journeys');
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
  },
};
// Public-only routes: comments are registered only with TOR_AUTH_ENABLED=1.
const paths = ['/catalog', '/books/1', '/reader/101', '/api/v1/books/1/toc'];
export default function () {
  const success = paths.every((path) => {
    const reply = http.get(BASE + path, {
      tags: { name: 'journey_' + path.split('/')[1] },
      timeout: '10s',
    });
    return check(reply, { 'HTTP 200': (r) => r.status === 200 });
  });
  if (success) completed.add(1);
}
