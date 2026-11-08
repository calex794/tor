// Private BR-060 archive TOC. No public chapter metadata or purchase grants.
// Runs only in a saved-free archive document with existing script-src 'self'.
(() => {
  "use strict";
  const root = document.querySelector("[data-archive-toc]");
  if (!root) return;
  const trigger = root.querySelector("summary");
  const modal = root.querySelector("[data-archive-toc-modal]");
  const scroll = root.querySelector("[data-archive-toc-scroll]");
  const current = root.querySelector(".archive-toc__row--current");
  const close = root.querySelector("[data-archive-toc-close]");
  const backdrop = root.querySelector("[data-archive-toc-backdrop]");
  if (!trigger || !modal || !scroll || !current || !close || !backdrop) return;
  let oldOverflow = "";
  const hide = () => {
    root.open = false;
    trigger.focus();
  };
  root.addEventListener("toggle", () => {
    if (root.open) {
      oldOverflow = document.body.style.overflow;
      document.body.style.overflow = "hidden";
      close.focus();
      // A 2000-chapter book must open around the exact saved chapter.
      const containerTop = scroll.getBoundingClientRect().top;
      const chapterTop = current.getBoundingClientRect().top;
      scroll.scrollTop = Math.max(0, scroll.scrollTop +
        chapterTop - containerTop - scroll.clientHeight / 2 +
        current.getBoundingClientRect().height / 2);
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
      const focusables = [close, scroll, ...modal.querySelectorAll("a[href]")].filter(
        node => node.getClientRects().length > 0
      );
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
