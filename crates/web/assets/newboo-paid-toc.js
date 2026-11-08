// D-scoped paid TOC: server-confirmed ownership, keyset paging, no paid body.
(() => {
  "use strict";
  const reader = document.querySelector("[data-paid-chapter-reader]");
  if (!reader) return;
  const chapterId = reader.dataset.chapterId;
  const purchaseId = reader.dataset.purchaseId;
  const isId = value => typeof value === "string" && /^[1-9]\d*$/.test(value);
  if (!isId(chapterId) || !isId(purchaseId)) return;
  const roots = [...reader.querySelectorAll("[data-paid-toc], [data-paid-toc-bottom]")];
  const endpoint = "/api/v1/reader/chapters/" + chapterId + "/toc/window";
  const validCursor = value => typeof value === "string" &&
    /^[1-9]\d{0,9}\.[1-9]\d{0,18}$/.test(value);
  let originalBodyOverflow = null;

  for (const root of roots) {
    const trigger = root.querySelector("summary");
    const dialog = root.querySelector("[data-paid-toc-modal]");
    const viewport = root.querySelector("[data-paid-toc-scroll]");
    const list = root.querySelector(".reader-paid__toc-list");
    const close = root.querySelector("[data-paid-toc-close]");
    const backdrop = root.querySelector("[data-paid-toc-backdrop]");
    const current = root.querySelector('[data-toc-state="current"]');
    const progress = root.querySelector(".reader-paid__toc-more");
    if (!trigger || !dialog || !viewport || !list || !close || !backdrop || !current) continue;

    let busy = false;
    const rows = [...list.children];
    const seen = new Set(rows.map(row => row.dataset.tocId).filter(isId));
    const cursorOf = row => {
      if (!row || !isId(row.dataset.tocId) ||
          !/^[1-9]\d*$/.test(row.dataset.tocNo || "")) return null;
      const cursor = row.dataset.tocNo + "." + row.dataset.tocId;
      return validCursor(cursor) ? cursor : null;
    };
    // The server deliberately returns the 100 closest D-scoped chapters.
    // If it truncated that selection, load real neighbours on either edge.
    let before = progress ? cursorOf(rows[0]) : null;
    let after = progress ? cursorOf(rows[rows.length - 1]) : null;

    const validItem = x => x && typeof x === "object" &&
      isId(x.chapter_id) && Number.isSafeInteger(x.chapter_no) &&
      x.chapter_no > 0 && typeof x.title === "string" && x.title.length <= 1000 &&
      ["purchased", "free", "locked"].includes(x.access_level) &&
      (x.access_level !== "purchased" || isId(x.purchase_id));

    const render = entry => {
      const row = document.createElement("li");
      row.className = "reader-paid__toc-row";
      row.dataset.tocId = entry.chapter_id;
      row.dataset.tocNo = String(entry.chapter_no);
      let link;
      let badgeText;
      if (entry.access_level === "purchased") {
        row.classList.add("reader-paid__toc-row--owned");
        row.dataset.tocState = "owned";
        link = document.createElement("a");
        link.href = "/read/paid/" + entry.chapter_id + "?purchase_id=" + entry.purchase_id;
        link.dataset.paidTocLink = "";
        badgeText = "Куплено · редакция " + entry.version_no;
      } else if (entry.access_level === "free") {
        row.classList.add("reader-paid__toc-row--free");
        row.dataset.tocState = "free";
        link = document.createElement("a");
        link.href = "/reader/" + entry.chapter_id;
        link.dataset.freeTocLink = "";
        badgeText = "Бесплатно";
      } else {
        row.classList.add("reader-paid__toc-row--locked");
        row.dataset.tocState = "locked";
        link = document.createElement("span");
        link.className = "reader-paid__toc-locked";
        link.setAttribute("aria-disabled", "true");
        badgeText = "Не куплено";
      }
      const title = document.createElement("span");
      title.className = "reader-paid__toc-title";
      title.textContent = "Глава " + entry.chapter_no + ". " + entry.title;
      const badge = document.createElement("span");
      badge.className = "reader-paid__toc-badge reader-paid__toc-badge--" +
        (entry.access_level === "purchased" ? "owned" : entry.access_level);
      badge.textContent = badgeText;
      link.append(title, badge);
      row.append(link);
      return row;
    };

    const load = async side => {
      if (!root.open || busy) return;
      const cursor = side === "before" ? before : after;
      if (!validCursor(cursor)) return;
      busy = true;
      const params = new URLSearchParams({
        purchase_id: purchaseId,
        limit: "60",
        [side]: cursor
      });
      const oldTop = viewport.scrollTop;
      const oldHeight = viewport.scrollHeight;
      try {
        const response = await fetch(endpoint + "?" + params, {
          credentials: "same-origin",
          cache: "no-store",
          headers: { Accept: "application/json" }
        });
        if (!response.ok) throw new Error("TOC page " + response.status);
        const payload = await response.json();
        if (!payload || !Array.isArray(payload.items) || payload.items.length > 60 ||
            !payload.items.every(validItem) ||
            (payload.next_cursor != null && !validCursor(payload.next_cursor))) {
          throw new Error("Invalid paid TOC metadata");
        }
        const fragment = document.createDocumentFragment();
        for (const entry of payload.items) {
          if (seen.has(entry.chapter_id)) continue;
          seen.add(entry.chapter_id);
          fragment.append(render(entry));
        }
        if (side === "before") {
          list.prepend(fragment);
          viewport.scrollTop = oldTop + viewport.scrollHeight - oldHeight;
          before = payload.next_cursor;
        } else {
          list.append(fragment);
          after = payload.next_cursor;
        }
        if (progress?.firstChild) {
          progress.firstChild.nodeValue = "Загружено глав: " + seen.size + ". Другие покупки — в ";
        }
      } catch {
        if (progress?.firstChild) progress.firstChild.nodeValue =
          "Не удалось загрузить соседние главы. Прокрутите ещё раз. Другие покупки — в ";
      } finally {
        busy = false;
      }
    };

    viewport.addEventListener("scroll", () => {
      if (!root.open || busy) return;
      if (viewport.scrollTop < 220 && before) void load("before");
      else if (viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop < 220 && after)
        void load("after");
    }, { passive: true });

    const hide = () => { root.open = false; };
    root.addEventListener("toggle", () => {
      if (root.open) {
        if (originalBodyOverflow === null) originalBodyOverflow = document.body.style.overflow;
        for (const other of roots) if (other !== root) other.open = false;
        document.body.style.overflow = "hidden";
        close.focus();
        current.scrollIntoView({ block: "center", inline: "nearest" });
      } else if (!roots.some(node => node.open)) {
        document.body.style.overflow = originalBodyOverflow ?? "";
        originalBodyOverflow = null;
        trigger.focus();
      }
    });
    close.addEventListener("click", hide);
    backdrop.addEventListener("click", hide);
    document.addEventListener("keydown", event => {
      if (!root.open) return;
      if (event.key === "Escape") {
        event.preventDefault(); hide();
      } else if (event.key === "Tab") {
        const nodes = [close, viewport, ...dialog.querySelectorAll("a[href]")].filter(
          node => node.getClientRects().length
        );
        if (!nodes.length) return;
        const first = nodes[0], last = nodes[nodes.length - 1];
        if (event.shiftKey && (document.activeElement === first ||
            !dialog.contains(document.activeElement))) {
          event.preventDefault(); last.focus();
        } else if (!event.shiftKey && (document.activeElement === last ||
            !dialog.contains(document.activeElement))) {
          event.preventDefault(); first.focus();
        }
      }
    });
  }
})();
