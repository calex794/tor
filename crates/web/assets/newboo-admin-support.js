(() => {
  const thread = document.querySelector("[data-admin-support-thread]");
  if (!thread) return;
  const form = document.querySelector("[data-admin-support-form]");
  const input = document.querySelector("[data-admin-support-input]");
  const submit = document.querySelector("[data-admin-support-submit]");
  const clientIdInput = document.querySelector("[data-admin-support-client-id]");
  const error = document.querySelector("[data-admin-support-error]");
  let pendingId = "", pendingBody = "";
  let newestId = thread.lastElementChild?.dataset.messageId || "";

  const uuid = () => window.crypto?.randomUUID?.() || `${Date.now()}-${Math.random().toString(16).slice(2)}`;
  const append = (message) => {
    if (!message?.message_id || thread.querySelector(`[data-message-id="${CSS.escape(message.message_id)}"]`)) return;
    const article = document.createElement("article");
    article.className = `admin-support-message ${message.is_own ? "is-own" : ""}`;
    article.dataset.messageId = message.message_id;
    const body = document.createElement("div");
    body.textContent = message.body || "";
    article.appendChild(body);
    thread.appendChild(article);
    newestId = message.message_id;
    thread.scrollTop = thread.scrollHeight;
  };
  thread.scrollTop = thread.scrollHeight;

  const poll = async () => {
    if (document.hidden) return;
    const suffix = newestId ? `?after_id=${encodeURIComponent(newestId)}` : "";
    try {
      const response = await fetch(`${thread.dataset.pollUrl}${suffix}`, { headers: { Accept: "application/json" }, credentials: "same-origin" });
      if (response.ok) (await response.json()).messages.forEach(append);
    } catch (_) { /* следующий poll повторит read */ }
  };
  window.setInterval(poll, 5000);

  form?.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (!pendingId || pendingBody !== input.value) { pendingId = uuid(); pendingBody = input.value; }
    const id = pendingId;
    clientIdInput.value = id;
    submit.disabled = true;
    error.hidden = true;
    try {
      const data = new FormData(form);
      const response = await fetch(form.action, { method: "POST", body: new URLSearchParams(data), credentials: "same-origin", headers: { Accept: "application/json", "Idempotency-Key": id } });
      const payload = await response.json();
      if (!response.ok) throw new Error(payload.message || "Не удалось отправить сообщение.");
      append(payload.message);
      input.value = "";
      pendingId = ""; pendingBody = "";
    } catch (failure) {
      error.textContent = "Не удалось отправить сообщение. Проверьте соединение и повторите. Текст сохранён.";
      error.hidden = false;
    } finally {
      submit.disabled = false;
      input.focus();
    }
  });
})();
