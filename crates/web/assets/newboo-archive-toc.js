// Private archived-reader TOC: canonical keyset seek, never a public book list.
// Included only in the finalized ordinary-deletion archive's self-hosted script.
(() => {
  "use strict";
  const root = document.querySelector("[data-archive-toc]");
  if (!root) return;
  const trigger = root.querySelector("summary");
  const modal = root.querySelector("[data-archive-toc-modal]");
  const scroll = root.querySelector("[data-archive-toc-scroll]");
  const list = root.querySelector(".archive-toc__list");
  const current = root.querySelector(".archive-toc__row--current");
  const close = root.querySelector("[data-archive-toc-close]");
  const backdrop = root.querySelector("[data-archive-toc-backdrop]");
  const status = root.querySelector("[data-archive-toc-status]");
  if (!trigger || !modal || !scroll || !list || !current ||
      !close || !backdrop || !status) return;

  const positiveId = value => typeof value === "string" &&
    /^[1-9][0-9]{0,18}$/.test(value);
  const cursorValid = value => typeof value === "string" &&
    /^[1-9][0-9]{0,9}\.[1-9][0-9]{0,18}$/.test(value);
  const chapterId = root.dataset.archiveTocChapter;
  if (!positiveId(chapterId)) return;
  const endpoint = "/api/v1/me/library/saved/" + chapterId + "/toc/window";
  const paidEnabled = root.dataset.archivePaidAvailable === "1";
  const seen = new Set(
    [...list.querySelectorAll("[data-archive-toc-id]")]
      .map(item => item.dataset.archiveTocId).filter(positiveId)
  );
  let before = cursorValid(root.dataset.archiveTocBefore) ?
    root.dataset.archiveTocBefore : null;
  let after = cursorValid(root.dataset.archiveTocAfter) ?
    root.dataset.archiveTocAfter : null;
  let busy = false;
  let oldOverflow = "";

  const validItem = item => item && typeof item === "object" &&
    positiveId(item.chapter_id) && Number.isSafeInteger(item.chapter_no) &&
    item.chapter_no > 0 && typeof item.title === "string" && item.title.length <= 1000 &&
    (item.saved_free_version_no == null ||
      (Number.isSafeInteger(item.saved_free_version_no) && item.saved_free_version_no > 0)) &&
    (item.purchased == null || (
      positiveId(item.purchased.purchase_id) &&
      Number.isSafeInteger(item.purchased.version_no) && item.purchased.version_no > 0
    )) && (item.saved_free_version_no != null || item.purchased != null);

  function createItem(item) {
    const row = document.createElement("li");
    row.className = "archive-toc__row";
    row.dataset.archiveTocId = item.chapter_id;
    const title = document.createElement("span");
    title.className = "archive-toc__title";
    title.textContent = "Глава " + item.chapter_no + ". " + item.title;
    const choices = document.createElement("span");
    choices.className = "archive-toc__choices";
    if (item.saved_free_version_no != null) {
      const link = document.createElement("a");
      link.href = "/me/library/saved/" + item.chapter_id;
      link.textContent = "Закреплённая бесплатная V" + item.saved_free_version_no;
      choices.append(link);
    }
    if (item.purchased != null) {
      const purchase = item.purchased;
      const link = paidEnabled ? document.createElement("a") :
        document.createElement("span");
      if (paidEnabled) {
        link.href = "/read/paid/" + item.chapter_id +
          "?purchase_id=" + purchase.purchase_id;
      }
      link.textContent = "Купленная V" + purchase.version_no +
        (paidEnabled ? "" : " — недоступно");
      choices.append(link);
    }
    row.append(title, choices);
    return row;
  }

  async function load(side) {
    if (!root.open || busy) return;
    const cursor = side === "before" ? before : after;
    if (!cursorValid(cursor)) return;
    busy = true;
    status.textContent = "Загрузка соседних глав…";
    const oldTop = scroll.scrollTop;
    const oldHeight = scroll.scrollHeight;
    try {
      const params = new URLSearchParams({ [side]: cursor, limit: "60" });
      const reply = await fetch(endpoint + "?" + params, {
        credentials: "same-origin",
        cache: "no-store",
        headers: { Accept: "application/json" }
      });
      if (!reply.ok) throw new Error("Archive TOC access unavailable");
      const page = await reply.json();
      if (!page || !Array.isArray(page.items) || page.items.length > 60 ||
          !page.items.every(validItem) ||
          (page.next_cursor != null && !cursorValid(page.next_cursor))) {
        throw new Error("Invalid archive TOC metadata");
      }
      const fragment = document.createDocumentFragment();
      for (const item of page.items) {
        if (seen.has(item.chapter_id)) continue;
        seen.add(item.chapter_id);
        fragment.append(createItem(item));
      }
      if (side === "before") {
        list.prepend(fragment);
        scroll.scrollTop = oldTop + scroll.scrollHeight - oldHeight;
        before = page.next_cursor;
      } else {
        list.append(fragment);
        after = page.next_cursor;
      }
      status.textContent = "Загружено сохранённых глав: " + seen.size;
    } catch {
      status.textContent = "Не удалось загрузить соседние главы. Повторите прокрутку.";
    } finally {
      busy = false;
    }
  }

  const hide = () => {
    root.open = false;
    trigger.focus();
  };
  root.addEventListener("toggle", () => {
    if (root.open) {
      oldOverflow = document.body.style.overflow;
      document.body.style.overflow = "hidden";
      close.focus();
      // Starting scroll is scoped to the modal, not the reading document.
      const containerTop = scroll.getBoundingClientRect().top;
      const box = current.getBoundingClientRect();
      scroll.scrollTop = Math.max(0, scroll.scrollTop +
        box.top - containerTop - scroll.clientHeight / 2 + box.height / 2);
    } else {
      document.body.style.overflow = oldOverflow;
      trigger.focus();
    }
  });
  scroll.addEventListener("scroll", () => {
    if (!root.open || busy) return;
    if (before && scroll.scrollTop < 220) void load("before");
    else if (after && scroll.scrollHeight - scroll.clientHeight - scroll.scrollTop < 220)
      void load("after");
  }, { passive: true });
  close.addEventListener("click", hide);
  backdrop.addEventListener("click", hide);
  document.addEventListener("keydown", event => {
    if (!root.open) return;
    if (event.key === "Escape") {
      event.preventDefault();
      hide();
    } else if (event.key === "Tab") {
      const focusables = [close, scroll, ...modal.querySelectorAll("a[href]")]
        .filter(node => node.getClientRects().length > 0);
      if (!focusables.length) return;
      const first = focusables[0], last = focusables[focusables.length - 1];
      if (event.shiftKey && (document.activeElement === first ||
          !modal.contains(document.activeElement))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (document.activeElement === last ||
          !modal.contains(document.activeElement))) {
        event.preventDefault();
        first.focus();
      }
    }
  });
})();