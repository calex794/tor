(() => {
  const root = document.querySelector("[data-support-widget]");
  if (!root) return;
  root.hidden = false;

  const trigger = root.querySelector("[data-support-open]");
  const panel = root.querySelector("[data-support-panel]");
  const closeButton = root.querySelector("[data-support-close]");
  const notice = root.querySelector("[data-support-notice]");
  const messages = root.querySelector("[data-support-messages]");
  const form = root.querySelector("[data-support-form]");
  const input = root.querySelector("[data-support-input]");
  const submit = root.querySelector("[data-support-submit]");
  const error = root.querySelector("[data-support-error]");
  let loaded = false;
  let loading = false;
  let csrfToken = "";
  let newestMessageId = "";
  let pollTimer = null;
  let pendingClientId = "", pendingBody = "";

  const clientId = () => {
    if (window.crypto && window.crypto.randomUUID) return window.crypto.randomUUID();
    return `${Date.now()}-${Math.random().toString(16).slice(2)}`;
  };

  const scrollToBottom = () => {
    messages.scrollTop = messages.scrollHeight;
  };

  const appendMessage = (message) => {
    if (!message || !message.message_id) return;
    if (messages.querySelector(`[data-message-id="${CSS.escape(message.message_id)}"]`)) return;
    const item = document.createElement("div");
    item.className = `support-widget__message ${message.is_own ? "support-widget__message--own" : "support-widget__message--support"}`;
    item.dataset.messageId = message.message_id;
    const body = document.createElement("div");
    body.className = "support-widget__message-body";
    body.textContent = message.body || "";
    item.appendChild(body);
    messages.appendChild(item);
    newestMessageId = message.message_id;
  };

  const renderMessages = (rows) => {
    (rows || []).forEach(appendMessage);
    scrollToBottom();
  };

  const load = async () => {
    if (loaded || loading) return;
    loading = true;
    error.hidden = true;
    try {
      const response = await fetch("/support/widget", {
        headers: { Accept: "application/json", "X-Requested-With": "fetch" },
        credentials: "same-origin",
      });
      if (!response.ok) throw new Error("load_failed");
      const data = await response.json();
      csrfToken = data.csrf_token || "";
      notice.textContent = data.identity_notice || "";
      messages.replaceChildren();
      renderMessages(data.messages);
      loaded = true;
      input.disabled = false;
      submit.disabled = false;
    } catch (_) {
      error.textContent = "Не удалось загрузить переписку. Попробуйте ещё раз.";
      error.hidden = false;
    } finally {
      loading = false;
    }
  };

  const poll = async () => {
    if (!loaded || panel.hidden || document.hidden) return;
    const query = newestMessageId ? `?after_id=${encodeURIComponent(newestMessageId)}` : "";
    try {
      const response = await fetch(`/support/messages${query}`, {
        headers: { Accept: "application/json", "X-Requested-With": "fetch" },
        credentials: "same-origin",
      });
      if (response.ok) renderMessages((await response.json()).messages);
    } catch (_) {
      // Следующий polling повторит временно неудавшийся read.
    }
  };

  const schedulePoll = () => {
    window.clearInterval(pollTimer);
    pollTimer = window.setInterval(poll, 5000);
  };

  const open = async () => {
    panel.hidden = false;
    trigger.setAttribute("aria-expanded", "true");
    await load();
    input.focus();
    schedulePoll();
  };

  const close = () => {
    panel.hidden = true;
    trigger.setAttribute("aria-expanded", "false");
    window.clearInterval(pollTimer);
    trigger.focus();
  };

  trigger.addEventListener("click", open);
  closeButton.addEventListener("click", close);
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !panel.hidden) close();
  });
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && !panel.hidden) poll();
  });

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const body = input.value.trim();
    if (!body || !loaded) return;
    submit.disabled = true;
    error.hidden = true;
    if (!pendingClientId || pendingBody !== body) {
      pendingClientId = clientId(); pendingBody = body;
    }
    const messageClientId = pendingClientId;
    const data = new FormData();
    data.set("body", body);
    data.set("client_message_id", messageClientId);
    data.set("csrf_token", csrfToken);
    data.set("website", "");
    try {
      const response = await fetch("/support/messages", {
        method: "POST",
        body: new URLSearchParams(data),
        credentials: "same-origin",
        headers: { Accept: "application/json", "X-Requested-With": "fetch", "Idempotency-Key": messageClientId, "X-CSRF-Token": csrfToken },
      });
      const payload = await response.json();
      if (!response.ok || !payload.ok) throw new Error(payload.message || "send_failed");
      appendMessage(payload.message);
      input.value = "";
      pendingClientId = ""; pendingBody = "";
      scrollToBottom();
    } catch (sendError) {
      error.textContent = "Не удалось отправить сообщение. Проверьте соединение и повторите. Текст сохранён.";
      error.hidden = false;
    } finally {
      submit.disabled = false;
      input.focus();
    }
  });
})();
