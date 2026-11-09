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
// S013: the paid reader uses exactly the paired-spoiler semantics of the
// public Rust comments renderer. All untrusted content becomes DOM text, never HTML.
export function parseCommentSpoilers(body) {
  let rest = typeof body === "string" ? body : "";
  const segments = [];
  while (true) {
    const start = rest.indexOf("||");
    if (start < 0) break;
    const end = rest.indexOf("||", start + 2);
    if (end < 0) break;
    if (start > 0) segments.push({kind: "text", text: rest.slice(0, start)});
    if (end === start + 2) {
      segments.push({kind: "text", text: rest.slice(start, end + 2)});
    } else {
      segments.push({kind: "spoiler", text: rest.slice(start + 2, end)});
    }
    rest = rest.slice(end + 2);
  }
  if (rest || segments.length === 0) segments.push({kind: "text", text: rest});
  return segments;
}
function asciiHandle(char) {
  return char !== undefined && /^[A-Za-z0-9_.]$/.test(char);
}
function asciiAlnum(char) {
  return char !== undefined && /^[A-Za-z0-9]$/.test(char);
}
// Explicit identity-neutral display, matching the Rust S013 public renderer.
// This masks supported mention syntax without trusting a client-provided identity.
export function maskCommentMentions(value) {
  const text = typeof value === "string" ? value : "";
  let result = "";
  let copied = 0;
  for (let start = 0; start < text.length; start += 1) {
    if (text[start] !== "@" || start < copied ||
        (start > 0 && (asciiAlnum(text[start - 1]) ||
          text[start - 1] === "_" || text[start - 1] === "@"))) continue;
    let end;
    if (text.startsWith("[user:", start + 1)) {
      const first = start + 7;
      let cursor = first;
      while (cursor < text.length && /^[0-9]$/.test(text[cursor])) cursor += 1;
      const digits = cursor - first;
      if (digits < 1 || digits > 19 || text[first] === "0" || text[cursor] !== "]") continue;
      end = cursor + 1;
    } else {
      let cursor = start + 1;
      while (asciiHandle(text[cursor])) cursor += 1;
      const count = cursor - start - 1;
      if (count < 3 || count > 32) continue;
      end = cursor;
    }
    if (asciiHandle(text[end])) continue;
    result += text.slice(copied, start) + "@пользователь";
    copied = end;
    start = end - 1;
  }
  return result + text.slice(copied);
}
export function appendPaidCommentMarkup(target, body) {
  for (const segment of parseCommentSpoilers(body)) {
    if (segment.kind === "text") {
      target.append(document.createTextNode(maskCommentMentions(segment.text)));
    } else {
      const details = node("details");
      details.className = "comment-spoiler";
      const summary = node("summary", "Спойлер");
      summary.className = "comment-spoiler__toggle";
      const content = node("span", maskCommentMentions(segment.text));
      content.className = "comment-spoiler__content";
      details.append(summary, content);
      target.append(details);
    }
  }
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
  // Do not accept a click while an entitlement-backed list is loading.
  submit.disabled = true;
  form.append(node("p", "Чтобы скрыть спойлер, заключите текст в ||двойные черты||."), input, submit);
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
    const content = node("div");
    content.className = "comment-card__body";
    appendPaidCommentMarkup(content, item.body);
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
    // The server binds a complaint to this exact purchased edition again.
    // Never send a body or selection quote back in a report.
    const reportBox = node("details");
    const reportSummary = node("summary", "Пожаловаться на комментарий");
    const reportForm = node("form");
    const reason = node("select");
    reason.setAttribute("aria-label", "Причина жалобы");
    for (const [value, label] of [
      ["spam", "Спам"], ["abuse", "Оскорбления или угрозы"],
      ["misleading", "Недостоверные сведения"], ["other", "Другое"]
    ]) {
      const option = node("option", label); option.value = value; reason.append(option);
    }
    const details = node("textarea");
    details.maxLength = 500;
    details.placeholder = "Обоснование жалобы (необязательно)";
    details.setAttribute("aria-label", "Обоснование жалобы");
    const reportStatus = node("p");
    reportStatus.setAttribute("role", "status");
    const sendReport = node("button", "Отправить жалобу");
    sendReport.type = "submit";
    reportForm.append(reason, details, sendReport, reportStatus);
    reportBox.append(reportSummary, reportForm);
    const intent = newKey();
    reportForm.addEventListener("submit", async event => {
      event.preventDefault();
      sendReport.disabled = true;
      try {
        const result = await fetch(url + "/report", {
          method:"POST", credentials:"same-origin", cache:"no-store",
          headers:headers(),
          body:JSON.stringify({
            purchase_id:purchase, version_id:version, comment_id:item.id,
            reason:reason.value, details:details.value.trim(), command_key:intent
          })
        });
        if (!result.ok) throw new Error("report denied");
        reportStatus.textContent = "Жалоба зарегистрирована.";
        reportBox.open = true;
        reportForm.remove();
      } catch {
        reportStatus.textContent = "Жалоба не отправлена. Проверьте право чтения.";
        sendReport.disabled = false;
      }
    });
    card.append(reportBox);
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
    submit.disabled = true;
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
    } finally {
      busy = false;
      submit.disabled = !alive;
    }
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
