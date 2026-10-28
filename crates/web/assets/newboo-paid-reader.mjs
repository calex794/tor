// H/S034: memory-only paid-reader WebCrypto, no plaintext or keys in storage.
// Browser page must be served by TOR with CSRF and an operator-pinned S3 origin.
const MAX_CIPHERTEXT_BYTES = 16 * 1024 * 1024;
const MAX_PLAINTEXT_BYTES = 1024 * 1024;
const MAX_SIGNED_SECONDS = 120;
const MAX_MEDIA_BYTES = 8 * 1024 * 1024;
const MAX_MEDIA_CIPHERTEXT_BYTES = MAX_MEDIA_BYTES + 16;
const MAX_MEDIA_DIMENSION = 8192;
const MAX_MEDIA_PIXELS = 32 * 1024 * 1024;

const fail = () => new Error("Не удалось безопасно открыть оплаченную главу.");

function requireValue(ok) {
  if (!ok) throw fail();
}

function decodeBase64Url(value) {
  requireValue(typeof value === "string" && /^[A-Za-z0-9_-]+$/.test(value));
  const raw = atob(value.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(raw, (ch) => ch.charCodeAt(0));
}

export function readerBlocks(source) {
  // Mirror C's reader_blocks exactly: up to 200 deterministic UTF-16 anchors.
  const paragraphs = source.replace(/\r\n?/g, "\n").split("\n\n");
  const groupSize = Math.ceil(paragraphs.length / 200);
  const result = [];
  for (let i = 0; i < paragraphs.length; i += groupSize) {
    result.push(paragraphs.slice(i, i + groupSize).join("\n\n"));
  }
  return result;
}

function verifiedS3URL(raw, configuredOrigin, assetId = null) {
  requireValue(typeof raw === "string" && raw.length < 4096);
  const url = new URL(raw);
  const origin = new URL(configuredOrigin);
  requireValue(
    url.origin === origin.origin &&
      (url.protocol === "https:" ||
        (url.protocol === "http:" &&
          (url.hostname === "127.0.0.1" || url.hostname === "localhost"))) &&
      origin.pathname === "/" && !origin.username && !origin.password &&
      !url.username && !url.password && !url.hash &&
      (assetId === null
        ? /\/private\/chapter-versions\/[0-9a-f-]{36}\.enc$/.test(url.pathname)
        : url.pathname.endsWith("/private/chapter-assets/" + assetId + ".enc")) &&
      url.searchParams.has("X-Amz-Signature")
  );
  return url.toString();
}

async function decompressBounded(zipped) {
  requireValue(typeof DecompressionStream === "function");
  const stream = new Blob([zipped]).stream().pipeThrough(new DecompressionStream("gzip"));
  const reader = stream.getReader();
  let size = 0;
  const chunks = [];
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      requireValue(size <= MAX_PLAINTEXT_BYTES);
      chunks.push(value);
    }
  } catch {
    await reader.cancel().catch(() => {});
    throw fail();
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
}

// Validate the S3 ciphertext stream before buffering it (Content-Length is
// advisory and CORS may hide it). Shared by chapter and image grants.
async function fetchVerifiedCiphertext(url, content, maxCiphertextBytes, fetchCiphertext) {
  const reply = await fetchCiphertext(url, {
    method: "GET",
    mode: "cors",
    credentials: "omit",
    redirect: "error",
    cache: "no-store",
    referrerPolicy: "no-referrer",
  });
  requireValue(reply.ok && reply.status === 200);
  // Do not buffer an untrusted S3 response before validating its real size.
  // Content-Length can be absent, wrong or inaccessible through CORS.
  const declared = reply.headers.get("content-length");
  if (declared !== null) {
    requireValue(/^(0|[1-9][0-9]*)$/.test(declared));
    requireValue(Number.isSafeInteger(Number(declared)) &&
      Number(declared) <= maxCiphertextBytes);
  }
  requireValue(reply.body && typeof reply.body.getReader === "function");
  const reader = reply.body.getReader();
  const chunks = [];
  let received = 0;
  let complete = false;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) { complete = true; break; }
      requireValue(value instanceof Uint8Array);
      received += value.byteLength;
      requireValue(received <= maxCiphertextBytes);
      chunks.push(value);
    }
  } finally {
    if (!complete) await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
  requireValue(received >= 17 &&
    (declared === null || Number(declared) === received));
  const cipher = new Uint8Array(received);
  let copied = 0;
  for (const chunk of chunks) {
    cipher.set(chunk, copied);
    copied += chunk.byteLength;
  }
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", cipher));
  const checksum = Array.from(digest, (b) => b.toString(16).padStart(2, "0")).join("");
  requireValue(checksum === content.object_sha256);
  return cipher;
}

