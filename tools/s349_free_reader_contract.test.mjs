// Unit-level execution of the actual browser script with controlled DOM/network.
// No external service, browser installation or real S3 credential is needed.
import assert from "node:assert/strict";
import { createHash, webcrypto } from "node:crypto";
import { readFile } from "node:fs/promises";
import { setImmediate as nextTurn } from "node:timers/promises";
import vm from "node:vm";
import test from "node:test";

const source = await readFile(new URL("../crates/web/assets/newboo-free-s3-reader.js", import.meta.url), "utf8");
const objectURL = "https://s3.example.invalid/bucket/private/free-chapters/101/201.md" +
  "?X-Amz-Expires=90&X-Amz-Signature=" + "a".repeat(64);
const sha = bytes => createHash("sha256").update(bytes).digest("hex");

async function runCase(options = {}) {
  const bytes = Buffer.from(options.bytes || options.text || "Chapter A\n\nSecond paragraph", "utf8");
  const events = { requests: [], scripts: [], created: [], cancellations: 0 };
  let completed;
  const done = new Promise(resolve => { completed = resolve; });
  class Element {
    constructor(tag) {
      this.tag = tag;
      this.children = [];
      this.dataset = {};
      this.attrs = {};
      this.value = "";
    }
    set innerHTML(_) { throw Error("raw HTML must never be injected"); }
    set textContent(text) { this.children = []; this.value = String(text); }
    get textContent() { return this.value + this.children.map(child => child.textContent).join(""); }
    setAttribute(name, value) { this.attrs[name] = String(value); }
    append(...nodes) {
      for (const node of nodes) {
        if (node.tag === "fragment") this.children.push(...node.children);
        else this.children.push(node);
      }
      if (this.tag === "head") events.scripts.push(...nodes);
    }
    replaceChildren(...nodes) {
      this.value = "";
      this.children = [];
      this.append(...nodes);
      if (this === article) completed();
    }
  }
  const article = new Element("article");
  article.dataset = {
    freeS3Chapter: "101", freeS3Version: "201", freeS3Csrf: options.csrf || "",
    ...options.dataset,
  };
  const document = {
    head: new Element("head"),
    querySelector: () => options.absent ? null : article,
    createElement: tag => { events.created.push(tag); return new Element(tag); },
    createDocumentFragment: () => new Element("fragment"),
  };
  const grant = {
    schema: "tor.free-s3-grant.v1", chapter_id: "101", version_id: "201",
    url: objectURL, sha256: sha(bytes), size_bytes: bytes.length,
    ...options.grant,
  };
  const actual = options.received || bytes;
  let storageResponse;
  const fetch = async (url, config) => {
    events.requests.push({ url: String(url), config });
    if (String(url).startsWith("/api/")) {
      return new Response(options.grantText ?? JSON.stringify(grant), {
        status: options.grantStatus || 200,
        headers: { "content-type": "application/json" },
      });
    }
    if (options.neverResponds) {
      return await new Promise((_, reject) => {
        config.signal.addEventListener("abort", () => reject(Error("aborted")), { once: true });
      });
    }
    let index = 0;
    const chunks = options.chunks || [actual];
    const stream = new ReadableStream({
      pull(controller) {
        if (index < chunks.length) controller.enqueue(new Uint8Array(chunks[index++]));
        else controller.close();
      },
      cancel() { events.cancellations++; },
    }, { highWaterMark: 0 });
    storageResponse = new Response(stream, { status: options.storageStatus || 200 });
    if (options.redirected) Object.defineProperty(storageResponse, "redirected", { value: true });
    return storageResponse;
  };
  const context = {
    document, fetch, URL, TextDecoder, Uint8Array, AbortController,
    crypto: webcrypto, clearTimeout,
    setTimeout: (callback, delay) => setTimeout(callback, options.neverResponds ? 5 : delay),
    localStorage: { setItem() { throw Error("must not persist chapter text"); } },
    sessionStorage: { setItem() { throw Error("must not persist chapter text"); } },
  };
  vm.runInNewContext(source, context, { timeout: 1000 });
  if (options.absent || options.invalidIdentity) {
    await nextTurn();
  } else {
    let timeout;
    try {
      await Promise.race([
        done,
        new Promise((_, reject) => { timeout = setTimeout(() => reject(Error("script did not finish")), 1500); }),
      ]);
    } finally {
      clearTimeout(timeout);
    }
    await nextTurn();
  }
  return {
    article, ...events,
    failed: article.children.some(node => node.attrs.role === "alert"),
    unlocked: !storageResponse?.body?.locked,
  };
}

function assertDenied(result, calls) {
  assert.equal(result.failed, true);
  assert.equal(result.requests.length, calls);
  assert.equal(result.scripts.length, 0);
  assert.equal(result.article.dataset.freeProgressVersion, undefined);
  assert.ok(!result.article.textContent.includes("X-Amz-"));
}

