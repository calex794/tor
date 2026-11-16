// Execute the actual H/S006 browser progress script with controlled DOM/network.
// Only the authoritative GET may resolve an ambiguous successful COMMIT; no
// unacknowledged PUT can be blindly retried or marked successful.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import test from "node:test";

const source = await readFile(
  new URL("../crates/web/assets/newboo-free-progress.js", import.meta.url),
  "utf8"
);

function response(status, data) {
  return {
    status,
    ok: status >= 200 && status < 300,
    async json() { return data; }
  };
}

async function fixture(mode) {
  const events = { get: [], put: [], notice: null };
  const handlers = {};
  const timers = new Map();
  let nextTimer = 0, desiredOffset = 2;
  let remoteRevision = 1, remoteOffset = 1;
  let failedOnce = false;
  const notice = { hidden: true, textContent: "" };
  const text = { nodeType: 3, length: 12 };
  const span = {
    dataset: { mdStart: "0" },
    firstChild: text,
    getBoundingClientRect: () => ({ top: 0 })
  };
  const article = {
    dataset: {
      freeProgressChapter: "101",
      freeProgressVersion: "201",
      freeProgressBlocks: "0:12",
      freeProgressCsrf: "a".repeat(43)
    },
    querySelectorAll: () => [span]
  };
  const state = () => ({
    chapter_id: "101", version_id: "201",
    revision: remoteRevision, block_id: 1, char_offset: remoteOffset
  });
  const fetch = async (url, options = {}) => {
    assert.equal(options.cache, "no-store");
    assert.equal(options.credentials, "same-origin");
    if (options.method === "GET") {
      assert.equal(url, "/api/v1/me/progress/free?chapter_id=101");
      events.get.push(options);
      if (mode === "unverifiable" && failedOnce) throw new Error("PG unreachable");
      return response(200, state());
    }
    assert.equal(options.method, "PUT");
    assert.equal(url, "/api/v1/me/progress/free");
    const body = JSON.parse(options.body);
    events.put.push(body);
    assert.equal(options.headers["X-CSRF-Token"], "a".repeat(43));
    assert.equal(body.chapter_id, "101");
    assert.equal(body.version_id, "201");
    if (!failedOnce) {
      failedOnce = true;
      if (mode === "ack-lost" || mode === "unverifiable" || mode === "json-lost") {
        remoteRevision = 2;
        remoteOffset = body.char_offset;
        if (mode === "json-lost") {
          return { status: 200, ok: true, async json() { throw new Error("truncated body"); } };
        }
        throw new Error("connection reset after COMMIT");
      }
      if (mode === "conflict-same" || mode === "conflict-newer") {
        remoteRevision = 2;
        remoteOffset = mode === "conflict-same" ? body.char_offset : 5;
        return response(409, {});
      }
      if (mode === "unauthorized") return response(401, {});
    }
    assert.equal(body.expected_revision, remoteRevision);
    remoteRevision += 1;
    remoteOffset = body.char_offset;
    return response(200, state());
  };
  const document = {
    querySelector: selector => selector.includes("data-free-progress-notice") ? notice : article,
    addEventListener: (name, listener) => { handlers[name] = listener; },
    createRange: () => {
      let offset = 0;
      return {
        setStart(_node, index) { offset = index; },
        collapse() {},
        getBoundingClientRect: () => ({ top: offset <= desiredOffset ? 100 : 500 })
      };
    }
  };
  const window = {
    innerHeight: 1000,
    scrollY: 50,
    location: { hash: "#already-reading" },
    addEventListener: (name, listener) => { handlers[name] = listener; },
    setTimeout: callback => {
      const id = ++nextTimer;
      timers.set(id, callback);
      return id;
    },
    clearTimeout: id => { timers.delete(id); },
    requestAnimationFrame: callback => callback(),
    scrollTo() { throw new Error("test does not trigger automatic scroll"); }
  };
  vm.runInNewContext(source, { document, window, Node: { TEXT_NODE: 3 }, fetch },
    { timeout: 1000 });
  const flush = async () => {
    for (let i = 0; i < 30; i++) await Promise.resolve();
  };
  await flush();
  assert.equal(article.dataset.freeProgressReady, "1", "initial authoritative GET");
  async function scroll(offset) {
    desiredOffset = offset;
    handlers.scroll();
    const tasks = [...timers.values()];
    timers.clear();
    for (const callback of tasks) callback();
    await flush();
  }
  return { events, notice, scroll, state, article };
}

test("lost HTTP acknowledgement reconciles committed cursor and next CAS uses new revision", async () => {
  const f = await fixture("ack-lost");
  await f.scroll(2);
  assert.equal(f.events.put.length, 1, "no blind retry");
  assert.equal(f.events.put[0].expected_revision, 1);
  assert.equal(f.events.get.length, 2, "initial GET + one reconciliation GET");
  assert.equal(f.notice.textContent, "Место чтения подтверждено после восстановления связи.");
  await f.scroll(3);
  assert.equal(f.events.put.length, 2);
  assert.equal(f.events.put[1].expected_revision, 2);
  assert.equal(f.state().revision, 3);
  assert.equal(f.state().char_offset, 3);
});

test("lost 200 JSON receipt is also reconciled from durable state", async () => {
  const f = await fixture("json-lost");
  await f.scroll(2);
  assert.equal(f.events.put.length, 1);
  assert.equal(f.events.get.length, 2);
  assert.match(f.notice.textContent, /подтверждено/);
});

test("409 with already committed matching place recovers without duplicate writes", async () => {
  const f = await fixture("conflict-same");
  await f.scroll(2);
  assert.equal(f.events.put.length, 1);
  assert.equal(f.events.get.length, 2);
  await f.scroll(3);
  assert.equal(f.events.put.length, 2);
  assert.equal(f.events.put[1].expected_revision, 2);
});

test("409 for another device's cursor blocks and never overwrites it", async () => {
  const f = await fixture("conflict-newer");
  await f.scroll(2);
  assert.equal(f.events.put.length, 1);
  assert.equal(f.state().char_offset, 5);
  assert.match(f.notice.textContent, /другой вкладке/);
  await f.scroll(3);
  assert.equal(f.events.put.length, 1, "no stale overwrite");
  assert.equal(f.state().char_offset, 5);
});

test("unverifiable COMMIT does not claim rollback or retry blindly", async () => {
  const f = await fixture("unverifiable");
  await f.scroll(2);
  assert.equal(f.events.put.length, 1);
  assert.equal(f.events.get.length, 2);
  assert.match(f.notice.textContent, /Не удалось подтвердить сохранение/);
  assert.doesNotMatch(f.notice.textContent, /не сохранено/i);
  await f.scroll(3);
  assert.equal(f.events.put.length, 1);
});

test("authentication denial is not retried or treated as a committed cursor", async () => {
  const f = await fixture("unauthorized");
  await f.scroll(2);
  assert.equal(f.events.put.length, 1);
  assert.equal(f.events.get.length, 1);
  assert.match(f.notice.textContent, /проверьте вход/);
  await f.scroll(3);
  assert.equal(f.events.put.length, 1);
});