export async function decryptPaidGrant(
  grant, chapterId, configuredS3Origin, fetchCiphertext = fetch, now = Date.now()
) {
  const content = grant?.content;
  requireValue(
    grant?.schema === "tor.paid_delivery.v1" &&
      grant?.chapter_id === chapterId &&
      /^\d+$/.test(grant?.version_id || "") &&
      grant?.protected_mode === "pua_v1" &&
      Array.isArray(grant?.assets) && grant.assets.length === 0 &&
      content?.cipher === "AES-256-GCM" && content.codec === "gzip" &&
      /^[0-9a-f]{64}$/.test(content.object_sha256 || "")
  );
  const expiry = Date.parse(grant.expires_at);
  requireValue(
    Number.isFinite(expiry) && expiry > now &&
      expiry - now <= (MAX_SIGNED_SECONDS + 10) * 1000
  );
  const url = verifiedS3URL(content.ciphertext_url, configuredS3Origin);
  const nonce = decodeBase64Url(content.nonce_b64u);
  const aad = decodeBase64Url(content.aad_b64u);
  const dek = decodeBase64Url(content.dek_b64u);
  requireValue(nonce.length === 12 && dek.length === 32 && aad.length >= 32);
  const aadText = new TextDecoder("utf-8", { fatal: true }).decode(aad);
  requireValue(aadText.startsWith("tor.encrypted.chapter.v1\n"));
  // The authenticated metadata is version- and object-specific.
  const parts = aadText.split("\n");
  requireValue(parts.length === 6 && parts[1] === chapterId &&
    parts[2] === grant.version_id &&
    /^[0-9a-f-]{36}$/.test(parts[3]) &&
    /^[0-9a-f]{64}$/.test(parts[4]) && parts[5] === "");

  const cipher = await fetchVerifiedCiphertext(
    url, content, MAX_CIPHERTEXT_BYTES, fetchCiphertext
  );

  const key = await crypto.subtle.importKey("raw", dek, { name: "AES-GCM" }, false, ["decrypt"]);
  let zipped;
  try {
    zipped = await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: nonce, additionalData: aad, tagLength: 128 },
      key, cipher
    );
  } catch {
    throw fail();
  } finally {
    dek.fill(0);
  }
  const text = await decompressBounded(zipped);
  requireValue(text.length <= 200000);
  // Bind the displayed plaintext to G's authenticated immutable AAD.
  const normalized = text.replace(/\r\n?/g, "\n");
  const plain = new Uint8Array(await crypto.subtle.digest(
    "SHA-256", new TextEncoder().encode(normalized)
  ));
  const plainHex = Array.from(plain, (b) => b.toString(16).padStart(2, "0")).join("");
  requireValue(plainHex === parts[4]);
  return { versionId: grant.version_id, text, plaintextSha256: plainHex };
}