test("fetches only metadata from TOR and verified bytes directly from S3", async () => {
  const result = await runCase();
  assert.equal(result.failed, false);
  assert.equal(result.article.textContent, "Chapter ASecond paragraph");
  assert.equal(result.requests.length, 2);
  assert.equal(result.requests[0].url, "/api/v1/chapters/101/free-grant");
  assert.equal(result.requests[0].config.credentials, "same-origin");
  assert.equal(result.requests[1].url, objectURL);
  for (const request of result.requests) {
    assert.equal(request.config.cache, "no-store");
    assert.equal(request.config.redirect, "error");
    assert.equal(request.config.referrerPolicy, "no-referrer");
    assert.ok(request.config.signal);
    assert.equal(request.config.body, undefined);
  }
  assert.equal(result.requests[1].config.credentials, "omit");
  assert.equal(result.requests[1].config.mode, "cors");
  assert.equal(result.scripts.length, 0, "guest must not load progress code");
  assert.equal(result.unlocked, true);
});

test("restores exact UTF-16 anchors only after verification", async () => {
  const result = await runCase({ text: "A\ud83d\ude00B\n\n\u4e2d\u6587", csrf: "x".repeat(43) });
  assert.equal(result.failed, false);
  assert.equal(result.article.dataset.freeProgressBlocks, "0:4,6:2");
  assert.equal(result.article.children[1].children[0].dataset.mdStart, "6");
  assert.equal(result.scripts.length, 1);
  assert.equal(result.scripts[0].src, "/assets/newboo-free-progress-s3.js");
});

test("more than 200 paragraphs keep bounded progress anchors", async () => {
  const result = await runCase({ text: Array.from({ length: 205 }, (_, i) => `p${i}`).join("\n\n"), csrf: "x".repeat(43) });
  assert.equal(result.failed, false);
  const blocks = result.article.dataset.freeProgressBlocks.split(",");
  assert.equal(blocks.length, 103);
  assert.ok(blocks.length <= 200);
});

for (const status of [403, 404, 503]) {
  test(`grant ${status} never makes an S3 request`, async () => {
    assertDenied(await runCase({ grantStatus: status }), 1);
  });
  test(`S3 ${status} never renders a chapter`, async () => {
    assertDenied(await runCase({ storageStatus: status }), 2);
  });
}
for (const [name, grant] of [
  ["other edition", { version_id: "202" }],
  ["other chapter", { chapter_id: "102" }],
  ["invalid schema", { schema: "anything" }],
  ["negative size", { size_bytes: -1 }],
  ["unbounded size", { size_bytes: 200001 }],
  ["fractional size", { size_bytes: 1.5 }],
  ["malformed hash", { sha256: "0" }],
  ["wrong object key", { url: objectURL.replace("101/201", "101/202") }],
  ["URL credentials", { url: objectURL.replace("https://", "https://alice:secret@") }],
  ["URL fragment", { url: objectURL + "#fragment" }],
  ["unsigned URL", { url: objectURL.split("?")[0] }],
  ["long expiry", { url: objectURL.replace("Expires=90", "Expires=9999") }],
  ["insecure remote HTTP", { url: objectURL.replace("https://", "http://") }],
]) {
  test(`${name} is refused before accessing S3`, async () => {
    assertDenied(await runCase({ grant }), 1);
  });
}

test("corrupt bytes with the correct size never render", async () => {
  assertDenied(await runCase({ text: "trusted", received: Buffer.from("CORRUPT") }), 2);
});
test("truncated bytes never render", async () => {
  assertDenied(await runCase({ text: "trusted", received: Buffer.from("x") }), 2);
});
test("stream overflow cancels the download and releases the reader", async () => {
  const result = await runCase({ text: "x", chunks: [Buffer.from("TOO-LARGE"), Buffer.from("more")] });
  assertDenied(result, 2);
  assert.equal(result.cancellations, 1);
  assert.equal(result.unlocked, true);
});
test("oversized grant is not a body proxy", async () => {
  assertDenied(await runCase({ grant: { unexpected_text: "x".repeat(20000) } }), 1);
});
test("JSON null and malformed JSON fail closed", async () => {
  assertDenied(await runCase({ grantText: "null" }), 1);
  assertDenied(await runCase({ grantText: "not-json" }), 1);
});
test("invalid UTF-8, NUL and oversized text fail after correct hashes", async () => {
  assertDenied(await runCase({ bytes: Buffer.from([0xff, 0xfe]) }), 2);
  assertDenied(await runCase({ text: "a\0b" }), 2);
  assertDenied(await runCase({ text: "a".repeat(50001) }), 2);
});
test("50000 four-byte characters fit the exact byte and character limits", async () => {
  const result = await runCase({ text: "\ud83d\ude00".repeat(50000) });
  assert.equal(result.failed, false);
  assert.equal(result.article.textContent.length, 100000);
});
test("HTML-looking content remains text and cannot create an executable element", async () => {
  const text = '<script>throw new Error("unsafe")</script><img src=x onerror=alert(1)>';
  const result = await runCase({ text });
  assert.equal(result.failed, false);
  assert.equal(result.article.textContent, text);
  assert.ok(!result.created.includes("script"));
  assert.ok(!result.created.includes("img"));
});
test("redirected S3 responses are not accepted", async () => {
  assertDenied(await runCase({ redirected: true }), 2);
});
test("a stalled provider is aborted instead of keeping the loading view forever", async () => {
  assertDenied(await runCase({ neverResponds: true }), 2);
});
test("missing reader or invalid identity never touches the network", async () => {
  assert.equal((await runCase({ absent: true })).requests.length, 0);
  assert.equal((await runCase({ invalidIdentity: true, dataset: { freeS3Chapter: "../private" } })).requests.length, 0);
});
