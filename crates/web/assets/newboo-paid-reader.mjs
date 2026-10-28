// H/S034: memory-only paid-reader WebCrypto, no plaintext or keys in storage.
// Browser page must be served by TOR with CSRF and an operator-pinned S3 origin.
const MAX_CIPHERTEXT_BYTES = 16 * 1024 * 1024;
const MAX_PLAINTEXT_BYTES = 1024 * 1024;
const MAX_SIGNED_SECONDS = 120;

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

function verifiedS3URL(raw, configuredOrigin) {
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
      /\/private\/chapter-versions\/[0-9a-f-]{36}\.enc$/.test(url.pathname) &&
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
      Number(declared) <= MAX_CIPHERTEXT_BYTES);
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
      requireValue(received <= MAX_CIPHERTEXT_BYTES);
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

export function renderPaidBlocks(container, text, onSave) {
  const blocks = readerBlocks(text);
  container.replaceChildren();
  blocks.forEach((block, index) => {
    const section = document.createElement("section");
    section.className = "reader-piece";
    section.id = "p-" + (index + 1);
    const paragraph = document.createElement("p");
    paragraph.className = "reader-piece__text reader-markdown";
    // Source Markdown is treated as data, never evaluated as raw HTML.
    paragraph.textContent = block;
    const button = document.createElement("button");
    button.type = "button";
    button.className = "reader-save-button";
    button.textContent = "Сохранить место здесь";
    button.setAttribute("aria-label", "Сохранить после фрагмента " + (index + 1));
    button.addEventListener("click", () => onSave(index + 1, block.length));
    section.append(paragraph, button);
    container.append(section);
  });
  return blocks;
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
  const close = panel.querySelector("[data-paid-settings-close]");
  const reset = panel.querySelector("[data-paid-settings-reset]");
  const font = panel.querySelector("[data-paid-font-scale]");
  const line = panel.querySelector("[data-paid-line-height]");
  const width = panel.querySelector("[data-paid-reader-width]");
  const fontOutput = panel.querySelector("[data-paid-font-output]");
  const lineOutput = panel.querySelector("[data-paid-line-output]");
  const widthOutput = panel.querySelector("[data-paid-width-output]");
  const themes = [...panel.querySelectorAll("[data-paid-theme]")];
  requireValue(toggle && themeCycle && close && reset && font && line && width &&
    fontOutput && lineOutput && widthOutput && themes.length === 3);

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
  const hide = () => {
    panel.hidden = true;
    toggle.setAttribute("aria-expanded", "false");
    toggle.focus();
  };

  toggle.addEventListener("click", () => {
    panel.hidden = !panel.hidden;
    toggle.setAttribute("aria-expanded", String(!panel.hidden));
    if (!panel.hidden) close.focus();
  });
  close.addEventListener("click", hide);
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !panel.hidden) {
      event.preventDefault();
      hide();
    }
  });
  for (const input of [font, line, width]) {
    input.addEventListener("input", refresh);
  }
  for (const button of themes) {
    button.addEventListener("click", () => selectTheme(button.dataset.paidTheme));
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
  });
  refresh();
  selectTheme("system");
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
  });
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
