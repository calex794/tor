// S047: map a one-span selection from public Markdown's global UTF-16
// source offset into the existing canonical block-relative E-12 contract.
// The PostgreSQL handler verifies the immutable edition and quote again.
(() => {
  "use strict";
  const root = document.querySelector("[data-free-reader]");
  const article = root?.querySelector("[data-free-progress-chapter]");
  const panel = root?.querySelector("[data-free-issues]");
  const form = panel?.querySelector("[data-free-issue-form]");
  if (!article || !form) return;
  const raw = (article.dataset.freeProgressBlocks || "").split(",");
  if (!raw.length || raw.length > 200) return;
  const blocks = raw.map(part => {
    const match = /^(\d+):(\d+)$/.exec(part);
    return match ? { start: Number(match[1]), length: Number(match[2]) } : null;
  });
  if (blocks.some((b, i) => !b || !Number.isSafeInteger(b.start) ||
      !Number.isSafeInteger(b.length) || b.length > 100000 ||
      b.start !== (i ? blocks[i - 1].start + blocks[i - 1].length + 2 : 0))) return;
  const quote = form.querySelector("[data-free-issue-quote]");
  const block = form.querySelector("[data-free-issue-block]");
  const startInput = form.querySelector("[data-free-issue-start]");
  const endInput = form.querySelector("[data-free-issue-end]");
  const suggestion = form.querySelector("[data-free-issue-suggestion]");
  const send = form.querySelector("[data-free-issue-submit]");
  const status = panel.querySelector("[data-free-issues-status]");
  if (!quote || !block || !startInput || !endInput || !suggestion || !send || !status) return;

  let valid = false;
  const updateButton = () => {
    send.disabled = !valid || !suggestion.value.trim();
  };
  suggestion.addEventListener("input", updateButton);
  document.addEventListener("selectionchange", () => {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || sel.rangeCount !== 1) return;
    const range = sel.getRangeAt(0);
    if (!article.contains(range.commonAncestorContainer)) return;
    const startNode = range.startContainer;
    const endNode = range.endContainer;
    if (startNode.nodeType !== Node.TEXT_NODE || startNode !== endNode) {
      status.textContent = "Выделите текст без разрыва и форматирования.";
      return;
    }
    const span = startNode.parentElement?.closest("span[data-md-start]");
    if (!span || !article.contains(span) || startNode !== span.firstChild) return;
    const offset = Number(span.dataset.mdStart);
    const start = offset + range.startOffset;
    const end = offset + range.endOffset;
    const selection = sel.toString();
    if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(start) ||
        !Number.isSafeInteger(end) || end <= start ||
        selection.length < 1 || selection.length > 500 ||
        selection.trim() !== selection) {
      status.textContent = "Выделите короткую цитату без пробелов по краям.";
      return;
    }
    const index = blocks.findIndex(b => start >= b.start &&
      end <= b.start + b.length);
    if (index === -1) {
      status.textContent = "Выделите цитату в пределах одного фрагмента главы.";
      return;
    }
    const relativeStart = start - blocks[index].start;
    const relativeEnd = end - blocks[index].start;
    if (relativeEnd > 100000) return;
    quote.value = selection;
    block.value = String(index + 1);
    startInput.value = String(relativeStart);
    endInput.value = String(relativeEnd);
    valid = true;
    status.textContent = "Цитата выбрана. Укажите правильный вариант.";
    updateButton();
  });
  form.addEventListener("submit", event => {
    if (!valid || !suggestion.value.trim()) {
      event.preventDefault();
      status.textContent = "Сначала выделите цитату и предложите исправление.";
    }
  });
})();
