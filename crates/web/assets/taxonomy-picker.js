/* B/TAX-01. Safe, bounded multi-selection backed by canonical taxonomy IDs. */
(() => {
  "use strict";
  const MAX_OPTIONS = 15;
  for (const root of document.querySelectorAll("[data-taxonomy-picker]")) {
    const kind = root.dataset.kind;
    if (!["genre", "tag", "fandom"].includes(kind)) continue;
    const input = root.querySelector("[data-query]");
    const list = root.querySelector("[data-results]");
    const chips = root.querySelector("[data-selected]");
    const hidden = root.querySelector("[data-selected-ids]");
    const hint = root.querySelector("[data-hint]");
    const limit = Math.min(8, Number(root.dataset.limit) || 8);
    if (!input || !list || !chips || !hidden || !hint) continue;

    let selected = Array.from(chips.querySelectorAll("[data-taxonomy-id]"))
      .map(node => ({
        id: Number(node.dataset.taxonomyId),
        label: node.dataset.taxonomyLabel || "",
      }))
      .filter(item => Number.isSafeInteger(item.id) && item.id > 0
        && item.label && item.label.length <= 80)
      .slice(0, limit);
    let options = [];
    let currentIndex = -1;
    let timer = null;
    let pending = null;
    let requestId = 0;

    function status(message) { hint.textContent = message; }
    function closeList() {
      options = [];
      currentIndex = -1;
      list.replaceChildren();
      list.hidden = true;
      input.setAttribute("aria-expanded", "false");
      input.removeAttribute("aria-activedescendant");
    }
    function renderChips() {
      chips.replaceChildren();
      hidden.value = selected.map(item => String(item.id)).join(",");
      for (const item of selected) {
        const button = document.createElement("button");
        button.type = "button";
        button.className = "taxonomy-picker__chip";
        button.textContent = item.label + " ×";
        button.setAttribute("aria-label", "Удалить: " + item.label);
        button.addEventListener("click", () => {
          selected = selected.filter(value => value.id !== item.id);
          renderChips();
          input.focus();
          search();
        });
        chips.appendChild(button);
      }
      if (selected.length >= limit) {
        status("Выбрано максимальное количество: " + limit);
        closeList();
      }
    }
    function choose(item) {
      if (selected.length >= limit || selected.some(value => value.id === item.id)) return;
      selected.push({ id: item.id, label: item.label });
      input.value = "";
      closeList();
      renderChips();
      status("Выбрано " + selected.length + " из " + limit);
      input.focus();
    }
    function setActive(index) {
      if (options.length === 0) return;
      currentIndex = ((index % options.length) + options.length) % options.length;
      for (const [i, element] of Array.from(list.children).entries()) {
        element.setAttribute("aria-selected", String(i === currentIndex));
      }
      const id = input.id + "-option-" + currentIndex;
      input.setAttribute("aria-activedescendant", id);
      list.children[currentIndex].scrollIntoView({ block: "nearest" });
    }
    function renderOptions(items) {
      list.replaceChildren();
      options = items.filter(item => Number.isSafeInteger(item.id)
        && item.id > 0 && typeof item.label === "string"
        && !selected.some(value => value.id === item.id)).slice(0, MAX_OPTIONS);
      currentIndex = -1;
      if (!options.length) {
        closeList();
        status("Совпадений нет. Новые названия добавляются через модерацию справочника.");
        return;
      }
      options.forEach((item, index) => {
        const li = document.createElement("li");
        li.id = input.id + "-option-" + index;
        li.setAttribute("role", "option");
        li.setAttribute("aria-selected", "false");
        const name = document.createElement("span");
        name.textContent = item.label; // Never parse taxonomy labels as HTML.
        const usage = document.createElement("span");
        usage.className = "taxonomy-picker__usage";
        usage.textContent = Number(item.usage || 0).toLocaleString("ru-RU") + " книг";
        li.append(name, usage);
        li.addEventListener("pointerdown", event => event.preventDefault());
        li.addEventListener("click", () => choose(item));
        list.appendChild(li);
      });
      list.hidden = false;
      input.setAttribute("aria-expanded", "true");
      status("Результатов: " + options.length + ". Стрелки — выбор, Enter — добавить.");
    }
    async function search() {
      const q = input.value.trim();
      if (pending) pending.abort();
      const serial = ++requestId;
      if (q.length < 2 || selected.length >= limit) {
        closeList();
        if (selected.length < limit) status("Введите минимум два символа.");
        return;
      }
      pending = new AbortController();
      status("Ищем...");
      try {
        const params = new URLSearchParams({ kind, q, limit: String(MAX_OPTIONS) });
        const response = await fetch("/api/v1/taxonomies/suggest?" + params.toString(), {
          credentials: "same-origin",
          headers: { Accept: "application/json" },
          signal: pending.signal,
        });
        if (serial !== requestId) return;
        if (!response.ok) throw new Error("HTTP " + response.status);
        const data = await response.json();
        renderOptions(Array.isArray(data.items) ? data.items : []);
      } catch (error) {
        if (serial !== requestId || error.name === "AbortError") return;
        closeList();
        status("Не удалось загрузить теги. Повторите запрос.");
      }
    }
    input.addEventListener("input", () => {
      clearTimeout(timer);
      timer = setTimeout(search, 180);
    });
    input.addEventListener("keydown", event => {
      if (event.key === "Escape") {
        closeList();
        return;
      }
      if (!options.length || list.hidden) return;
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        setActive(currentIndex + (event.key === "ArrowDown" ? 1 : -1));
      } else if (event.key === "Enter" && currentIndex >= 0) {
        event.preventDefault();
        choose(options[currentIndex]);
      }
    });
    input.addEventListener("blur", () => {
      clearTimeout(timer);
      ++requestId;
      if (pending) pending.abort();
      setTimeout(closeList, 150);
    });
    renderChips();
  }
})();
