// H-05 / S006: use the existing actor/CAS-protected free progress API.
// Raw Markdown source offsets are supplied by the B/C renderer, never guessed
// from rendered paragraph counts or written to browser storage.
(() => {
  "use strict";

  const article = document.querySelector("[data-free-reader] [data-free-progress-chapter]");
  if (!article) return; // Anonymous pages must not even attempt a progress read.
  const chapterId = article.dataset.freeProgressChapter;
  const versionId = article.dataset.freeProgressVersion;
  const csrf = article.dataset.freeProgressCsrf;
  if (!/^[1-9]\d*$/.test(chapterId || "") ||
      !/^[1-9]\d*$/.test(versionId || "") ||
      !/^[A-Za-z0-9_-]{43}$/.test(csrf || "")) return;

  const rawBlocks = (article.dataset.freeProgressBlocks || "").split(",");
  if (!rawBlocks.length || rawBlocks.length > 200) return;
  const blocks = rawBlocks.map(entry => {
    const match = /^(\d+):(\d+)$/.exec(entry);
    return match ? { start: Number(match[1]), length: Number(match[2]) } : null;
  });
  if (blocks.some((b, i) => !b || !Number.isSafeInteger(b.start) ||
      !Number.isSafeInteger(b.length) || b.length > 100000 ||
      b.start !== (i === 0 ? 0 : blocks[i - 1].start + blocks[i - 1].length + 2))) return;

  const spans = [...article.querySelectorAll("span[data-md-start]")].filter(
    el => el.firstChild?.nodeType === Node.TEXT_NODE &&
      /^\d+$/.test(el.dataset.mdStart || "")
  );
  if (!spans.length) return;
  const notice = document.querySelector("[data-free-progress-notice]");
  const endpoint = "/api/v1/me/progress/free";
  const readUrl = endpoint + "?chapter_id=" + encodeURIComponent(chapterId);
  let revision = 0;
  let enabled = false;
  let blocked = false;
  let movedBeforeRead = false;
  let hasScrolled = false;
  let restoring = false;
  let saving = false;
  let saveTimer = 0;
  let saved = null;

  const announce = message => {
    if (!notice) return;
    notice.hidden = false;
    notice.textContent = message;
  };
  const sourcePosition = span => {
    const text = span.firstChild;
    const sourceStart = Number(span.dataset.mdStart);
    if (!text || !text.length) return sourceStart;
    const anchor = window.innerHeight * 0.32;
    const range = document.createRange();
    let low = 0, high = text.length;
    while (low < high) {
      const mid = Math.ceil((low + high) / 2);
      range.setStart(text, mid);
      range.collapse(true);
      const rect = range.getBoundingClientRect();
      if (rect.top <= anchor) low = mid;
      else high = mid - 1;
    }
    return sourceStart + low;
  };
  const currentPlace = () => {
    const anchor = window.innerHeight * 0.32;
    let low = 0, high = spans.length - 1;
    while (low < high) {
      const mid = Math.ceil((low + high) / 2);
      if (spans[mid].getBoundingClientRect().top <= anchor) low = mid;
      else high = mid - 1;
    }
    const sourceOffset = sourcePosition(spans[low]);
    let i = 0, j = blocks.length - 1;
    while (i < j) {
      const mid = Math.ceil((i + j) / 2);
      if (blocks[mid].start <= sourceOffset) i = mid;
      else j = mid - 1;
    }
    // Never mint ChapterCompleted by a scrolling/autosave event.
    // Completion continues to require the explicit user action in /read/{id}.
    const offset = Math.max(0, Math.min(
      sourceOffset - blocks[i].start, Math.max(0, blocks[i].length - 1)
    ));
    return { block_id: i + 1, char_offset: offset };
  };
  const validPlace = value => value &&
    Number.isInteger(value.block_id) && value.block_id >= 1 &&
    value.block_id <= blocks.length &&
    Number.isInteger(value.char_offset) && value.char_offset >= 0 &&
    value.char_offset <= blocks[value.block_id - 1].length;

  const restore = place => {
    const sourceOffset = blocks[place.block_id - 1].start + place.char_offset;
    let low = 0, high = spans.length - 1;
    while (low < high) {
      const mid = Math.ceil((low + high) / 2);
      if (Number(spans[mid].dataset.mdStart) <= sourceOffset) low = mid;
      else high = mid - 1;
    }
    const span = spans[low];
    const node = span.firstChild;
    if (!node) return;
    const local = Math.min(node.length,
      Math.max(0, sourceOffset - Number(span.dataset.mdStart)));
    const range = document.createRange();
    range.setStart(node, local);
    range.collapse(true);
    const rect = range.getBoundingClientRect();
    restoring = true;
    window.scrollTo({
      top: window.scrollY + rect.top - window.innerHeight * 0.32,
      behavior: "instant"
    });
    article.dataset.freeProgressRestored = "1";
    window.requestAnimationFrame(() => {
      window.requestAnimationFrame(() => { restoring = false; });
    });
  };
  const samePlace = (a, b) => a && b &&
    a.block_id === b.block_id && a.char_offset === b.char_offset;

  const save = async () => {
    // A GET and a pagehide alone are not evidence of having read this chapter.
    if (!enabled || !hasScrolled || blocked || saving) return;
    const place = currentPlace();
    if (!validPlace(place) || samePlace(place, saved)) return;
    saving = true;
    try {
      const response = await fetch(endpoint, {
        method: "PUT", credentials: "same-origin", cache: "no-store",
        keepalive: true,
        headers: { "Content-Type": "application/json", "Accept": "application/json",
          "X-CSRF-Token": csrf },
        body: JSON.stringify({
          chapter_id: chapterId, version_id: versionId,
          block_id: place.block_id, char_offset: place.char_offset,
          expected_revision: revision
        })
      });
      if (response.ok) {
        const result = await response.json();
        if (result.chapter_id !== chapterId ||
            result.version_id !== versionId ||
            !Number.isSafeInteger(result.revision) ||
            result.revision <= revision) throw new Error("invalid revision");
        revision = result.revision;
        saved = place;
      } else {
        blocked = true; // A stale device must not blindly overwrite a newer cursor.
        announce(response.status === 409
          ? "Место чтения изменилось в другой вкладке. Обновите страницу."
          : "Не удалось сохранить место чтения. Обновите страницу и проверьте вход.");
      }
    } catch {
      blocked = true;
      announce("Место чтения не сохранено. Проверьте соединение и обновите страницу.");
    } finally {
      saving = false;
    }
  };
  const onScroll = () => {
    if (!enabled) { movedBeforeRead = true; return; }
    if (blocked || restoring) return;
    hasScrolled = true;
    window.clearTimeout(saveTimer);
    saveTimer = window.setTimeout(() => { void save(); }, 1200);
  };
  window.addEventListener("scroll", onScroll, { passive: true });
  document.addEventListener("visibilitychange", () => {
    if (document.hidden && enabled) {
      window.clearTimeout(saveTimer);
      void save();
    }
  });
  window.addEventListener("pagehide", () => {
    if (enabled) {
      window.clearTimeout(saveTimer);
      void save();
    }
  });
  (async () => {
    try {
      const response = await fetch(readUrl, {
        method: "GET", credentials: "same-origin", cache: "no-store",
        headers: { "Accept": "application/json" }
      });
      if (!response.ok) throw new Error("progress read status");
      const result = await response.json();
      if (result.chapter_id !== chapterId ||
          result.version_id !== versionId ||
          !Number.isSafeInteger(result.revision) || result.revision < 0) {
        throw new Error("unexpected published version");
      }
      revision = result.revision;
      const place = result.block_id == null ? null :
        { block_id: result.block_id, char_offset: result.char_offset };
      if (place && !validPlace(place)) throw new Error("invalid saved cursor");
      saved = place;
      enabled = true;
      article.dataset.freeProgressReady = "1";
      if (place && !window.location.hash && !movedBeforeRead &&
          window.scrollY < 16) {
        window.requestAnimationFrame(() => restore(place));
      }
    } catch {
      blocked = true;
      announce("Не удалось загрузить место чтения. Страница остаётся доступной без сохранения.");
    }
  })();
})();
