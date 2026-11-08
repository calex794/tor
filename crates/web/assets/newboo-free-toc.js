// C-03/H: reader-centered public chapter metadata, with bidirectional loading.
// The TOC contains no chapter body and never grants access to paid editions.
(() => {
  "use strict";
  const root = document.querySelector("[data-free-reader] [data-free-toc]");
  if (!root) return;
  const bookId = root.dataset.freeTocBook;
  const currentId = root.dataset.freeTocChapter;
  const list = root.querySelector("[data-free-toc-list]");
  const viewport = root.querySelector("[data-free-toc-scroll]");
  const status = root.querySelector("[data-free-toc-status]");
  const total = root.querySelector("[data-free-toc-total]");
  const close = root.querySelector("[data-free-toc-close]");
  const summary = root.querySelector("summary");
  const backdrop = root.querySelector("[data-free-toc-backdrop]");
  const modal = root.querySelector("[data-free-toc-modal]");
  if (!/^[1-9]\d*$/.test(bookId || "") ||
      !/^[1-9]\d*$/.test(currentId || "") ||
      !list || !viewport || !status || !total || !close ||
      !summary || !backdrop || !modal) return;

  let busy = false;
  let initialized = false;
  let before = null;
  let after = null;
  let oldBodyOverflow = "";
  const seen = new Set();
  const validCursor = value => value == null || (
    typeof value === "string" && /^[1-9]\d{0,9}\.[1-9]\d{0,18}$/.test(value)
  );
  const api = "/api/v1/books/" + bookId + "/toc/window";
  const makeRow = entry => {
    const row = document.createElement("li");
    row.className = "reader-free-toc__row";
    row.dataset.chapterId = entry.chapter_id;
    const label = "Глава " + entry.chapter_no + ". " + entry.title;
    const node = entry.chapter_id === currentId
      ? document.createElement("span") : document.createElement("a");
    node.textContent = label;
    if (entry.chapter_id === currentId) {
      row.classList.add("reader-free-toc__row--current");
      node.setAttribute("aria-current", "page");
      const badge = document.createElement("small");
      badge.textContent = "Читается";
      row.append(node, badge);
    } else {
      node.href = "/reader/" + entry.chapter_id;
      const badge = document.createElement("small");
      badge.textContent = entry.access_level === "paid" ? "Платная · нужен доступ" : "Бесплатно";
      row.append(node, badge);
    }
    return row;
  };
  const validItem = entry => entry && typeof entry === "object" &&
    typeof entry.chapter_id === "string" &&
    /^[1-9]\d*$/.test(entry.chapter_id) &&
    Number.isSafeInteger(entry.chapter_no) && entry.chapter_no > 0 &&
    typeof entry.title === "string" && entry.title.length <= 1000 &&
    ["free", "paid"].includes(entry.access_level);

  const load = async (direction) => {
    if (busy || !root.open) return;
    const cursor = direction === "before" ? before : after;
    if (direction !== "around" && !cursor) return;
    busy = true;
    status.textContent = "Загрузка глав…";
    const params = new URLSearchParams({ limit: direction === "around" ? "99" : "60" });
    if (direction === "around") params.set("around_chapter_id", currentId);
    else params.set(direction, cursor);
    const oldHeight = viewport.scrollHeight;
    const oldTop = viewport.scrollTop;
    try {
      const response = await fetch(api + "?" + params, {
        headers: { Accept: "application/json" },
        credentials: "same-origin",
        cache: "no-store"
      });
      if (!response.ok) throw new Error("TOC HTTP " + response.status);
      const payload = await response.json();
      if (!payload || !Array.isArray(payload.items) || payload.items.length > 100 ||
          !payload.items.every(validItem) ||
          !validCursor(payload.before_cursor) || !validCursor(payload.after_cursor) ||
          (payload.total_chapters != null &&
            (!Number.isSafeInteger(payload.total_chapters) || payload.total_chapters < 0))) {
        throw new Error("Invalid TOC response");
      }
      const fragment = document.createDocumentFragment();
      const newEntries = payload.items.filter(entry => !seen.has(entry.chapter_id));
      for (const entry of newEntries) {
        seen.add(entry.chapter_id);
        fragment.append(makeRow(entry));
      }
      if (direction === "before") {
        list.prepend(fragment);
        viewport.scrollTop = oldTop + viewport.scrollHeight - oldHeight;
        before = payload.before_cursor;
      } else {
        list.append(fragment);
        if (direction === "around") {
          before = payload.before_cursor;
          const reading = [...list.children].find(
            row => row.dataset.chapterId === currentId
          );
          if (!reading) throw new Error("Current chapter is not visible");
          reading.scrollIntoView({ block: "center", inline: "nearest" });
          if (payload.total_chapters != null) {
            total.textContent = "Всего глав: " + payload.total_chapters;
          }
          initialized = true;
        }
        after = payload.after_cursor;
      }
      status.textContent = "Загружено глав: " + seen.size;
    } catch {
      status.textContent = "Не удалось загрузить главы. Прокрутите ещё раз.";
    } finally {
      busy = false;
    }
  };

  const onScroll = () => {
    if (!initialized || busy || !root.open) return;
    if (viewport.scrollTop < 240 && before) {
      void load("before");
    } else if (viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop < 240 && after) {
      void load("after");
    }
  };
  const hide = () => { root.open = false; summary.focus(); };
  const focusable = () => [close, ...modal.querySelectorAll("a[href]")];
  root.addEventListener("toggle", () => {
    if (root.open) {
      oldBodyOverflow = document.body.style.overflow;
      document.body.style.overflow = "hidden";
      close.focus();
      if (!initialized) void load("around");
      else {
        const reading = [...list.children].find(row => row.dataset.chapterId === currentId);
        reading?.scrollIntoView({ block: "center", inline: "nearest" });
      }
    } else {
      document.body.style.overflow = oldBodyOverflow;
      summary.focus();
    }
  });
  viewport.addEventListener("scroll", onScroll, { passive: true });
  close.addEventListener("click", hide);
  backdrop.addEventListener("click", hide);
  document.addEventListener("keydown", event => {
    if (!root.open) return;
    if (event.key === "Escape") {
      event.preventDefault();
      hide();
    } else if (event.key === "Tab") {
      const nodes = focusable().filter(node => node.getClientRects().length);
      if (!nodes.length) return;
      const first = nodes[0], last = nodes[nodes.length - 1];
      if (event.shiftKey && (document.activeElement === first ||
          !modal.contains(document.activeElement))) {
        event.preventDefault(); last.focus();
      } else if (!event.shiftKey && (document.activeElement === last ||
          !modal.contains(document.activeElement))) {
        event.preventDefault(); first.focus();
      }
    }
  });
})();
