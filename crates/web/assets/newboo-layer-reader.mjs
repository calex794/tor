// D-05/H: Layer CURRENT edition, using the same verified ciphertext +
// browser WebCrypto implementation as pinned direct purchases. No purchase
// progress, entitlement cache, signed URL or DEK is persisted.
import { decryptPaidGrant, readerBlocks } from "/assets/newboo-paid-reader.mjs";

function requireValue(ok) {
  if (!ok) throw new Error("Не удалось безопасно открыть главу Layer.");
}

export async function startLayerReader(root) {
  const chapter = root.dataset.chapterId;
  const csrf = root.dataset.csrfToken;
  const s3Origin = root.dataset.s3Origin;
  const status = root.querySelector("[data-layer-status]");
  const article = root.querySelector("[data-layer-content]");
  requireValue(
    /^[1-9][0-9]{0,18}$/.test(chapter || "") &&
    /^[A-Za-z0-9_-]{43}$/.test(csrf || "") &&
    typeof s3Origin === "string" && s3Origin.length > 0 &&
    status && article
  );

  // The browser chooses only the entitlement SOURCE, not the edition,
  // version ID, storage object or private signing host.
  const reply = await fetch(
    "/api/v1/reader/chapters/" + chapter + "/delivery-grants",
    {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-CSRF-Token": csrf },
      credentials: "same-origin",
      redirect: "error",
      cache: "no-store",
      referrerPolicy: "no-referrer",
      body: JSON.stringify({ source: "layer" }),
    }
  );
  requireValue(reply.status === 200 && reply.ok);
  const grant = await reply.json();
  const { versionId, text } = await decryptPaidGrant(grant, chapter, s3Origin);

  // Render text nodes only: never interpret decrypted Markdown as HTML.
  // Purchased progress/completion APIs require a purchase_id and MUST NOT
  // be reused for a Layer-only subscription.
  const sections = document.createDocumentFragment();
  readerBlocks(text).forEach((block, index) => {
    const section = document.createElement("section");
    section.className = "reader-piece";
    section.id = "layer-p-" + (index + 1);
    const paragraph = document.createElement("p");
    paragraph.className = "reader-piece__text reader-markdown";
    paragraph.textContent = block;
    section.append(paragraph);
    sections.append(section);
  });
  article.replaceChildren(sections);
  status.textContent = "Текущая редакция " + versionId + " · Защищённая загрузка завершена";
}

if (typeof document !== "undefined") {
  const root = document.querySelector("[data-layer-chapter-reader]");
  if (root) {
    startLayerReader(root).catch(() => {
      const status = root.querySelector("[data-layer-status]");
      if (status) status.textContent = "Доступ к главе недоступен. Проверьте подписку.";
      const article = root.querySelector("[data-layer-content]");
      if (article) article.replaceChildren();
    });
  }
}


