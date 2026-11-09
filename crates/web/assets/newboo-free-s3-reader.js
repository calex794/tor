// S349: browser downloads a verified FREE edition directly from private S3.
// No chapter text is routed through TOR, injected as HTML or persisted locally.
(() => {
  "use strict";
  const article = document.querySelector("#chapter-text[data-free-s3-chapter]");
  if (!article) return;
  const chapter = article.dataset.freeS3Chapter;
  const version = article.dataset.freeS3Version;
  const csrf = article.dataset.freeS3Csrf || "";
  if (!/^[1-9]\d*$/.test(chapter || "") ||
      !/^[1-9]\d*$/.test(version || "")) return;

  const failure = () => {
    article.replaceChildren();
    const notice = document.createElement("p");
    notice.setAttribute("role", "alert");
    notice.textContent = "Текст главы недоступен. Проверьте соединение и обновите страницу.";
    article.append(notice);
  };
  const progress = (text) => {
    if (!/^[A-Za-z0-9_-]{43}$/.test(csrf)) return;
    const paragraphs = text.split("\n\n");
    const groupSize = Math.ceil(paragraphs.length / 200);
    const blocks = [];
    for (let index = 0; index < paragraphs.length; index += groupSize) {
      blocks.push(paragraphs.slice(index, index + groupSize).join("\n\n"));
    }
    let start = 0;
    const positions = [];
    for (const block of blocks) {
      positions.push(start + ":" + block.length);
      start += block.length + 2;
    }
    article.dataset.freeProgressChapter = chapter;
    article.dataset.freeProgressVersion = version;
    article.dataset.freeProgressBlocks = positions.join(",");
    article.dataset.freeProgressCsrf = csrf;
    const script = document.createElement("script");
    script.src = "/assets/newboo-free-progress-s3.js";
    script.defer = true;
    document.head.append(script);
  };
  const render = (text) => {
    const fragment = document.createDocumentFragment();
    let offset = 0;
    for (const paragraph of text.split("\n\n")) {
      const p = document.createElement("p");
      const span = document.createElement("span");
      span.dataset.mdStart = String(offset);
      span.textContent = paragraph;
      p.append(span);
      fragment.append(p);
      offset += paragraph.length + 2;
    }
    article.replaceChildren(fragment);
    progress(text);
  };
  const readBounded = async (response, limit) => {
    const stream = response.body?.getReader();
    if (!stream) throw Error("missing stream");
    const parts = [];
    let size = 0;
    for (;;) {
      const { done, value } = await stream.read();
      if (done) break;
      size += value.length;
      if (size > limit) {
        await stream.cancel();
        throw Error("oversize");
      }
      parts.push(value);
    }
    const bytes = new Uint8Array(size);
    let offset = 0;
    for (const part of parts) {
      bytes.set(part, offset);
      offset += part.length;
    }
    return bytes;
  };
  (async () => {
    try {
      const grantResponse = await fetch(
        "/api/v1/chapters/" + encodeURIComponent(chapter) + "/free-grant", {
          credentials: "same-origin", cache: "no-store",
          headers: { "Accept": "application/json" }
        }
      );
      if (!grantResponse.ok) throw Error("grant denied");
      const grant = await grantResponse.json();
      if (grant.schema !== "tor.free-s3-grant.v1" ||
          grant.chapter_id !== chapter || grant.version_id !== version ||
          !Number.isInteger(grant.size_bytes) || grant.size_bytes < 1 ||
          grant.size_bytes > 200000 ||
          !/^[a-f0-9]{64}$/.test(grant.sha256) ||
          typeof grant.url !== "string") throw Error("invalid grant");
      const target = new URL(grant.url);
      if (target.protocol !== "https:" &&
          !(target.protocol === "http:" &&
            (target.hostname === "localhost" || target.hostname === "127.0.0.1"))) {
        throw Error("unsafe transport");
      }
      const response = await fetch(target, { credentials: "omit", cache: "no-store" });
      if (!response.ok || response.type === "opaque") throw Error("storage unavailable");
      const bytes = await readBounded(response, grant.size_bytes);
      if (bytes.length !== grant.size_bytes) throw Error("size mismatch");
      const digest = await crypto.subtle.digest("SHA-256", bytes);
      const actual = [...new Uint8Array(digest)]
        .map(value => value.toString(16).padStart(2, "0")).join("");
      if (actual !== grant.sha256) throw Error("hash mismatch");
      const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes)
        .replace(/\r\n?/g, "\n");
      if (text.length < 1 || [...text].length > 50000) throw Error("invalid length");
      render(text);
    } catch {
      failure();
    }
  })();
})();
