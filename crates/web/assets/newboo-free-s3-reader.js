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
  const progress = (text, withComments = false) => {
    if (!/^[A-Za-z0-9_-]{43}$/.test(csrf) && !withComments) return;
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
    if (article.dataset.freePrivateRevision !== undefined) {
      const spans = [...article.querySelectorAll("span[data-md-start]")];
      positions.forEach((value, index) => {
        const start = Number(value.split(":")[0]);
        const span = spans.find(node => Number(node.dataset.mdStart) >= start) || spans.at(-1);
        if (span) {
          const anchor = document.createElement("span"); anchor.id = "p-" + (index + 1);
          span.before(anchor);
        }
      });
    }
    if (article.dataset.freeS3Archive === "1") article.dataset.freeProgressArchive = "1";
    const script = document.createElement("script");
    script.src = article.dataset.freeS3Archive === "1"
      ? "/assets/newboo-archive-progress.js" : "/assets/newboo-free-progress-s3.js";
    script.defer = true;
    document.head.append(script);
  };
  // Mirror the finite C renderer: construct inert nodes, never innerHTML.
  const segment = (parent, text, offset) => {
    if (!text) return;
    const span = document.createElement("span");
    span.dataset.mdStart = String(offset);
    span.textContent = text;
    parent.append(span);
  };
  const safeLink = value => {
    if (!value || value.length > 2048 || /[\s\u0000-\u001f\u007f<>"'\\{}\x60]/u.test(value)) return false;
    if (value.startsWith("/") && !value.startsWith("//") || value.startsWith("#")) return true;
    const match = /^https?:\/\/([^/?#]+)/.exec(value);
    return !!match && !match[1].startsWith(".") && !match[1].includes("@");
  };
  const inline = (parent, text, base, depth = 0) => {
    let i = 0, plain = 0;
    while (i < text.length) {
      const rest = text.slice(i), candidate = rest.slice(0, 2048);
      let consumed = 0;
      if (depth < 6) {
        if (rest[0] === "[" && (i === 0 || text[i - 1] !== "!")) {
          const labelEnd = candidate.indexOf("]("), end = candidate.indexOf(")", labelEnd + 2);
          if (labelEnd > 1 && end > labelEnd && safeLink(candidate.slice(labelEnd + 2, end))) {
            segment(parent, text.slice(plain, i), base + plain);
            const a = document.createElement("a");
            a.setAttribute("href", candidate.slice(labelEnd + 2, end));
            a.setAttribute("rel", "nofollow noopener noreferrer");
            inline(a, candidate.slice(1, labelEnd), base + i + 1, depth + 1);
            parent.append(a); consumed = end + 1;
          }
        }
        if (!consumed && rest[0] === "\x60") {
          const end = candidate.indexOf("\x60", 1);
          if (end > 0) {
            segment(parent, text.slice(plain, i), base + plain);
            const code = document.createElement("code");
            segment(code, candidate.slice(1, end), base + i + 1);
            parent.append(code); consumed = end + 1;
          }
        }
        if (!consumed) for (const [marker, tag] of [["**", "strong"], ["__", "strong"], ["*", "em"], ["_", "em"]]) {
          if (marker[0] === "_" && i > 0 && /[\p{L}\p{N}]$/u.test(text.slice(Math.max(0, i - 2), i))) continue;
          if (!rest.startsWith(marker) || marker.length === 1 && rest.startsWith(marker.repeat(2))) continue;
          const end = candidate.indexOf(marker, marker.length);
          if (end > marker.length) {
            segment(parent, text.slice(plain, i), base + plain);
            const node = document.createElement(tag);
            inline(node, candidate.slice(marker.length, end), base + i + marker.length, depth + 1);
            parent.append(node); consumed = end + marker.length; break;
          }
        }
      }
      if (consumed) { i += consumed; plain = i; }
      else i += text.codePointAt(i) > 0xffff ? 2 : 1;
    }
    segment(parent, text.slice(plain), base + plain);
  };
  const listPrefix = line => /^[-*+] /.test(line) ? ["ul", 2] :
    (/^\d{1,9}\. /.exec(line)?.[0] ? ["ol", /^\d{1,9}\. /.exec(line)[0].length] : null);
  const heading = line => /^(#{1,6}) /.exec(line);
  const special = line => heading(line) || listPrefix(line) || line.startsWith("> ") || line.startsWith("\x60\x60\x60");
  const savedAnchor = text => {
    if (!article.dataset.freeSavedBlock) return;
    const block = Number(article.dataset.freeSavedBlock), offset = Number(article.dataset.freeSavedOffset);
    const paragraphs = text.split("\n\n"), size = Math.ceil(paragraphs.length / 200), blocks = [];
    for (let i = 0; i < paragraphs.length; i += size) blocks.push(paragraphs.slice(i, i + size).join("\n\n"));
    if (!Number.isInteger(block) || block < 1 || block > blocks.length ||
        !Number.isInteger(offset) || offset < 0 || offset > blocks[block - 1].length) return;
    const target = blocks.slice(0, block - 1).reduce((sum, value) => sum + value.length + 2, 0) + offset;
    const spans = [...article.querySelectorAll("span[data-md-start]")];
    const span = spans.findLast(node => Number(node.dataset.mdStart) <= target) || spans[0];
    if (!span?.firstChild || span.firstChild.nodeType !== Node.TEXT_NODE) return;
    let local = Math.min(span.firstChild.length, Math.max(0, target - Number(span.dataset.mdStart)));
    // A cursor in the middle of a UTF-16 pair rounds down, as the C renderer does.
    if (local > 0 && /[\udc00-\udfff]/.test(span.firstChild.textContent[local] || "") &&
        /[\ud800-\udbff]/.test(span.firstChild.textContent[local - 1])) local--;
    const remainder = span.firstChild.splitText(local), tail = document.createElement("span");
    tail.dataset.mdStart = String(Number(span.dataset.mdStart) + local); tail.append(remainder);
    const marker = document.createElement("span"); marker.id = "saved-resume";
    span.after(marker, tail);
  };
  const completion = text => {
    if (article.dataset.freePrivateRevision === undefined ||
        article.dataset.freePrivateCompleted === "true" || !/^[A-Za-z0-9_-]{43}$/.test(csrf)) return;
    const parts = text.split("\n\n"), size = Math.ceil(parts.length / 200), blocks = [];
    for (let i = 0; i < parts.length; i += size) blocks.push(parts.slice(i, i + size).join("\n\n"));
    const last = blocks.findLastIndex(block => block.trim());
    if (last < 0) return;
    const form = document.createElement("form"), button = document.createElement("button");
    form.method = "post"; form.action = "/read/" + chapter + "/complete";
    const fields = {};
    for (const [name, value] of Object.entries({ csrf_token: csrf, version_id: version,
      block_index: last + 1, char_offset: blocks[last].length,
      expected_revision: article.dataset.freePrivateRevision })) {
      const input = document.createElement("input"); input.type = "hidden";
      input.name = name; input.value = String(value); fields[name] = input; form.append(input);
    }
    button.type = "submit"; button.textContent = "Глава прочитана"; form.append(button);
    form.addEventListener("submit", async event => {
      event.preventDefault(); button.disabled = true;
      try {
        const response = await fetch("/api/v1/me/progress/free?chapter_id=" + chapter,
          { credentials: "same-origin", cache: "no-store", redirect: "error" });
        if (response.status !== 200) throw Error("progress unavailable");
        const state = await response.json();
        if (!Number.isSafeInteger(state.revision) || state.revision < 0) throw Error("invalid progress");
        fields.expected_revision.value = String(state.revision); form.submit();
      } catch {
        button.disabled = false; button.textContent = "Повторить отметку о прочтении";
      }
    });
    article.append(form);
  };
  const render = (text) => {
    const fragment = document.createDocumentFragment();
    let offset = 0;
    const lines = text.split("\n").map(value => { const line = { value, offset }; offset += value.length + 1; return line; });
    for (let i = 0; i < lines.length;) {
      const line = lines[i];
      if (!line.value.trim()) { i++; continue; }
      let node;
      if (line.value.startsWith("\x60\x60\x60")) {
        node = document.createElement("pre");
        const code = document.createElement("code"); node.append(code); i++;
        while (i < lines.length && !lines[i].value.startsWith("\x60\x60\x60")) {
          segment(code, lines[i].value + (i + 1 < lines.length ? "\n" : ""), lines[i].offset); i++;
        }
        if (i < lines.length) i++;
      } else if (heading(line.value)) {
        const match = heading(line.value);
        node = document.createElement("h" + match[1].length);
        inline(node, line.value.slice(match[0].length), line.offset + match[0].length); i++;
      } else if (listPrefix(line.value)) {
        const kind = listPrefix(line.value)[0]; node = document.createElement(kind);
        while (i < lines.length && listPrefix(lines[i].value)?.[0] === kind) {
          const item = document.createElement("li"), prefix = listPrefix(lines[i].value)[1];
          inline(item, lines[i].value.slice(prefix), lines[i].offset + prefix); node.append(item); i++;
        }
      } else if (line.value.startsWith("> ")) {
        node = document.createElement("blockquote"); const p = document.createElement("p"); node.append(p);
        let first = true;
        while (i < lines.length && lines[i].value.startsWith("> ")) {
          if (!first) segment(p, " ", lines[i].offset - 1);
          first = false;
          inline(p, lines[i].value.slice(2), lines[i].offset + 2); i++;
        }
      } else {
        node = document.createElement("p"); inline(node, line.value, line.offset); i++;
        while (i < lines.length && lines[i].value.trim() && !special(lines[i].value)) {
          segment(node, " ", lines[i].offset - 1);
          inline(node, lines[i].value, lines[i].offset); i++;
        }
      }
      fragment.append(node);
    }
    article.replaceChildren(fragment);
    savedAnchor(text);
    const panel = article.closest?.("[data-free-reader]")?.querySelector("[data-chapter-comments]");
    if (panel) {
      const paragraphs = text.split("\n\n"), size = Math.ceil(paragraphs.length / 200), positions = [];
      let start = 0;
      for (let i = 0; i < paragraphs.length; i += size) {
        const length = paragraphs.slice(i, i + size).join("\n\n").length;
        positions.push(start + ":" + length); start += length + 2;
      }
      panel.dataset.readerCommentBlocks = positions.join(",");
    }
    completion(text);
    progress(text, !!panel);
  };
  const readBounded = async (response, limit) => {
    const stream = response.body?.getReader();
    if (!stream) throw Error("missing stream");
    const parts = [];
    let size = 0;
    try {
      for (;;) {
        const { done, value } = await stream.read();
        if (done) break;
        size += value.byteLength;
        if (size > limit) throw Error("oversize");
        parts.push(value);
      }
    } catch (error) {
      await stream.cancel().catch(() => {});
      throw error;
    } finally {
      stream.releaseLock();
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
    const controller = new AbortController();
    const deadline = setTimeout(() => controller.abort(), 15000);
    try {

      const query = article.dataset.freeS3Saved === "1"
        ? "?mode=saved&version_id=" + version : "";
      const common = {
        credentials: "same-origin", cache: "no-store", redirect: "error",
        referrerPolicy: "no-referrer", signal: controller.signal
      };
      const parsePlan = async response => {
        if (response.status !== 200 || response.redirected ||
            !/^application\/json\b/i.test(response.headers.get("content-type") || "")) {
          throw Error("source plan denied");
        }
        const plan = JSON.parse(new TextDecoder("utf-8", { fatal: true })
          .decode(await readBounded(response, 4096)));
        if (plan?.schema !== "tor.free-source-plan.v1" ||
            plan.chapter_id !== chapter || plan.version_id !== version ||
            !["gateway", "signed-single"].includes(plan.transport) ||
            !Number.isInteger(plan.size_bytes) || plan.size_bytes < 1 ||
            plan.size_bytes > 200000 ||
            !/^[a-f0-9]{64}$/.test(plan.sha256 || "")) throw Error("invalid plan");
        return plan;
      };
      const plan = article.dataset.freePackReader === "1"
        ? await parsePlan(await fetch(
            "/api/v1/chapters/" + encodeURIComponent(chapter) + "/free-source-plan" + query,
            { ...common, headers: { "Accept": "application/json" } }))
        : null;
      let bytes, expected;
      if (plan?.transport === "gateway") {
        const response = await fetch(
          "/api/v1/chapters/" + encodeURIComponent(chapter) + "/free-pack-source" + query,
          { ...common, headers: { "Accept": "text/plain" } });
        if (response.status !== 200 || response.redirected ||
            !/^text\/plain\b/i.test(response.headers.get("content-type") || "") ||
            response.headers.get("x-tor-free-version-id") !== version ||
            response.headers.get("x-tor-free-sha256") !== plan.sha256 ||
            response.headers.get("x-tor-free-size") !== String(plan.size_bytes)) {
          throw Error("invalid PACK source");
        }
        bytes = await readBounded(response, plan.size_bytes);
        expected = plan;
      } else {
        // A signed SINGLE is permitted only when the server selected SINGLE.
        // Failure to fetch a selected PACK never downgrades to this path.
        const grantResponse = await fetch(
          "/api/v1/chapters/" + encodeURIComponent(chapter) + "/free-grant" + query,
          { ...common, headers: { "Accept": "application/json" } });
        if (grantResponse.status !== 200 || grantResponse.redirected) throw Error("grant denied");
        const grant = JSON.parse(new TextDecoder("utf-8", { fatal: true })
          .decode(await readBounded(grantResponse, 16384)));
        if (grant?.schema !== "tor.free-s3-grant.v1" ||
            grant.chapter_id !== chapter || grant.version_id !== version ||
            !Number.isInteger(grant.size_bytes) || grant.size_bytes < 1 ||
            grant.size_bytes > 200000 ||
            typeof grant.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(grant.sha256) ||
            typeof grant.url !== "string" || grant.url.length > 4096 ||
            (plan && (plan.transport !== "signed-single" ||
              plan.sha256 !== grant.sha256 || plan.size_bytes !== grant.size_bytes))) {
          throw Error("invalid grant");
        }
        const target = new URL(grant.url);
        const expires = target.searchParams.get("X-Amz-Expires");
        if ((target.protocol !== "https:" &&
             !(target.protocol === "http:" &&
               (target.hostname === "localhost" || target.hostname === "127.0.0.1"))) ||
            target.username || target.password || target.hash ||
            !target.pathname.endsWith("/private/free-chapters/" + chapter + "/" + version + ".md") ||
            target.searchParams.getAll("X-Amz-Signature").length !== 1 ||
            !/^[a-f0-9]{64}$/.test(target.searchParams.get("X-Amz-Signature") || "") ||
            !/^\d+$/.test(expires || "") || Number(expires) < 30 || Number(expires) > 120) {
          throw Error("unsafe storage capability");
        }
        const response = await fetch(target, {
          method: "GET", mode: "cors", credentials: "omit", cache: "no-store",
          redirect: "error", referrerPolicy: "no-referrer", signal: controller.signal
        });
        if (response.status !== 200 || response.type === "opaque" || response.redirected) {
          throw Error("storage unavailable");
        }
        bytes = await readBounded(response, grant.size_bytes);
        expected = grant;
      }
      if (bytes.length !== expected.size_bytes) throw Error("size mismatch");
      const digest = await crypto.subtle.digest("SHA-256", bytes);
      const actual = [...new Uint8Array(digest)]
        .map(value => value.toString(16).padStart(2, "0")).join("");
      if (actual !== expected.sha256) throw Error("hash mismatch");
      const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes)
        .replace(/\r\n?/g, "\n");
      if (text.length < 1 || text.includes("\0") || [...text].length > 50000) {
        throw Error("invalid text");
      }
      render(text);
      // Book bookmark is not an open. Only report the exact FREE edition
      // after verified bytes have been downloaded, hashed and rendered.
      // An archived/pinned read must never create a new historical pin.
      if (article.dataset.freeS3Saved !== "1" &&
          article.dataset.freeS3Archive !== "1" &&
          /^[A-Za-z0-9_-]{43}$/.test(csrf)) {
        try {
          await fetch("/api/v1/chapters/" + encodeURIComponent(chapter) + "/free-open", {
            method: "POST", credentials: "same-origin", cache: "no-store",
            redirect: "error", keepalive: true,
            headers: { "Content-Type": "application/json",
              "X-CSRF-Token": csrf },
            body: JSON.stringify({ version_id: version, sha256: actual })
          });
        } catch {
          // A failed acknowledgement is not a proven saved edition. Text is
          // already visible; a later successful read may confirm the open.
        }
      }
    } catch {
      failure();
    } finally {
      clearTimeout(deadline);
    }
  })();
})();