// Inspect dimensions before asking the image decoder to allocate a large
// bitmap. Never trust a Content-Type header or an author-supplied suffix.
function boundedImageInfo(bytes) {
  const u16 = (i) => (bytes[i] << 8) | bytes[i + 1];
  const u32 = (i) => ((bytes[i] * 16777216) +
    (bytes[i + 1] << 16) + (bytes[i + 2] << 8) + bytes[i + 3]);
  let mime, width, height;
  if (bytes.length >= 33 &&
      bytes.subarray(0, 8).every((n, i) =>
        n === [137, 80, 78, 71, 13, 10, 26, 10][i]) &&
      u32(8) === 13 && String.fromCharCode(...bytes.subarray(12, 16)) === "IHDR") {
    mime = "image/png";
    width = u32(16);
    height = u32(20);
  } else if (bytes.length >= 16 && bytes[0] === 255 &&
      bytes[1] === 216 && bytes.at(-2) === 255 && bytes.at(-1) === 217) {
    let pos = 2;
    while (pos + 9 < bytes.length && pos < 262144) {
      if (bytes[pos++] !== 255) break;
      while (bytes[pos] === 255) pos++;
      const marker = bytes[pos++];
      if (marker === 0xda || marker === 0xd9) break;
      if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) continue;
      if (pos + 2 > bytes.length) break;
      const length = u16(pos);
      if (length < 2 || pos + length > bytes.length) break;
      if ([0xc0, 0xc1, 0xc2, 0xc3, 0xc5, 0xc6, 0xc7,
           0xc9, 0xca, 0xcb, 0xcd, 0xce, 0xcf].includes(marker)) {
        requireValue(length >= 7);
        mime = "image/jpeg";
        height = u16(pos + 3);
        width = u16(pos + 5);
        break;
      }
      pos += length;
    }
  } else if (bytes.length >= 30 &&
      String.fromCharCode(...bytes.subarray(0, 4)) === "RIFF" &&
      String.fromCharCode(...bytes.subarray(8, 12)) === "WEBP" &&
      (bytes[4] + bytes[5] * 256 + bytes[6] * 65536 +
       bytes[7] * 16777216) + 8 === bytes.length) {
    const kind = String.fromCharCode(...bytes.subarray(12, 16));
    mime = "image/webp";
    if (kind === "VP8X") {
      width = 1 + bytes[24] + (bytes[25] << 8) + (bytes[26] << 16);
      height = 1 + bytes[27] + (bytes[28] << 8) + (bytes[29] << 16);
    } else if (kind === "VP8L" && bytes[20] === 47) {
      width = 1 + bytes[21] + ((bytes[22] & 63) << 8);
      height = 1 + (bytes[22] >> 6) + (bytes[23] << 2) +
        ((bytes[24] & 15) << 10);
    } else if (kind === "VP8 " &&
        bytes[23] === 157 && bytes[24] === 1 && bytes[25] === 42) {
      width = (bytes[26] | (bytes[27] << 8)) & 16383;
      height = (bytes[28] | (bytes[29] << 8)) & 16383;
    }
  }
  requireValue(mime && Number.isInteger(width) && Number.isInteger(height) &&
    width > 0 && height > 0 &&
    width <= MAX_MEDIA_DIMENSION && height <= MAX_MEDIA_DIMENSION &&
    width * height <= MAX_MEDIA_PIXELS);
  return { mime, width, height };
}

