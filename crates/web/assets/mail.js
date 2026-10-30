// NEwBOO E-08: AJAX send, retry identity, older prepend and visible polling.
(() => {
  'use strict';
  const list = document.getElementById('mail-messages');
  if (!list) return;
  const form = document.querySelector('[data-mail-compose]');
  const status = document.getElementById('mail-status');
  const error = document.getElementById('mail-send-error');
  const older = document.getElementById('mail-load-older');
  let last = Number(list.dataset.lastId || 0);
  let busy = false, timer;
  const ids = () => new Set(Array.from(list.querySelectorAll('[data-message-id]'), el => el.dataset.messageId));
  const card = (message) => {
    const article = document.createElement('article');
    article.className = 'me-now-card';
    article.dataset.messageId = String(message.id);
    const sender = document.createElement('strong');
    sender.textContent = String(message.sender_id) === list.dataset.actor ? 'Вы' : list.dataset.peerName;
    const body = document.createElement('p');
    body.className = 'mail-message-body';
    body.textContent = message.text;
    const time = document.createElement('time');
    time.dateTime = message.created_at;
    time.textContent = message.created_at_display;
    article.append(sender, body, time);
    return article;
  };
  function add(messages, prepend = false) {
    const existing = ids();
    const fragment = document.createDocumentFragment();
    for (const message of messages) {
      const id = String(message.id);
      if (existing.has(id)) continue;
      existing.add(id);
      fragment.append(card(message));
      if (!prepend) last = Math.max(last, Number(message.id));
    }
    if (prepend) {
      const height = list.scrollHeight;
      list.prepend(fragment);
      list.scrollTop += list.scrollHeight - height;
    } else {
      list.append(fragment);
    }
  }
  function schedule(delay = 3000) { clearTimeout(timer); timer = setTimeout(poll, delay); }
  async function get(query) {
    const response = await fetch('/api/v1/me/messages?' + query, {
      credentials: 'same-origin', cache: 'no-store', headers: { Accept: 'application/json' }
    });
    if (!response.ok) throw new Error('mail-request');
    return response.json();
  }
  async function poll() {
    if (busy) { schedule(); return; }
    if (document.hidden) { schedule(10000); return; }
    busy = true;
    try {
      const query = new URLSearchParams({conversation_id: list.dataset.conversation, limit: '50', after_id: String(last)});
      const data = await get(query);
      add(data.messages || []);
      status.textContent = data.blocked ? 'Новые сообщения заблокированы. Обновите страницу.' : '';
    } catch (_) { status.textContent = 'Связь прервана. Повторим получение сообщений.'; }
    finally { busy = false; schedule(); }
  }
  if (older) older.addEventListener('click', async event => {
    event.preventDefault();
    if (older.dataset.loading === 'true') return;
    older.dataset.loading = 'true';
    const query = new URLSearchParams({conversation_id: list.dataset.conversation, limit:'50', before_id: older.dataset.beforeId});
    try {
      const data = await get(query);
      const messages = data.messages || [];
      add(messages, true);
      if (!data.has_more || messages.length === 0) older.remove();
      else older.dataset.beforeId = data.oldest_message_id;
    } catch (_) { status.textContent = 'Не удалось загрузить предыдущие сообщения.'; }
    finally { older.dataset.loading = 'false'; }
  });
  if (form) form.addEventListener('submit', async event => {
    event.preventDefault();
    if (form.dataset.sending === 'true') return;
    const textarea = form.querySelector('[name="body"]');
    const clientId = form.querySelector('[name="client_message_id"]');
    const button = form.querySelector('[type="submit"]');
    if (!textarea.value.trim()) { error.hidden = false; error.textContent = 'Сообщение не может быть пустым.'; return; }
    if (!clientId.value) clientId.value = crypto.randomUUID();
    form.dataset.sending = 'true';
    button.disabled = true;
    error.hidden = true;
    try {
      const response = await fetch(form.action, {
        // Axum Form<T> accepts URL-encoded form bodies; raw FormData would send multipart.
        method: 'POST', body: new URLSearchParams(new FormData(form)), credentials: 'same-origin',
        headers: { Accept: 'application/json', 'X-Requested-With': 'fetch', 'Idempotency-Key': clientId.value }
      });
      if (!response.ok) throw new Error('http-' + response.status);
      const result = await response.json();
      if (!result.ok) throw new Error('invalid');
      add([result.message]);
      textarea.value = '';
      clientId.value = '';
      textarea.focus();
    } catch (_) {
      error.hidden = false;
      error.textContent = 'Не удалось отправить сообщение. Проверьте соединение и повторите.';
      // Preserve clientId so a retry returns the original message after timeout.
    } finally { button.disabled = false; form.dataset.sending = 'false'; }
  });
  document.addEventListener('visibilitychange', () => { if (!document.hidden) schedule(250); });
  schedule();
})();