// D-05/H: Layer TOC is navigation metadata, never an alternative paid grant.
function installLayerToc(root) {
  const detail = root.querySelector("[data-layer-toc]");
  const modal = detail?.querySelector("[data-layer-toc-modal]");
  const scroll = detail?.querySelector("[data-layer-toc-scroll]");
  const list = detail?.querySelector("[data-layer-toc-list]");
  const close = detail?.querySelector("[data-layer-toc-close]");
  const backdrop = detail?.querySelector("[data-layer-toc-backdrop]");
  const trigger = detail?.querySelector("summary");
  const status = detail?.querySelector("[data-layer-toc-status]");
  const current = list?.querySelector('[data-toc-state="current"]');
  if (!detail || !modal || !scroll || !list || !close ||
      !backdrop || !trigger || !status || !current) return;

  const id = root.dataset.chapterId;
  const validId = value => typeof value === "string" &&
    /^[1-9][0-9]{0,18}$/.test(value);
  const validCursor = value => typeof value === "string" &&
    /^[1-9][0-9]{0,9}\.[1-9][0-9]{0,18}$/.test(value);
  if (!validId(id)) return;
  const endpoint = "/api/v1/reader/layer/" + id + "/toc/window";
  const initial = current.dataset.tocNo + "." + current.dataset.tocId;
  if (!validCursor(initial)) return;
  const seen = new Set([current.dataset.tocId]);
  let before = initial, after = initial;
  let loading = false, opened = false;
  let previousOverflow = "";

  function safeItem(item) {
    return item && validId(item.chapter_id) &&
      Number.isSafeInteger(item.chapter_no) && item.chapter_no > 0 &&
      typeof item.title === "string" && item.title.length <= 1000 &&
      (item.access_level === "layer" || item.access_level === "free");
  }
  function render(item) {
    const row = document.createElement("li");
    row.className = "reader-paid__toc-row";
    row.classList.add(item.access_level === "layer" ?
      "reader-paid__toc-row--owned" : "reader-paid__toc-row--free");
    row.dataset.tocId = item.chapter_id;
    row.dataset.tocNo = String(item.chapter_no);
    row.dataset.tocState = item.access_level;
    const link = document.createElement("a");
    link.href = (item.access_level === "layer" ? "/read/layer/" : "/reader/") +
      item.chapter_id;
    const title = document.createElement("span");
    title.className = "reader-paid__toc-title";
    title.textContent = "Глава " + item.chapter_no + ". " + item.title;
    const badge = document.createElement("span");
    badge.className = "reader-paid__toc-badge reader-paid__toc-badge--" +
      (item.access_level === "layer" ? "owned" : "free");
    badge.textContent = item.access_level === "layer" ? "Layer" : "Бесплатно";
    link.append(title, badge);
    row.append(link);
    return row;
  }
  async function load(side) {
    if (!detail.open || loading) return;
    const cursor = side === "before" ? before : after;
    if (!validCursor(cursor)) return;
    loading = true;
    status.textContent = "Загрузка глав…";
    const top = scroll.scrollTop, height = scroll.scrollHeight;
    try {
      const params = new URLSearchParams({ [side]: cursor, limit: "36" });
      const reply = await fetch(endpoint + "?" + params, {
        credentials: "same-origin", cache: "no-store",
        headers: { Accept: "application/json" }
      });
      if (!reply.ok) throw new Error("Layer access unavailable");
      const page = await reply.json();
      if (!page || !Array.isArray(page.items) || page.items.length > 36 ||
          !page.items.every(safeItem) ||
          (page.next_cursor != null && !validCursor(page.next_cursor))) {
        throw new Error("Invalid Layer TOC response");
      }
      const fragment = document.createDocumentFragment();
      for (const item of page.items) {
        if (seen.has(item.chapter_id)) continue;
        seen.add(item.chapter_id);
        fragment.append(render(item));
      }
      if (side === "before") {
        list.prepend(fragment);
        scroll.scrollTop = top + scroll.scrollHeight - height;
        before = page.next_cursor;
      } else {
        list.append(fragment);
        after = page.next_cursor;
      }
      status.textContent = "Доступно в оглавлении: " + seen.size + " глав";
    } catch {
      status.textContent = "Не удалось загрузить соседние главы. Повторите прокрутку.";
    } finally {
      loading = false;
    }
  }
  const hide = () => { detail.open = false; trigger.focus(); };
  close.addEventListener("click", hide);
  backdrop.addEventListener("click", hide);
  detail.addEventListener("toggle", () => {
    if (detail.open) {
      previousOverflow = document.body.style.overflow;
      document.body.style.overflow = "hidden";
      close.focus();
      current.scrollIntoView({ block: "center", inline: "nearest" });
      if (!opened) {
        opened = true;
        void (async () => { await load("before"); await load("after"); })();
      }
    } else {
      document.body.style.overflow = previousOverflow;
      trigger.focus();
    }
  });
  scroll.addEventListener("scroll", () => {
    if (!detail.open || loading) return;
    if (before && scroll.scrollTop < 200) void load("before");
    else if (after && scroll.scrollHeight - scroll.clientHeight - scroll.scrollTop < 200)
      void load("after");
  }, { passive: true });
  document.addEventListener("keydown", event => {
    if (!detail.open) return;
    if (event.key === "Escape") {
      event.preventDefault(); hide();
    } else if (event.key === "Tab") {
      const nodes = [close, scroll, ...modal.querySelectorAll("a[href]")].filter(
        node => node.getClientRects().length
      );
      const first = nodes[0], last = nodes[nodes.length - 1];
      if (!first || !last) return;
      if (event.shiftKey && (document.activeElement === first ||
          !modal.contains(document.activeElement))) {
        event.preventDefault(); last.focus();
      } else if (!event.shiftKey && (document.activeElement === last ||
          !modal.contains(document.activeElement))) {
        event.preventDefault(); first.focus();
      }
    }
  });
}

if (typeof document !== "undefined") {
  const root = document.querySelector("[data-layer-chapter-reader]");
  if (root) installLayerToc(root);
}
