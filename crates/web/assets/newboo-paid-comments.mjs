// S011: purchased-edition discussions appear only after successful D/B/G
// decryption. Server rechecks purchase, account, age and edition on EVERY call.
function newKey() {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
function node(tag, text) {
  const element = document.createElement(tag);
  if (text !== undefined) element.textContent = text;
  return element;
}
function exactSelection(article, blocks) {
  const selection = window.getSelection();
  if (!selection || selection.isCollapsed || selection.rangeCount !== 1) return null;
  const range = selection.getRangeAt(0);
  if (range.startContainer.nodeType !== Node.TEXT_NODE ||
      range.endContainer !== range.startContainer) return null;
  const paragraph = range.startContainer.parentElement?.closest(".reader-piece__text");
  if (!paragraph || !article.contains(paragraph) ||
      !paragraph.contains(range.endContainer)) return null;
  const piece = paragraph.closest(".reader-piece");
  const match = /^p-(\d+)$/.exec(piece?.id || "");
  if (!match) return null;
  const block = Number(match[1]);
  const quote = selection.toString();
  if (paragraph.textContent !== blocks[block - 1] ||
      !quote || quote.length > 500 || quote.trim() !== quote ||
      range.endOffset <= range.startOffset || range.endOffset > 100000) return null;
  return {
    anchor_block: block, anchor_start: range.startOffset,
    anchor_end: range.endOffset, anchor_quote: quote,
  };
}
export async function installPaidComments(root, article, blocks, chapter, purchase, version, csrf) {
  const url = "/api/v1/reader/chapters/" + chapter + "/paid-comments";
  const section = node("section");
  section.className = "comments-block reader-chapter-comments";
  section.id = "paid-chapter-comments";
  section.append(node("h2", "Комментарии к купленной редакции " + version));
  const status = node("p");
  status.setAttribute("role", "status");
  status.setAttribute("aria-live", "polite");
  const selectionLabel = node("p", "Для комментария к фрагменту выделите цитату.");
  const entries = node("div");
  entries.className = "comments-list";
  const more = node("button", "Показать ещё");
  more.type = "button";
  more.hidden = true;
  const form = node("form");
  form.className = "comments-form";
  const input = node("textarea");
  input.placeholder = "Написать комментарий";
  input.setAttribute("aria-label", "Текст комментария");
  input.required = true;
  input.maxLength = 2000;
  const submit = node("button", "Отправить");
  submit.type = "submit";
  form.append(input, submit);
  section.append(selectionLabel, entries, more, form, status);
  article.insertAdjacentElement("afterend", section);
  let anchor = null;
  let parentId = null;
  let cursor = null;
  let alive = true;
  let busy = false;
  function headers() { return {"Content-Type":"application/json","X-CSRF-Token":csrf}; }
  async function send(method, payload) {
    const suffix = method === "GET"
      ? "?" + new URLSearchParams({
          purchase_id:purchase, version_id:version,
          ...(payload?.after ? {after:String(payload.after)} : {}),
        })
      : "";
    const res = await fetch(url + suffix, method === "GET"
      ? {credentials:"same-origin",cache:"no-store"}
      : {method,credentials:"same-origin",headers:headers(),body:JSON.stringify(payload)});
    if (!res.ok) throw new Error("access unavailable");
    return await res.json();
  }
  function showItem(item) {
    const card = node("article");
    card.className = "comment-card";
    const title = node("p", "Комментарий №" + item.id);
    const content = node("p", item.body);
    card.append(title);
    if (item.parent_id) card.append(node("p", "Ответ на №" + item.parent_id));
    if (item.anchor) {
      const a = item.anchor;
      const jump = node("button", "Перейти к фрагменту");
      jump.type = "button";
      jump.addEventListener("click", () => {
        const piece = article.querySelector("#p-" + a.block_index);
        piece?.scrollIntoView({behavior:"smooth",block:"center"});
      });
      card.append(jump);
    }
    card.append(content);
    if (!item.parent_id) {
      const reply = node("button", "Ответить");
      reply.type = "button";
      reply.addEventListener("click", () => {
        parentId = item.id; anchor = null;
        selectionLabel.textContent = "Ответ на комментарий №" + item.id;
        input.focus();
      });
      card.append(reply);
    }
    entries.append(card);
  }
  async function load(reset) {
    if (busy || !alive) return;
    busy = true;
    try {
      const data = await send("GET", {after:reset ? null : cursor});
      if (String(data.version_id) !== String(version)) throw new Error("wrong edition");
      if (reset) entries.replaceChildren();
      for (const item of data.items || []) showItem(item);
      cursor = data.next_cursor;
      more.hidden = !cursor;
      status.textContent = "";
    } catch {
      // Never retain cached private comments after any failed entitlement recheck.
      entries.replaceChildren();
      more.hidden = true;
      status.textContent = "Комментарии недоступны. Проверьте право чтения.";
    } finally { busy = false; }
  }
  const onSelection = () => {
    if (!alive || parentId) return;
    const selected = exactSelection(article, blocks);
    if (!selected) return;
    anchor = selected;
    selectionLabel.textContent = "Цитата выбрана. Её позиция будет проверена сервером.";
  };
  document.addEventListener("selectionchange", onSelection);
  more.addEventListener("click", () => load(false));
  form.addEventListener("submit", async event => {
    event.preventDefault();
    if (busy) return;
    const body = input.value.trim();
    if (!body || body.length > 2000) return;
    busy = true; submit.disabled = true;
    try {
      await send("POST", {
        purchase_id:purchase, version_id:version, body,
        command_key:newKey(), parent_id:parentId,
        ...(!parentId && anchor ? anchor : {}),
      });
      input.value = ""; anchor = null; parentId = null;
      selectionLabel.textContent = "Для комментария к фрагменту выделите цитату.";
      status.textContent = "Комментарий опубликован.";
    } catch { status.textContent = "Отправка не удалась. Право чтения проверяется заново."; }
    finally { busy = false; submit.disabled = false; }
    await load(true);
  });
  window.addEventListener("pagehide", () => {
    alive = false;
    document.removeEventListener("selectionchange", onSelection);
    entries.replaceChildren();
    section.remove();
  }, {once:true});
  await load(true);
  return section;
}
