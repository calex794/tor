// S011: exact UTF-16 anchors for published free chapter comments.
// Source offsets come from the same canonical reader_blocks parser as E/C.
// This script never stores comments or quotes in localStorage.
(() => {
  "use strict";
  const root = document.querySelector("[data-free-reader]");
  const article = root?.querySelector("#chapter-text");
  const panel = root?.querySelector("[data-chapter-comments]");
  if (!article || !panel) return;
  const raw = (panel.dataset.readerCommentBlocks || "").split(",");
  if (!raw.length || raw.length > 200) return;
  const blocks = raw.map(value => {
    const match = /^(\d+):(\d+)$/.exec(value);
    return match ? { start: Number(match[1]), length: Number(match[2]) } : null;
  });
  if (blocks.some((b, i) => !b || !Number.isSafeInteger(b.start) ||
      !Number.isSafeInteger(b.length) || b.length > 100000 ||
      b.start !== (i ? blocks[i - 1].start + blocks[i - 1].length + 2 : 0))) return;

  const segments = [...article.querySelectorAll("span[data-md-start]")].filter(
    element => element.firstChild?.nodeType === Node.TEXT_NODE &&
      /^\d+$/.test(element.dataset.mdStart || "")
  );
  if (!segments.length) return;

  const input = panel.querySelector("[data-chapter-comment-block]");
  const form = input?.closest("[data-chapter-comment-form]");
  const start = form?.querySelector("[data-chapter-comment-start]");
  const end = form?.querySelector("[data-chapter-comment-end]");
  const quote = form?.querySelector("[data-chapter-comment-quote]");
  const status = panel.querySelector("[data-chapter-comment-anchor-status]");
  const fields = [input, start, end, quote];

  const clearAnchor = (message) => {
    if (!form || fields.some(field => !field)) return;
    fields.forEach(field => {
      field.removeAttribute("name");
      field.value = "";
    });
    if (status) {
      status.textContent = message ||
        "Можно выделить фразу в тексте главы или написать общий комментарий.";
    }
  };
  const recordAnchor = () => {
    if (!form || fields.some(field => !field)) return;
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || sel.rangeCount !== 1) return;
    const range = sel.getRangeAt(0);
    if (!article.contains(range.commonAncestorContainer)) return;
    if (range.startContainer.nodeType !== Node.TEXT_NODE ||
        range.startContainer !== range.endContainer) {
      clearAnchor("Выделение не привязано: выберите фразу внутри одного текстового фрагмента.");
      return;
    }
    const node = range.startContainer;
    const span = node.parentElement?.closest("span[data-md-start]");
    if (!span || !article.contains(span) || span.firstChild !== node) {
      clearAnchor("Выделите фразу непосредственно в тексте главы.");
      return;
    }
    const mdOffset = Number(span.dataset.mdStart);
    const from = mdOffset + range.startOffset;
    const to = mdOffset + range.endOffset;
    const selected = sel.toString();
    const index = blocks.findIndex(b => from >= b.start && to <= b.start + b.length);
    if (!Number.isSafeInteger(mdOffset) || !Number.isSafeInteger(from) ||
        !Number.isSafeInteger(to) || from >= to ||
        index < 0 || selected.length === 0 || [...selected].length > 500) {
      clearAnchor("Выделите не более 500 знаков в пределах одного фрагмента.");
      return;
    }
    input.name = "anchor_block";
    start.name = "anchor_start";
    end.name = "anchor_end";
    quote.name = "anchor_quote";
    input.value = String(index + 1);
    start.value = String(from - blocks[index].start);
    end.value = String(to - blocks[index].start);
    quote.value = selected;
    if (status) {
      const example = selected.length > 90 ? selected.slice(0, 90) + "…" : selected;
      status.textContent = "Комментарий к фрагменту " + (index + 1) +
        ": «" + example + "». Уберите выделение кнопкой ниже, чтобы обсудить всю главу.";
    }
  };
  if (form) {
    document.addEventListener("selectionchange", recordAnchor);
    const reset = document.createElement("button");
    reset.type = "button";
    reset.className = "ui-button ui-button--ghost";
    reset.textContent = "Комментировать всю главу";
    reset.addEventListener("click", () => clearAnchor());
    status?.insertAdjacentElement("afterend", reset);
    clearAnchor();
  }

  // Public anchors survive guest access; there is no session token here.
  const scrollToAnchor = (block, offset) => {
    const location = blocks[block - 1];
    if (!location || offset < 0 || offset > location.length) return false;
    const sourceOffset = location.start + offset;
    let span = null;
    for (const candidate of segments) {
      if (Number(candidate.dataset.mdStart) > sourceOffset) break;
      span = candidate;
    }
    if (!span) span = segments[0];
    const node = span.firstChild;
    if (!node) return false;
    const localOffset = Math.max(
      0, Math.min(node.length, sourceOffset - Number(span.dataset.mdStart))
    );
    const range = document.createRange();
    range.setStart(node, localOffset);
    range.collapse(true);
    const rect = range.getBoundingClientRect();
    const target = Math.max(0, window.scrollY + rect.top - window.innerHeight * 0.25);
    window.scrollTo({ top: target, behavior: "instant" });
    span.classList.add("reader-chapter-comments__source-highlight");
    window.setTimeout(() => span.classList.remove("reader-chapter-comments__source-highlight"), 1800);
    return true;
  };
  panel.addEventListener("click", event => {
    const link = event.target.closest("[data-chapter-comment-goto]");
    if (!link) return;
    const block = Number(link.dataset.anchorBlock);
    const offset = Number(link.dataset.anchorStart);
    if (!Number.isSafeInteger(block) || !Number.isSafeInteger(offset)) return;
    if (scrollToAnchor(block, offset)) {
      event.preventDefault();
      history.replaceState(null, "", "#chapter-text");
    }
  });
})();
