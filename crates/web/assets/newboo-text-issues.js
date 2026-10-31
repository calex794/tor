// E-12 free reader selection. The server re-verifies the quote and version.
document.addEventListener("DOMContentLoaded", function () {
  const section = document.querySelector(".reader-issues");
  const content = document.querySelector(".reader-private__content");
  if (!section || !content) return;
  const quote = section.querySelector("#issue-quote");
  const block = section.querySelector("#issue-block");
  document.addEventListener("selectionchange", function () {
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed) return;
    const anchor = selection.anchorNode && selection.anchorNode.parentElement;
    const focus = selection.focusNode && selection.focusNode.parentElement;
    if (!anchor || !focus) return;
    const first = anchor.closest(".reader-piece");
    const last = focus.closest(".reader-piece");
    if (!first || first !== last || !content.contains(first)) return;
    const text = selection.toString().trim();
    if (!text || text.length > 500) return;
    const matched = /^p-(\d+)$/.exec(first.id);
    if (!matched) return;
    block.value = matched[1];
    quote.value = text;
  });
});
