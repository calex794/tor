// Paid-reader TOC interaction: only server-verified D metadata is used.
(() => {
  "use strict";
  const reader = document.querySelector("[data-paid-chapter-reader]");
  if (!reader) return;
  const roots = [...reader.querySelectorAll("[data-paid-toc], [data-paid-toc-bottom]")];
  for (const root of roots) {
    const trigger = root.querySelector("summary");
    const dialog = root.querySelector("[data-paid-toc-modal]");
    const viewport = root.querySelector("[data-paid-toc-scroll]");
    const close = root.querySelector("[data-paid-toc-close]");
    const backdrop = root.querySelector("[data-paid-toc-backdrop]");
    const current = root.querySelector('[data-toc-state="current"]');
    if (!trigger || !dialog || !viewport || !close || !backdrop || !current) continue;
    let oldOverflow = "";
    const hide = () => { root.open = false; };
    root.addEventListener("toggle", () => {
      if (root.open) {
        for (const other of roots) {
          if (other !== root) other.open = false;
        }
        oldOverflow = document.body.style.overflow;
        document.body.style.overflow = "hidden";
        close.focus();
        current.scrollIntoView({ block: "center", inline: "nearest" });
      } else {
        document.body.style.overflow = oldOverflow;
        trigger.focus();
      }
    });
    close.addEventListener("click", hide);
    backdrop.addEventListener("click", hide);
    document.addEventListener("keydown", event => {
      if (!root.open) return;
      if (event.key === "Escape") {
        event.preventDefault();
        hide();
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