// A scoped D -> B -> G grant is still untrusted until its ciphertext hash,
// exact edition-specific AAD, plaintext hash, GCM tag and file signature match.
// Return only decoded in-memory Blob bytes, never a public S3 object URL.
export async function decryptPaidAsset(
  grant, chapterId, versionId, objectId, configuredS3Origin,
  fetchCiphertext = fetch, now = Date.now()
) {
  requireValue(
    grant?.schema === "tor.paid_asset_delivery.v1" &&
    grant.chapter_id === chapterId && grant.version_id === versionId &&
    grant.object_id === objectId &&
    /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(objectId) &&
    grant.content?.cipher === "AES-256-GCM" &&
    grant.content.codec === "none" &&
    /^[0-9a-f]{64}$/.test(grant.content.object_sha256 || "")
  );
  const expires = Date.parse(grant.expires_at);
  requireValue(Number.isFinite(expires) && expires > now &&
    expires - now <= (MAX_SIGNED_SECONDS + 10) * 1000);
  const url = verifiedS3URL(
    grant.content.ciphertext_url, configuredS3Origin, objectId
  );
  const nonce = decodeBase64Url(grant.content.nonce_b64u);
  const aad = decodeBase64Url(grant.content.aad_b64u);
  const dek = decodeBase64Url(grant.content.dek_b64u);
  requireValue(nonce.length === 12 && dek.length === 32 && aad.length < 256);
  const metadata = new TextDecoder("utf-8", { fatal: true }).decode(aad).split("\n");
  requireValue(metadata.length === 6 &&
    metadata[0] === "tor.encrypted.asset.v1" &&
    metadata[1] === chapterId && metadata[2] === versionId &&
    metadata[3] === objectId && /^[0-9a-f]{64}$/.test(metadata[4]) &&
    metadata[5] === "");
  const ciphertext = await fetchVerifiedCiphertext(
    url, grant.content, MAX_MEDIA_CIPHERTEXT_BYTES, fetchCiphertext
  );
  let clear;
  try {
    const key = await crypto.subtle.importKey(
      "raw", dek, { name: "AES-GCM" }, false, ["decrypt"]
    );
    clear = new Uint8Array(await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: nonce, additionalData: aad, tagLength: 128 },
      key, ciphertext
    ));
  } catch {
    throw fail();
  } finally {
    dek.fill(0);
  }
  try {
    requireValue(clear.length >= 16 && clear.length <= MAX_MEDIA_BYTES);
    const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", clear));
    const hex = Array.from(hash, b => b.toString(16).padStart(2, "0")).join("");
    requireValue(hex === metadata[4]);
    const { mime, width, height } = boundedImageInfo(clear);
    const blob = new Blob([clear], { type: mime });
    const bitmap = await createImageBitmap(blob);
    try {
      requireValue(bitmap.width === width && bitmap.height === height);
    } finally {
      bitmap.close();
    }
    return blob;
  } finally {
    clear.fill(0);
  }
}

export function renderPaidBlocks(container, text, onSave, onImage = null) {
  const blocks = readerBlocks(text);
  container.replaceChildren();
  blocks.forEach((block, index) => {
    const section = document.createElement("section");
    section.className = "reader-piece";
    section.id = "p-" + (index + 1);
    // Paid Markdown remains inert text. Only an isolated, exact
    // ![caption](asset:uuid) marker is upgraded to a private image figure.
    // No external URL, HTML, data: or arbitrary Markdown is interpreted.
    const imagePattern =
      /^!\\[([^\\]\\r\\n]{0,160})\\]\\(asset:([0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12})\\)$/gm;
    let cursor = 0;
    let media = 0;
    const mediaFrames = [];
    if (onImage) {
      for (const match of block.matchAll(imagePattern)) {
        const before = block.slice(cursor, match.index);
        if (before.trim()) {
          const paragraph = document.createElement("p");
          paragraph.className = "reader-piece__text reader-markdown";
          paragraph.textContent = before.trim();
          section.append(paragraph);
        }
        const figure = document.createElement("figure");
        figure.className = "reader-content__image-block reader-paid__image-block";
        const frame = document.createElement("div");
        frame.className = "reader-content__image-frame";
        frame.textContent = "Иллюстрация загружается…";
        frame.setAttribute("role", "status");
        figure.append(frame);
        if (match[1]) {
          const caption = document.createElement("figcaption");
          caption.className = "reader-content__image-caption";
          caption.textContent = match[1];
          figure.append(caption);
        }
        section.append(figure);
        mediaFrames.push([frame, match[2], match[1]]);
        media++;
        cursor = match.index + match[0].length;
      }
    }
    if (media === 0 || block.slice(cursor).trim()) {
      const paragraph = document.createElement("p");
      paragraph.className = "reader-piece__text reader-markdown";
      paragraph.textContent = media ? block.slice(cursor).trim() : block;
      section.append(paragraph);
    }
    const button = document.createElement("button");
    button.type = "button";
    button.className = "reader-save-button";
    button.textContent = "Сохранить место здесь";
    button.setAttribute("aria-label", "Сохранить после фрагмента " + (index + 1));
    button.addEventListener("click", () => onSave(index + 1, block.length));
    section.append(button);
    container.append(section);
    for (const [frame, id, alt] of mediaFrames) onImage(frame, id, alt);
  });
  return blocks;
}

