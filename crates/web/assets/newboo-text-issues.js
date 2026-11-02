// E-12: derive an exact source offset from the one-span selection in a free
// chapter. The server checks the stored edition and canonical quote again.
document.addEventListener("DOMContentLoaded", function () {
  const form = document.querySelector(".reader-issues form");
  const chapter = document.querySelector(".reader-private__content");
  if (!form || !chapter) return;
  const quote = form.querySelector("#issue-quote");
  const block = form.querySelector("#issue-block");
  const startInput = form.querySelector("#issue-start");
  const endInput = form.querySelector("#issue-end");
  document.addEventListener("selectionchange", function () {
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed || selection.rangeCount !== 1) return;
    const range = selection.getRangeAt(0);
    if (range.startContainer.nodeType !== Node.TEXT_NODE
        || range.endContainer.nodeType !== Node.TEXT_NODE) return;
    const startSpan = range.startContainer.parentElement.closest("[data-md-start]");
    const endSpan = range.endContainer.parentElement.closest("[data-md-start]");
    if (!startSpan || startSpan !== endSpan || !chapter.contains(startSpan)) return;
    const piece = startSpan.closest(".reader-piece");
    if (!piece) return;
    const index = /^p-(\d+)$/.exec(piece.id);
    if (!index) return;
    const sourceOffset = Number(startSpan.dataset.mdStart);
    const start = sourceOffset + range.startOffset;
    const end = sourceOffset + range.endOffset;
    const selected = selection.toString();
    if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end)
        || start < 0 || end <= start || end > 100000
        || !selected || selected.length > 500 || selected.trim() !== selected) return;
    quote.value = selected;
    block.value = index[1];
    startInput.value = String(start);
    endInput.value = String(end);
  });
});
