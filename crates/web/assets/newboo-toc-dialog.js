// REUSE-04A: presentation-only keyboard and dismissal behavior.
// This source is scoped to one TOC bundle; no global API or entitlement logic.
function bindTocDialog(root, trigger, modal, close, backdrop, extraFocusables) {
  const hide = () => {
    root.open = false;
    trigger.focus();
  };
  close.addEventListener("click", hide);
  backdrop.addEventListener("click", hide);
  document.addEventListener("keydown", event => {
    if (!root.open) return;
    if (event.key === "Escape") {
      event.preventDefault();
      hide();
    } else if (event.key === "Tab") {
      const nodes = [close, ...extraFocusables, ...modal.querySelectorAll("a[href]")]
        .filter(node => node.getClientRects().length > 0);
      if (!nodes.length) return;
      const first = nodes[0], last = nodes[nodes.length - 1];
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
}