// A small lazy queue avoids issuing dozens of purchase+S3 grants for images
// outside the viewport. No signed URL, key, image or Blob is stored beyond
// this page. Revoke cached Blob URLs on navigation and bound resident images.
export function privateImageLoader(root, chapter, version, purchase, csrf, s3Origin) {
  let closed = false;
  let running = 0;
  const work = [];
  const live = new Map();
  const queued = new Set();
  const MAX_VISIBLE_URLS = 12;
  let observer = null;

  const placeholder = (frame, message) => {
    frame.classList.remove("is-loaded");
    frame.classList.add("is-error");
    frame.replaceChildren(document.createTextNode(message));
    frame.setAttribute("role", "status");
  };
  const dispose = () => {
    closed = true;
    observer?.disconnect();
    for (const url of live.values()) URL.revokeObjectURL(url);
    live.clear();
    work.length = 0;
    queued.clear();
  };
  window.addEventListener("pagehide", dispose, { once: true });
  const load = async ({ frame, id, alt }) => {
    if (closed || !frame.isConnected) return;
    try {
      const grant = await api(
        "/api/v1/reader/chapters/" + chapter + "/assets/" + id + "/delivery-grants",
        {
          method: "POST",
          headers: { "Content-Type": "application/json", "X-CSRF-Token": csrf },
          body: JSON.stringify({ purchase_id: purchase }),
        }
      );
      const blob = await decryptPaidAsset(grant, chapter, version, id, s3Origin);
      if (closed || !frame.isConnected) return;
      const url = URL.createObjectURL(blob);
      // Never retain unbounded image Blobs while reading a long chapter.
      while (live.size >= MAX_VISIBLE_URLS) {
        const [previous, previousURL] = live.entries().next().value;
        live.delete(previous);
        URL.revokeObjectURL(previousURL);
        placeholder(previous, "Прокрутите к изображению для повторной загрузки.");
        if (!closed && previous.isConnected) observer?.observe(previous);
      }
      live.set(frame, url);
      const image = document.createElement("img");
      image.className = "reader-content__image";
      image.alt = alt;
      image.loading = "lazy";
      image.decoding = "async";
      image.referrerPolicy = "no-referrer";
      image.addEventListener("load", () => {
        frame.classList.add("is-loaded");
        frame.classList.remove("is-error");
        frame.removeAttribute("role");
      }, { once: true });
      image.addEventListener("error", () => {
        URL.revokeObjectURL(url);
        live.delete(frame);
        placeholder(frame, "Не удалось отобразить иллюстрацию.");
      }, { once: true });
      image.src = url;
      frame.replaceChildren(image);
    } catch {
      if (!closed && frame.isConnected) {
        placeholder(frame, "Иллюстрация недоступна. Проверьте покупку и обновите страницу.");
      }
    }
  };
  const drain = () => {
    while (!closed && running < 2 && work.length) {
      const task = work.shift();
      if (!task.frame.isConnected) { queued.delete(task.frame); continue; }
      running++;
      load(task).finally(() => {
        running--;
        queued.delete(task.frame);
        drain();
      });
    }
  };
  const schedule = (task) => {
    if (closed || queued.has(task.frame) || live.has(task.frame)) return;
    queued.add(task.frame);
    work.push(task);
    drain();
  };
  const known = new Map();
  if (typeof IntersectionObserver === "function") {
    observer = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        observer.unobserve(entry.target);
        const task = known.get(entry.target);
        if (task) schedule(task);
      }
    }, { rootMargin: "300px" });
  }
  return (frame, id, alt) => {
    requireValue(root.contains(frame) && known.size < 64);
    const task = { frame, id, alt };
    known.set(frame, task);
    if (observer) observer.observe(frame);
    else schedule(task);
  };
}

