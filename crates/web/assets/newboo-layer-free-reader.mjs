// D-05 / BR-060: archived FREE version of an ordinarily deleted Book.
// A public /reader or free-grant request is NEVER an alternative delivery
// source. The server selects immutable V1 using the paid Layer origin.
function requireSafe(value) {
  if (!value) throw new Error("Архивная глава недоступна.");
}

async function readBounded(response, limit) {
  const reader = response.body?.getReader();
  requireSafe(reader && limit > 0 && limit <= 200000);
  const chunks = [];
  let size = 0;
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      requireSafe(value instanceof Uint8Array);
      size += value.byteLength;
      requireSafe(size <= limit);
      chunks.push(value);
    }
  } catch (error) {
    await reader.cancel().catch(() => {});
    throw error;
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const part of chunks) {
    bytes.set(part, offset);
    offset += part.byteLength;
  }
  return bytes;
}

async function openLayerFree(root) {
  const chapter = root.dataset.chapterId;
  const csrf = root.dataset.csrfToken;
  const origin = root.dataset.s3Origin;
  const status = root.querySelector("[data-layer-free-status]");
  const article = root.querySelector("[data-layer-free-content]");
  requireSafe(/^[1-9][0-9]{0,18}$/.test(chapter || ""));
  requireSafe(/^[A-Za-z0-9_-]{43}$/.test(csrf || ""));
  requireSafe(typeof origin === "string" && origin.length > 0);
  requireSafe(status && article);

  const reply = await fetch("/api/v1/reader/layer/free/" + chapter + "/grant", {
    method: "POST",
    credentials: "same-origin",
    cache: "no-store",
    redirect: "error",
    referrerPolicy: "no-referrer",
    headers: { "X-CSRF-Token": csrf, "Accept": "application/json" }
  });
  requireSafe(reply.ok && reply.status === 200 && !reply.redirected);
  requireSafe(/^application\/json\b/i.test(reply.headers.get("content-type") || ""));
  const grant = await reply.json();
  requireSafe(grant?.schema === "tor.layer-free-s3-grant.v1");
  requireSafe(grant.chapter_id === chapter);
  requireSafe(/^[1-9][0-9]{0,18}$/.test(grant.version_id || ""));
  requireSafe(typeof grant.size_bytes === "number" &&
    Number.isSafeInteger(grant.size_bytes) &&
    grant.size_bytes > 0 && grant.size_bytes <= 200000);
  requireSafe(typeof grant.sha256 === "string" && /^[a-f0-9]{64}$/.test(grant.sha256));
  requireSafe(typeof grant.url === "string" && grant.url.length < 4096);

  const target = new URL(grant.url);
  const allowed = new URL(origin);
  const expires = target.searchParams.get("X-Amz-Expires");
  requireSafe(target.origin === allowed.origin &&
    (target.protocol === "https:" ||
      (target.protocol === "http:" && ["localhost", "127.0.0.1"].includes(target.hostname))) &&
    !target.username && !target.password && !target.hash);
  requireSafe(target.pathname.endsWith(
    "/private/free-chapters/" + chapter + "/" + grant.version_id + ".md"));
  requireSafe(target.searchParams.getAll("X-Amz-Signature").length === 1 &&
    /^[a-f0-9]{64}$/.test(target.searchParams.get("X-Amz-Signature") || "") &&
    /^\d+$/.test(expires || "") &&
    Number(expires) >= 30 && Number(expires) <= 120);

  // A malicious/misconfigured storage response must not be allowed to turn
  // this into an unbounded download or content-version substitution.
  const data = await fetch(target.href, {
    method: "GET",
    credentials: "omit",
    mode: "cors",
    cache: "no-store",
    redirect: "error",
    referrerPolicy: "no-referrer"
  });
  requireSafe(data.status === 200 && data.ok && !data.redirected);
  const bytes = await readBounded(data, grant.size_bytes);
  requireSafe(bytes.byteLength === grant.size_bytes);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  const hex = [...new Uint8Array(digest)].map(byte =>
    byte.toString(16).padStart(2, "0")).join("");
  requireSafe(hex === grant.sha256);
  const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);

  // Never execute Markdown or HTML from a deleted Book.
  const sections = document.createDocumentFragment();
  const blocks = text.replace(/\r\n?/g, "\n").split(/\n\s*\n/);
  for (const block of blocks) {
    if (!block.trim()) continue;
    const p = document.createElement("p");
    p.className = "reader-piece__text reader-markdown";
    p.textContent = block;
    sections.append(p);
  }
  article.replaceChildren(sections);
  status.textContent = "Архивная редакция · защищённое чтение";
}

if (typeof document !== "undefined") {
  const root = document.querySelector("[data-layer-free-reader]");
  if (root) {
    openLayerFree(root).catch(() => {
      const status = root.querySelector("[data-layer-free-status]");
      const article = root.querySelector("[data-layer-free-content]");
      if (article) article.replaceChildren();
      if (status) status.textContent = "Архивное чтение недоступно. Проверьте подписку.";
    });
  }
}
