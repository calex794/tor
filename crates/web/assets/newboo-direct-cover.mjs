// Direct S3 cover loader. Disabled unless the page explicitly opts in via data-direct-cover.
// Signed URLs are ephemeral and never written to the DOM or browser storage.
const MAX_BYTES = 8 * 1024 * 1024;
const MAX_ACTIVE = 4;
const queue = Array.from(document.querySelectorAll('img[data-direct-cover]'));
let active = 0;
const urls = new Set();

function start() {
  while (active < MAX_ACTIVE && queue.length) {
    const image = queue.shift();
    if (!image.isConnected) continue;
    active++;
    load(image).finally(() => {
      active--;
      start();
    });
  }
}

async function load(image) {
  const path = image.dataset.directCover;
  if (!/^\/api\/v1\/covers\/[1-9][0-9]*\/(card|main)\.webp\/direct-grant$/.test(path)) return;
  let objectUrl;
  try {
    const grantResponse = await fetch(path, { credentials: 'same-origin', cache: 'no-store' });
    if (!grantResponse.ok) return;
    const grant = await grantResponse.json();
    const bytesExpected = grant.size_bytes;
    if (!Number.isInteger(bytesExpected) || bytesExpected < 12 || bytesExpected > MAX_BYTES ||
        !/^[a-f0-9]{64}$/.test(grant.sha256) || grant.content_type !== 'image/webp') return;
    const signed = new URL(grant.url);
    // Credentials are deliberately not sent to the object store.
    // A browser CSP connect-src allowlist is required before opting in.
    if (signed.protocol !== 'https:' &&
        !(signed.protocol === 'http:' && ['localhost', '127.0.0.1'].includes(signed.hostname))) return;
    const response = await fetch(signed, { credentials: 'omit', cache: 'no-store', redirect: 'error' });
    if (!response.ok || response.type === 'opaque') return;
    const length = response.headers.get('content-length');
    if (length !== null && Number(length) !== bytesExpected) return;
    const reader = response.body?.getReader();
    if (!reader) return;
    const parts = [];
    let lengthRead = 0;
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      lengthRead += value.byteLength;
      if (lengthRead > bytesExpected) {
        await reader.cancel();
        return;
      }
      parts.push(value);
    }
    if (lengthRead !== bytesExpected) return;
    const data = new Uint8Array(lengthRead);
    let at = 0;
    for (const part of parts) {
      data.set(part, at);
      at += part.byteLength;
    }
    if (String.fromCharCode(...data.subarray(0, 4)) !== 'RIFF' ||
        String.fromCharCode(...data.subarray(8, 12)) !== 'WEBP') return;
    const actual = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', data)),
      value => value.toString(16).padStart(2, '0')).join('');
    if (actual !== grant.sha256) return;
    objectUrl = URL.createObjectURL(new Blob([data], { type: 'image/webp' }));
    if (!image.isConnected) return;
    const previous = image.dataset.directBlob;
    image.src = objectUrl;
    image.dataset.directBlob = objectUrl;
    urls.add(objectUrl);
    objectUrl = undefined;
    if (previous) {
      URL.revokeObjectURL(previous);
      urls.delete(previous);
    }
  } catch {
    // Do not fallback to proxying large images through the home server.
  } finally {
    if (objectUrl) URL.revokeObjectURL(objectUrl);
  }
}

window.addEventListener('pagehide', () => {
  for (const url of urls) URL.revokeObjectURL(url);
  urls.clear();
});
start();
