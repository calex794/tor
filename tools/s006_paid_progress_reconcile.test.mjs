// Execute the real paid reader's exported save path without S3 credentials.
// A missing COMMIT acknowledgement is reconciled only by an actor-scoped GET.
import assert from "node:assert/strict";
import test from "node:test";
import { savePaidProgressWithReconcile } from "../crates/web/assets/newboo-paid-reader.mjs";

const endpoint = "/api/v1/me/progress/purchased";
const lookup = "chapter_id=101&purchase_id=301&version_id=201";
const csrf = "a".repeat(43);
const command = {
  chapter_id: "101", purchase_id: "301", version_id: "201",
  block_id: 1, char_offset: 2, expected_revision: 1,
};

function fixture(mode) {
  const calls = [];
  let revision = 1, offset = 1;
  const state = () => ({
    chapter_id: "101", version_id: "201",
    block_id: 1, char_offset: offset, revision,
  });
  const request = async (url, options = {}) => {
    calls.push({ url, options });
    if (options.method === "PUT") {
      assert.equal(url, endpoint);
      assert.equal(options.headers["X-CSRF-Token"], csrf);
      assert.deepEqual(JSON.parse(options.body), command);
      if (mode === "denied") throw Error("401");
      if (mode === "conflict-divergent") {
        revision = 2;
        offset = 5;
        throw Error("409");
      }
      revision = 2;
      offset = 2;
      if (mode === "lost" || mode === "get-outage") throw Error("ECONNRESET");
      if (mode === "truncated") return { revision: undefined };
      return state();
    }
    assert.equal(url, endpoint + "?" + lookup);
    assert.equal(options.method, "GET");
    assert.equal(options.redirect, "error");
    if (mode === "get-outage" || mode === "denied") throw Error("unavailable");
    if (mode === "wrong-version") return { ...state(), version_id: "999" };
    return state();
  };
  return { calls, request, state };
}

test("paid cursor reconciles a committed PUT whose HTTP ACK vanished", async () => {
  const f = fixture("lost");
  const result = await savePaidProgressWithReconcile(
    f.request, endpoint, lookup, command, csrf
  );
  assert.deepEqual(result, { revision: 2, recovered: true });
  assert.deepEqual(f.calls.map(c => c.options.method), ["PUT", "GET"]);
  assert.equal(f.state().char_offset, 2);
});

test("normal 200 receipt needs no extra GET", async () => {
  const f = fixture("success");
  const result = await savePaidProgressWithReconcile(
    f.request, endpoint, lookup, command, csrf
  );
  assert.deepEqual(result, { revision: 2, recovered: false });
  assert.deepEqual(f.calls.map(c => c.options.method), ["PUT"]);
});

test("truncated or invalid 200 JSON still requires the authoritative GET", async () => {
  const f = fixture("truncated");
  const result = await savePaidProgressWithReconcile(
    f.request, endpoint, lookup, command, csrf
  );
  assert.deepEqual(result, { revision: 2, recovered: true });
  assert.deepEqual(f.calls.map(c => c.options.method), ["PUT", "GET"]);
});

test("a real competing cursor never gets overwritten after a stale 409", async () => {
  const f = fixture("conflict-divergent");
  const result = await savePaidProgressWithReconcile(
    f.request, endpoint, lookup, command, csrf
  );
  assert.equal(result, null);
  assert.equal(f.state().char_offset, 5);
  assert.deepEqual(f.calls.map(c => c.options.method), ["PUT", "GET"]);
});

test("unknown committed status after GET outage never claims success", async () => {
  const f = fixture("get-outage");
  const result = await savePaidProgressWithReconcile(
    f.request, endpoint, lookup, command, csrf
  );
  assert.equal(result, null);
  assert.deepEqual(f.calls.map(c => c.options.method), ["PUT", "GET"]);
});

test("denied purchase progress cannot recover a cursor or mint a new right", async () => {
  const f = fixture("denied");
  const result = await savePaidProgressWithReconcile(
    f.request, endpoint, lookup, command, csrf
  );
  assert.equal(result, null);
  assert.deepEqual(f.calls.map(c => c.options.method), ["PUT", "GET"]);
});
