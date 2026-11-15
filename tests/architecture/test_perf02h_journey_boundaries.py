"""Execute the real k6 purchase path across sequential stage boundaries."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = r"""
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(process.argv[1], 'utf8')
  .replace(/^import .*;\r?\n/gm, '')
  .replace(/^export /gm, '');
const expected = {25: 120, 50: 60, 100: 120};

function stage(rate, store, forceReplay = false) {
  const counters = new Map();
  let posts = 0;
  const context = {
    __ENV: {TOR_PERF252_JOURNEYS_PER_SECOND: String(rate)},
    exec: {scenario: {iterationInTest: 0}},
    Counter: class {
      constructor(name) { this.name = name; counters.set(name, 0); }
      add(n) { counters.set(this.name, counters.get(this.name) + n); }
    },
    Trend: class { add() {} },
    check: (response, checks) => Object.values(checks).every(f => f(response)),
    http: {
      post(url, body, options) {
        posts++;
        const payload = JSON.parse(body);
        const key = options.headers['idempotency-key'];
        let saved = store.get(key);
        const replayed = Boolean(saved) || forceReplay;
        if (saved) {
          assert.equal(saved.chapter_id, payload.chapter_id);
          assert.equal(saved.version_id, payload.expected_version_id);
        } else {
          saved = {purchase_id: String(store.size + 1),
            chapter_id: payload.chapter_id,
            version_id: payload.expected_version_id};
          store.set(key, saved);
        }
        return {status: 200, timings: {duration: 1},
          json: () => ({...saved, replayed})};
      },
      get(url) {
        const version = new URL(url).searchParams.get('version_id');
        return {status: 200, json: () => ({version_id: version})};
      },
    },
  };
  vm.createContext(context);
  vm.runInContext(source + ';this.buy = paidReader;', context);
  const buyers = Array.from({length: 8}, () =>
    ({session: 'fixture', csrfCookie: 'fixture', csrfValue: 'fixture'}));
  return {
    run(i) {
      context.exec.scenario.iterationInTest = i;
      context.buy({buyers});
    },
    count(name) { return counters.get(name); },
    posts() { return posts; },
  };
}

// k6 can schedule an iteration at the duration boundary. Execute the real
// script with both the nominal count and that extra iteration in EVERY stage.
for (const extra of [0, 1]) {
  const store = new Map();
  let total = 0;
  for (const rate of [25, 50, 100]) {
    const runner = stage(rate, store);
    const count = expected[rate] + extra;
    for (let i = 0; i < count; i++) runner.run(i);
    assert.equal(runner.count('purchase_journey_failures'), 0,
      'stage ' + rate + ' must not reuse the preceding stage purchase');
    assert.equal(runner.count('purchased_journeys'), count);
    total += count;
    assert.equal(store.size, total, 'every journey must buy a unique edition');
  }
  assert.equal(store.size, 300 + 3 * extra);
}

// Exhaustion still fails explicitly; it must not touch another stage's IDs.
for (const rate of [25, 50, 100]) {
  const store = new Map();
  const runner = stage(rate, store);
  runner.run(expected[rate] + 1);
  assert.equal(runner.count('purchase_journey_failures'), 1);
  assert.equal(runner.posts(), 0);
  assert.equal(store.size, 0);
}

// A replay in place of a NEW purchase is still a failure, even with HTTP 200.
const replay = stage(25, new Map(), true);
replay.run(0);
assert.equal(replay.count('purchase_journey_failures'), 1);
assert.equal(replay.count('purchased_journeys'), 0);
"""


class MixedJourneyBoundaryTests(unittest.TestCase):
    def test_actual_k6_purchase_path_preserves_disjoint_stage_fixtures(self):
        result = subprocess.run(
            ["node", "-e", SCRIPT,
             str(ROOT / "tests/load/tor_authenticated_progress.js")],
            text=True, capture_output=True, timeout=15, check=False,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
