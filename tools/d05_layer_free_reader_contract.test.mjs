// D-05 browser contract: never use public routes, never render unverified HTML.
// This deterministic test supplements (not replaces) PG + MinIO + Chromium E2E.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createHash, webcrypto } from "node:crypto";
import vm from "node:vm";
import test from "node:test";

const script = readFileSync(
  new URL("../crates/web/assets/newboo-layer-free-reader.mjs", import.meta.url),
  "utf8"
);
const chapter = "17";
const version = "23";
const bytes = new TextEncoder().encode("Историческая V1\n\n<script>alert('unsafe')</script>");
const hash = createHash("sha256").update(bytes).digest("hex");
const signed = "https://objects.example.test/private/free-chapters/17/23.md" +
  "?X-Amz-Expires=90&X-Amz-Signature=" + "a".repeat(64);
const baseGrant = {
  schema: "tor.layer-free-s3-grant.v1",
  chapter_id: chapter,
  version_id: version,
  url: signed,
  sha256: hash,
  size_bytes: bytes.byteLength
};

function networkResponse(body, type = "application/json") {
  return {
    ok: true,
    status: 200,
    redirected: false,
    headers: { get: key => key === "content-type" ? type : null },
    json: async () => body,
    body: {
      getReader: () => {
        let read = false;
        return {
          read: async () => read
            ? { done: true }
            : (read = true, { done: false, value: body }),
          cancel: async () => {}
        };
      }
    }
  };
}

async function run({ grant = baseGrant, payload = bytes } = {}) {
  const status = { textContent: "" };
  const article = {
    children: [],
    replaceChildren(...nodes) { this.children = nodes; }
  };
  const root = {
    dataset: {
      chapterId: chapter,
      csrfToken: "A".repeat(43),
      s3Origin: "https://objects.example.test"
    },
    querySelector: key => key === "[data-layer-free-status]" ? status :
      key === "[data-layer-free-content]" ? article : null
  };
  const calls = [];
  const document = {
    querySelector: key => key === "[data-layer-free-reader]" ? root : null,
    createDocumentFragment: () => ({
      pieces: [],
      append(piece) { this.pieces.push(piece); }
    }),
    createElement: tag => {
      assert.equal(tag, "p");
      return { className: "", textContent: "" };
    }
  };
  const fetch = async (uri, options) => {
    calls.push({ uri: String(uri), options });
    if (calls.length === 1) {
      assert.equal(uri, "/api/v1/reader/layer/free/17/grant");
      assert.equal(options.method, "POST");
      assert.equal(options.headers["X-CSRF-Token"], "A".repeat(43));
      assert.equal(options.credentials, "same-origin");
      return networkResponse(grant);
    }
    assert.equal(calls.length, 2, "no alternate public grant fallback");
    assert.equal(uri, signed);
    assert.equal(options.credentials, "omit");
    assert.equal(options.redirect, "error");
    return networkResponse(payload, "text/plain");
  };
  vm.runInNewContext(script, {
    document, fetch, Uint8Array, URL, TextDecoder,
    crypto: webcrypto, console
  }, { filename: "newboo-layer-free-reader.mjs" });
  // Two fetches and WebCrypto digest complete as microtasks.
  for (let i = 0; i < 10; i++) await new Promise(resolve => setImmediate(resolve));
  return { status, article, calls };
}

test("verified FREE V1 renders only text nodes and never public fallback", async () => {
  const output = await run();
  assert.equal(output.calls.length, 2);
  assert.match(output.status.textContent, /Архивная редакция/);
  assert.equal(output.article.children.length, 1);
  assert.equal(output.article.children[0].pieces.length, 2);
  assert.equal(
    output.article.children[0].pieces[1].textContent,
    "<script>alert('unsafe')</script>"
  );
});

test("incorrect hash refuses rendered text", async () => {
  const output = await run({ grant: { ...baseGrant, sha256: "0".repeat(64) } });
  assert.match(output.status.textContent, /недоступно/);
  assert.equal(output.article.children.length, 0);
});

test("wrong archive version, origin and oversized responses never leak content", async () => {
  for (const change of [
    { version_id: "24" },
    { url: signed.replace("objects.example.test", "external.example.test") },
    { size_bytes: 200001 },
    { chapter_id: "18" }
  ]) {
    const output = await run({ grant: { ...baseGrant, ...change } });
    assert.equal(output.calls.length, 1, "must reject before external S3 GET");
    assert.equal(output.article.children.length, 0);
    assert.match(output.status.textContent, /недоступно/);
  }
});
