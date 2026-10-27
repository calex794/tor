// I-11/S061: CI-only authenticated encryption for a DISPOSABLE PostgreSQL dump.
// Not a production backup tool: intentionally bounded to 16 MiB fixtures.
'use strict';

const fs = require('node:fs');
const crypto = require('node:crypto');

const HEADER = Buffer.from('TOR-CI-DR-1\n', 'ascii');
const AAD = Buffer.from('tor.i11.pg18.backup.ci.v1', 'ascii');
const MAX_INPUT = 16 * 1024 * 1024;

function fail() {
  process.stderr.write('CI_BACKUP_CRYPTO_REJECTED\n');
  process.exit(1);
}
if (process.env.GITHUB_ACTIONS !== 'true') fail();
const [action, ...args] = process.argv.slice(2);

function exclusiveWrite(path, bytes) {
  fs.writeFileSync(path, bytes, {mode: 0o600, flag: 'wx'});
}

function loadKey(path) {
  const key = fs.readFileSync(path);
  if (key.length !== 32) fail();
  return key;
}

try {
  if (action === 'new-key' && args.length === 1) {
    exclusiveWrite(args[0], crypto.randomBytes(32));
  } else if (action === 'seal' && args.length === 3) {
    const [keyPath, source, target] = args;
    const key = loadKey(keyPath);
    const size = fs.statSync(source).size;
    if (!Number.isSafeInteger(size) || size <= 0 || size > MAX_INPUT) fail();
    const plaintext = fs.readFileSync(source);
    const nonce = crypto.randomBytes(12);
    const cipher = crypto.createCipheriv('aes-256-gcm', key, nonce);
    cipher.setAAD(AAD);
    const ciphertext = Buffer.concat([cipher.update(plaintext), cipher.final()]);
    const tag = cipher.getAuthTag();
    exclusiveWrite(target, Buffer.concat([HEADER, nonce, ciphertext, tag]));
    plaintext.fill(0);
    key.fill(0);
  } else if (action === 'open' && args.length === 3) {
    const [keyPath, source, target] = args;
    const key = loadKey(keyPath);
    const blob = fs.readFileSync(source);
    const start = HEADER.length + 12;
    if (!blob.subarray(0, HEADER.length).equals(HEADER) ||
        blob.length < start + 17 ||
        blob.length > MAX_INPUT + start + 16) fail();
    const nonce = blob.subarray(HEADER.length, start);
    const tag = blob.subarray(blob.length - 16);
    const ciphertext = blob.subarray(start, blob.length - 16);
    const decipher = crypto.createDecipheriv('aes-256-gcm', key, nonce);
    decipher.setAAD(AAD);
    decipher.setAuthTag(tag);
    const plaintext = Buffer.concat([decipher.update(ciphertext), decipher.final()]);
    if (plaintext.length <= 0 || plaintext.length > MAX_INPUT) fail();
    exclusiveWrite(target, plaintext);
    plaintext.fill(0);
    key.fill(0);
  } else if (action === 'damage' && args.length === 2) {
    const blob = fs.readFileSync(args[0]);
    if (blob.length <= HEADER.length + 12 + 16 ||
        !blob.subarray(0, HEADER.length).equals(HEADER)) fail();
    blob[HEADER.length + 13] ^= 0x40;
    exclusiveWrite(args[1], blob);
  } else {
    fail();
  }
} catch (_) {
  // No exception details, secret paths, plaintext or cipher data in CI logs.
  fail();
}