function checkedId(value) {
  requireValue(typeof value === "string" && /^[1-9][0-9]{0,18}$/.test(value));
  return value;
}

async function api(url, options) {
  const response = await fetch(url, {
    credentials: "same-origin",
    cache: "no-store",
    referrerPolicy: "no-referrer",
    ...options,
  });
  if (!response.ok) throw fail();
  return response.json();
}

// H/S065: active controls for the standalone NEwBOO reader.
// Apply only enumerated themes and bounded numbers. Preferences live only
// in this document; encrypted chapters, keys and settings never reach storage.
export function installPaidReaderSettings(root) {
  const panel = root.querySelector("[data-paid-reader-settings]");
  if (!panel) return;
  const toggle = root.querySelector("[data-paid-settings-open]");
  const themeCycle = root.querySelector("[data-paid-theme-cycle]");
  const dialog = panel.querySelector(".reader-paid__settings-panel");
  const backdrop = panel.querySelector(".reader-paid__settings-backdrop");
  const close = dialog?.querySelector("[data-paid-settings-close]");
  const reset = panel.querySelector("[data-paid-settings-reset]");
  const font = panel.querySelector("[data-paid-font-scale]");
  const line = panel.querySelector("[data-paid-line-height]");
  const width = panel.querySelector("[data-paid-reader-width]");
  const fontOutput = panel.querySelector("[data-paid-font-output]");
  const lineOutput = panel.querySelector("[data-paid-line-output]");
  const widthOutput = panel.querySelector("[data-paid-width-output]");
  const themes = [...panel.querySelectorAll("[data-paid-theme]")];
  const fonts = [...panel.querySelectorAll("[data-paid-font-family]")];
  requireValue(toggle && themeCycle && dialog && backdrop && close && reset &&
    font && line && width && fontOutput && lineOutput &&
    themes.length === 3 && fonts.length === 2);

  const boundedValue = (input, min, max, step) => {
    const number = Number(input.value);
    requireValue(Number.isInteger(number) && number >= min &&
      number <= max && (number - min) % step === 0);
    return number;
  };
  const refresh = () => {
    const scale = boundedValue(font, 80, 140, 5);
    const spacing = boundedValue(line, 150, 220, 5);
    const pixels = boundedValue(width, 620, 980, 20);
    fontOutput.textContent = String(scale) + "%";
    lineOutput.textContent = (spacing / 100).toFixed(2);
    widthOutput.textContent = String(pixels) + " px";
    if (scale === 100) root.style.removeProperty("--paid-font-size");
    else root.style.setProperty("--paid-font-size", String(scale / 5) + "px");
    if (spacing === 185) root.style.removeProperty("--paid-line-height");
    else root.style.setProperty("--paid-line-height", String(spacing / 100));
    if (pixels === 860) root.style.removeProperty("--paid-reader-width");
    else root.style.setProperty("--paid-reader-width", String(pixels) + "px");
  };
  const selectTheme = (choice) => {
    requireValue(["system", "light", "sepia", "dark"].includes(choice));
    if (choice === "system") delete document.body.dataset.paidReaderTheme;
    else document.body.dataset.paidReaderTheme = choice;
    for (const button of themes) {
      button.setAttribute("aria-pressed",
        String(button.dataset.paidTheme === choice));
    }
    const current = {
      system: "системная", light: "светлая", sepia: "сепия", dark: "тёмная",
    }[choice];
    themeCycle.setAttribute("aria-label", "Сменить тему. Сейчас: " + current);
    themeCycle.title = "Сменить тему. Сейчас: " + current;
  };
  // The original NEwBOO dialog offers serif/sans-serif selection. Neither
  // web fonts nor a network request are required: only trusted CSS stacks.
  const selectFont = (value) => {
    requireValue(value === "serif" || value === "sans");
    if (value === "serif") root.style.removeProperty("--paid-font-family");
    else root.style.setProperty("--paid-font-family", "Arial, Helvetica, sans-serif");
    for (const button of fonts) {
      button.setAttribute("aria-pressed",
        String(button.dataset.paidFontFamily === value));
    }
  };
  // The original NEwBOO settings are modal. Make the rest of the reader
  // inert while the panel is open; do not hide any content from the DOM.
  const backdropTargets = [
    root.querySelector(".reader-topbar"),
    ...[...panel.parentElement.children].filter((child) => child !== panel),
  ].filter(Boolean);
  let previousInert = [];
  let previousOverflow = "";
  const hide = () => {
    if (panel.hidden) return;
    panel.hidden = true;
    for (const [node, wasInert] of previousInert) node.inert = wasInert;
    previousInert = [];
    document.body.style.overflow = previousOverflow;
    toggle.setAttribute("aria-expanded", "false");
    toggle.focus();
  };
  const show = () => {
    if (!panel.hidden) return;
    previousOverflow = document.body.style.overflow;
    previousInert = backdropTargets.map((node) => [node, node.inert]);
    for (const [node] of previousInert) node.inert = true;
    panel.hidden = false;
    document.body.style.overflow = "hidden";
    toggle.setAttribute("aria-expanded", "true");
    dialog.focus();
  };
  toggle.addEventListener("click", () => {
    if (panel.hidden) show();
    else hide();
  });
  close.addEventListener("click", hide);
  backdrop.addEventListener("click", hide);
  document.addEventListener("keydown", (event) => {
    if (panel.hidden) return;
    if (event.key === "Escape") {
      event.preventDefault();
      hide();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = [...dialog.querySelectorAll(
      "button:not([disabled]), input:not([disabled]), a[href]",
    )].filter((node) => node.getClientRects().length > 0);
    if (!focusable.length) {
      event.preventDefault();
      dialog.focus();
      return;
    }
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    const active = document.activeElement;
    if (event.shiftKey && (active === first || active === dialog ||
      !dialog.contains(active))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (active === last || !dialog.contains(active))) {
      event.preventDefault();
      first.focus();
    }
  });
  for (const input of [font, line, width]) {
    input.addEventListener("input", refresh);
  }
  for (const button of themes) {
    button.addEventListener("click", () => selectTheme(button.dataset.paidTheme));
  }
  for (const button of fonts) {
    button.addEventListener("click", () => selectFont(button.dataset.paidFontFamily));
  }
  // NEwBOO one-tap theme switch: changing a mode updates the same settings
  // controls. An untouched page still follows the operating system.
  themeCycle.addEventListener("click", () => {
    const effective = document.body.dataset.paidReaderTheme ||
      (window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
    const next = { light: "sepia", sepia: "dark", dark: "light" }[effective];
    requireValue(next);
    selectTheme(next);
  });
  reset.addEventListener("click", () => {
    font.value = "100";
    line.value = "185";
    width.value = "860";
    refresh();
    selectTheme("system");
    selectFont("serif");
  });
  refresh();
  selectTheme("system");
  selectFont("serif");
}

export async function startPaidReader(root) {
  const chapter = checkedId(root.dataset.chapterId);
  const purchase = checkedId(root.dataset.purchaseId);
  const csrf = root.dataset.csrfToken;
  const s3Origin = root.dataset.s3Origin;
  requireValue(/^[A-Za-z0-9_-]{43}$/.test(csrf || ""));
  const status = root.querySelector("[data-paid-status]");
  const article = root.querySelector("[data-paid-content]");
  const resume = root.querySelector("[data-paid-resume]");
  const completion = root.querySelector("[data-paid-completion]");
  requireValue(status && article && resume && completion);
  status.textContent = "Проверяем покупку и загружаем защищённую версию…";
  const grant = await api(
    "/api/v1/reader/chapters/" + chapter + "/delivery-grants",
    {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-CSRF-Token": csrf },
      body: JSON.stringify({ purchase_id: purchase }),
    }
  );
  const { versionId, text, plaintextSha256 } = await decryptPaidGrant(grant, chapter, s3Origin);
  let revision = 0;
  const url = "/api/v1/me/progress/purchased";
  const lookup = new URLSearchParams({
    chapter_id: chapter, purchase_id: purchase, version_id: versionId,
  });
  const stored = await api(url + "?" + lookup.toString());
  revision = stored.revision;
  const showPrivateImage = privateImageLoader(
    root, chapter, versionId, purchase, csrf, s3Origin
  );
  const blocks = renderPaidBlocks(article, text, async (blockId, charOffset) => {
    try {
      const saved = await api(url, {
        method: "PUT",
        headers: { "Content-Type": "application/json", "X-CSRF-Token": csrf },
        body: JSON.stringify({
          chapter_id: chapter, purchase_id: purchase, version_id: versionId,
          block_id: blockId, char_offset: charOffset, expected_revision: revision,
        }),
      });
      revision = saved.revision;
      resume.textContent = "Место сохранено.";
    } catch {
      resume.textContent = "Не удалось сохранить место. Обновите страницу и повторите.";
    }
  }, showPrivateImage);
  if (stored.block_id && document.getElementById("p-" + stored.block_id)) {
    const anchor = document.createElement("a");
    anchor.href = "#p-" + stored.block_id;
    anchor.textContent = "К сохранённому месту";
    resume.replaceChildren(anchor);
  } else {
    resume.textContent = "Место ещё не сохранено.";
  }
  // A bookmark or download is not proof of completion. Only this separate,
  // user-initiated action may request a C completion from a G-sealed anchor.
  const finishUrl = "/api/v1/me/progress/purchased/complete";
  const capability = await api(finishUrl + "?" + lookup.toString());
  completion.hidden = false;
  if (capability.completed) {
    completion.textContent = "Глава уже отмечена прочитанной.";
  } else if (!capability.available) {
    completion.textContent = "Для этой редакции отметка прочтения пока недоступна.";
  } else {
    let last = -1;
    blocks.forEach((block, index) => { if (block.trim()) last = index; });
    requireValue(last >= 0);
    const button = document.createElement("button");
    button.type = "button";
    button.className = "button reader-paid__finish";
    button.textContent = "Глава прочитана";
    const message = document.createElement("span");
    message.setAttribute("role", "status");
    message.setAttribute("aria-live", "polite");
    button.addEventListener("click", async () => {
      button.disabled = true;
      message.textContent = "Сохраняем прочтение…";
      try {
        const result = await api(finishUrl, {
          method: "POST",
          headers: { "Content-Type": "application/json", "X-CSRF-Token": csrf },
          body: JSON.stringify({
            chapter_id: chapter, purchase_id: purchase, version_id: versionId,
            block_id: last + 1, char_offset: blocks[last].length,
            expected_revision: revision, plaintext_sha256: plaintextSha256,
          }),
        });
        requireValue(result.status === "completed" || result.status === "already_completed");
        completion.replaceChildren(document.createTextNode("Глава отмечена прочитанной. "));
        const ranking = document.createElement("a");
        ranking.href = "/me/rankings";
        ranking.textContent = "Открыть мой рейтинг";
        ranking.className = "text-link";
        completion.append(ranking);
      } catch {
        button.disabled = false;
        message.textContent = "Не удалось отметить главу. Проверьте доступ и повторите.";
      }
    });
    completion.replaceChildren(button, message);
  }
  status.textContent = "Оплаченная версия " + versionId + " · Защищённая загрузка завершена";
}

if (typeof document !== "undefined") {
  const root = document.querySelector("[data-paid-chapter-reader]");
  if (root) {
    installPaidReaderSettings(root);
    startPaidReader(root).catch(() => {
      const status = root.querySelector("[data-paid-status]");
      if (status) status.textContent = "Не удалось открыть главу. Проверьте доступ и повторите попытку.";
      const article = root.querySelector("[data-paid-content]");
      if (article) article.replaceChildren();
    });
  }
}
