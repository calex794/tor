// NEwBOO-style table of contents. TOR exposes only published metadata,
// keyset-paginated in pages of 100; links never imply a paid entitlement.
(() => {
  "use strict";
  const root = document.querySelector("[data-free-reader] [data-free-toc]");
  if (!root) return;
  const bookId = root.dataset.freeTocBook;
  const currentId = root.dataset.freeTocChapter;
  const list = root.querySelector("[data-free-toc-list]");
  const status = root.querySelector("[data-free-toc-status]");
  const more = root.querySelector("[data-free-toc-more]");
  if (!/^[1-9]\d*$/.test(bookId || "") || !/^[1-9]\d*$/.test(currentId || "") ||
      !list || !status || !more) return;

  let loading = false;
  let loaded = false;
  let cursor = null;
  let count = 0;
  const used = new Set();
  const validCursor = value => typeof value === "string" &&
    /^[1-9]\d{0,9}\.[1-9]\d{0,18}$/.test(value);
  const load = async () => {
    if (loading || (loaded && !cursor)) return;
    loading = true;
    more.hidden = true;
    status.textContent = "Загружаем оглавление…";
    const after = cursor;
    try {
      const url = "/api/v1/books/" + bookId + "/toc?limit=100" +
        (after ? "&after=" + encodeURIComponent(after) : "");
      const response = await fetch(url, {
        credentials: "same-origin", cache: "no-store",
        headers: { Accept: "application/json" }
      });
      if (!response.ok) throw new Error("TOC HTTP " + response.status);
      const payload = await response.json();
      if (!payload || !Array.isArray(payload.items) ||
          payload.items.length > 100 ||
          payload.items.some(item => !item || typeof item !== "object" ||
            !/^[1-9]\d*$/.test(item.chapter_id || "") ||
            !Number.isSafeInteger(item.chapter_no) || item.chapter_no <= 0 ||
            typeof item.title !== "string" || item.title.length > 1000 ||
            !["free", "paid"].includes(item.access_level))) {
        throw new Error("Unexpected TOC data");
      }
      const next = payload.next_cursor;
      if (next != null && (!validCursor(next) || next === after ||
                           used.has(next) || !payload.items.length)) {
        throw new Error("Invalid TOC cursor");
      }
      const fragment = document.createDocumentFragment();
      for (const entry of payload.items) {
        const row = document.createElement("li");
        row.className = "reader-free-toc__row";
        const label = "Глава " + entry.chapter_no + ". " + entry.title;
        if (entry.chapter_id === currentId) {
          row.classList.add("reader-free-toc__row--current");
          const now = document.createElement("span");
          now.setAttribute("aria-current", "page");
          now.textContent = label + " · Читаю";
          row.appendChild(now);
        } else {
          const anchor = document.createElement("a");
          anchor.href = "/reader/" + entry.chapter_id;
          anchor.textContent = label;
          row.appendChild(anchor);
        }
        const badge = document.createElement("small");
        badge.textContent = entry.access_level === "paid" ?
          "Платная · нужен доступ" : "Бесплатно";
        row.appendChild(badge);
        fragment.appendChild(row);
      }
      list.appendChild(fragment);
      count += payload.items.length;
      loaded = true;
      if (after) used.add(after);
      cursor = next ?? null;
      status.textContent = count ? "Показано глав: " + count : "Опубликованных глав пока нет.";
      more.hidden = !cursor;
      more.textContent = "Показать ещё";
    } catch {
      status.textContent = "Не удалось загрузить оглавление. Попробуйте ещё раз.";
      more.hidden = false;
      more.textContent = "Повторить загрузку";
    } finally {
      loading = false;
    }
  };
  root.addEventListener("toggle", () => {
    if (root.open && !loaded) void load();
  });
  more.addEventListener("click", () => { void load(); });
})();
