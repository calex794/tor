// S3-07: independent WebCrypto proof of the Python/Rust AES-GCM golden.
// Node WebCrypto uses the same API as browsers; full browser/S3 E2E is separate.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createHash, webcrypto } from "node:crypto";
import { gunzipSync } from "node:zlib";

const fixture = JSON.parse(
  readFileSync(new URL("./fixtures/chapter_v1.json", import.meta.url), "utf8")
);
const bytes = (field) => Buffer.from(fixture[field], "hex");
const aesKey = async (bytesKey) =>
  webcrypto.subtle.importKey("raw", bytesKey, { name: "AES-GCM" }, false, ["decrypt"]);
const open = (key, nonce, aad, ciphertext) =>
  webcrypto.subtle.decrypt(
    { name: "AES-GCM", iv: nonce, additionalData: aad, tagLength: 128 },
    key,
    ciphertext
  );

const key = await aesKey(bytes("dek_hex"));
const ciphertext = bytes("ciphertext_hex");
const compressed = Buffer.from(
  await open(key, bytes("nonce_hex"), bytes("aad_hex"), ciphertext)
);
assert.deepEqual(compressed, bytes("gzip_hex"));
assert.equal(gunzipSync(compressed).toString("utf8"), fixture.plaintext);
assert.equal(createHash("sha256").update(ciphertext).digest("hex"), fixture.sha256_ciphertext);
assert.equal(
  createHash("sha256").update(fixture.plaintext, "utf8").digest("hex"),
  fixture.plaintext_sha256
);

// Independently reverse the KEK wrap: no plaintext DEK is required in PG.
const kek = await aesKey(bytes("kek_hex"));
const recovered = Buffer.from(
  await open(kek, bytes("wrap_nonce_hex"), bytes("wrap_aad_hex"), bytes("wrapped_dek_hex"))
);
assert.deepEqual(recovered, bytes("dek_hex"));

const tampered = Buffer.from(ciphertext);
tampered[tampered.length - 1] ^= 1;
await assert.rejects(() => open(key, bytes("nonce_hex"), bytes("aad_hex"), tampered));
const badAad = Buffer.from(bytes("aad_hex"));
badAad[0] ^= 1;
await assert.rejects(() => open(key, bytes("nonce_hex"), badAad, ciphertext));
const badNonce = Buffer.from(bytes("nonce_hex"));
badNonce[0] ^= 1;
await assert.rejects(() => open(key, badNonce, bytes("aad_hex"), ciphertext));
const wrongDek = await aesKey(Buffer.alloc(32, 0xff));
await assert.rejects(() => open(wrongDek, bytes("nonce_hex"), bytes("aad_hex"), ciphertext));
await assert.rejects(() =>
  open(wrongDek, bytes("wrap_nonce_hex"), bytes("wrap_aad_hex"), bytes("wrapped_dek_hex"))
);

console.log("S3-07 WebCrypto golden: AES-GCM/gzip/UTF-8 verified, tampering denied");
