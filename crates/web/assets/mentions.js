// E-03: scoped mention chooser. No global account enumeration.
(() => {
  const ROOT = "/api/v1/social/mentions/discover";
  for (const field of document.querySelectorAll("textarea[data-mention-book]")) {
    const list = document.createElement("div");
    list.className = "comment-mention-suggestions";
    list.setAttribute("role", "listbox");
    list.hidden = true;
    field.after(list);
    let current = 0;
    let timer;
    const hide = () => { list.hidden = true; list.replaceChildren(); };
    field.addEventListener("input", () => {
      clearTimeout(timer);
      const value = field.value.slice(0, field.selectionStart);
      const match = value.match(/(?:^|\s)@([\p{L}\p{N}_.-]{2,64})$/u);
      if (!match) { hide(); return; }
      const serial = ++current;
      timer = setTimeout(async () => {
        try {
          const params = new URLSearchParams({ book_id: field.dataset.mentionBook, q: match[1] });
          const response = await fetch(ROOT + "?" + params, { credentials: "same-origin" });
          if (!response.ok) throw new Error("candidate lookup failed");
          const payload = await response.json();
          if (serial !== current) return;
          hide();
          for (const item of (payload.items || []).slice(0, 20)) {
            const button = document.createElement("button");
            button.type = "button";
            button.textContent = item.label + " — участник обсуждения";
            button.addEventListener("mousedown", event => event.preventDefault());
            button.addEventListener("click", () => {
              const start = field.selectionStart - match[1].length - 1;
              const token = "@[user:" + item.user_id + "] ";
              field.setRangeText(token, start, field.selectionStart, "end");
              field.dispatchEvent(new Event("input", { bubbles: true }));
              hide();
              field.focus();
            });
            list.appendChild(button);
          }
          list.hidden = !list.childElementCount;
        } catch (_) { hide(); }
      }, 250);
    });
    field.addEventListener("keydown", event => {
      if (event.key === "Escape") { ++current; hide(); }
    });
  }
})();
