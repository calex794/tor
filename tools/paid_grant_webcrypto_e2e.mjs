// S034: paid grant returned by TOR, MinIO signed GET, browser-compatible WebCrypto.
// Reads only an ephemeral file produced by paid_grants_s3_pg; never print it.
import assert from "node:assert/strict";
import { createHash, webcrypto } from "node:crypto";
import { readFileSync } from "node:fs";

const path = process.env.TOR_PAID_GRANT_FIXTURE_PATH;
const temp = process.env.RUNNER_TEMP;
assert(path && temp && path.startsWith(temp + "/"), "fixture must be in runner temp");

const fixture = JSON.parse(readFileSync(path, "utf8"));
const grant = fixture.grant;
assert.equal(grant.schema, "tor.paid_delivery.v1");
assert.equal(grant.content.cipher, "AES-256-GCM");
assert.equal(grant.content.codec, "gzip");
assert.equal(grant.version_id, fixture.purchased_version_id);
assert.equal(grant.protected_mode, "pua_v1");
assert.deepEqual(grant.assets, []);

const signed = new URL(grant.content.ciphertext_url);
assert.equal(signed.protocol, "http:");
assert.equal(signed.hostname, "127.0.0.1");
assert.equal(signed.port, "9000");
assert(signed.pathname.startsWith("/tor-private-chapter-ci/private/chapter-versions/"));
assert(signed.searchParams.has("X-Amz-Signature"), "not a real S3 signed GET");
assert(new Date(grant.expires_at).getTime() > Date.now(), "grant expired too early");

const response = await fetch(signed);
assert.equal(response.status, 200, "paid ciphertext must be accessible via signed GET");
assert.equal(response.headers.get("content-type"), "application/octet-stream");
const cipherBytes = Buffer.from(await response.arrayBuffer());
assert(cipherBytes.length >= 17 && cipherBytes.length <= 16 * 1024 * 1024);
assert.equal(
  createHash("sha256").update(cipherBytes).digest("hex"),
  grant.content.object_sha256,
  "downloaded ciphertext must match committed PostgreSQL/S3 digest"
);
const decode = (s) => Buffer.from(s, "base64url");
const nonce = decode(grant.content.nonce_b64u);
const aad = decode(grant.content.aad_b64u);
const rawKey = decode(grant.content.dek_b64u);
assert.equal(nonce.length, 12);
assert.equal(rawKey.length, 32);
assert(aad.toString("utf8").startsWith("tor.encrypted.chapter.v1\n"));
const key = await webcrypto.subtle.importKey(
  "raw", rawKey, { name: "AES-GCM" }, false, ["decrypt"]
);
const open = (bytes, extra = {}) =>
  webcrypto.subtle.decrypt(
    {
      name: "AES-GCM",
      iv: extra.nonce ?? nonce,
      additionalData: extra.aad ?? aad,
      tagLength: 128,
    },
    key,
    bytes
  );
const zipped = await open(cipherBytes);
const plainBytes = await new Response(
  new Blob([zipped]).stream().pipeThrough(new DecompressionStream("gzip"))
).arrayBuffer();
const decoded = new TextDecoder("utf-8", { fatal: true }).decode(plainBytes);
assert.equal(decoded, fixture.expected_markdown);

// Cryptography must fail closed: no plaintext or fallback on tampered bytes.
const tampered = Buffer.from(cipherBytes);
tampered[tampered.length - 1] ^= 1;
await assert.rejects(() => open(tampered));
const wrongAad = Buffer.from(aad);
wrongAad[0] ^= 1;
await assert.rejects(() => open(cipherBytes, { aad: wrongAad }));
const wrongNonce = Buffer.from(nonce);
wrongNonce[0] ^= 1;
await assert.rejects(() => open(cipherBytes, { nonce: wrongNonce }));

// The exact same private ciphertext cannot be fetched anonymously or with
// a tampered signature. Neither assertion prints bearer URL/DEK.
const anonymous = new URL(signed);
anonymous.search = "";
assert.equal((await fetch(anonymous)).status, 403);
const badSignature = new URL(signed);
badSignature.searchParams.set("X-Amz-Signature", "0".repeat(64));
assert.equal((await fetch(badSignature)).status, 403);

console.log("S034 real PG + private MinIO + WebCrypto: paid V1 PASS; unauthorized and tampered DENIED");
