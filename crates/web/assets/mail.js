// No message body, user ID or cursor is persisted in localStorage or logs.
(() => {
  'use strict';
  const list = document.getElementById('mail-messages');
  if (!list) return;
  const status = document.getElementById('mail-status');
  let last = list.dataset.lastId;
  let busy = false;
  let timer;
  // NEwBOO mail-detail.js: 3s visible, 10s hidden, 250ms on becoming visible.
  function schedule(delay = 3000) { clearTimeout(timer); timer = setTimeout(poll, delay); }
  async function poll() {
    if (busy) { schedule(); return; }
    if (document.hidden) { schedule(10000); return; }
    busy = true;
    try {
      const query = new URLSearchParams({ conversation_id: list.dataset.conversation, limit: '50' });
      query.set('after_id', last);
      const response = await fetch('/api/v1/me/messages?' + query, { credentials: 'same-origin', cache: 'no-store' });
      if (!response.ok) { status.textContent = 'Не удалось получить новые сообщения. Обновите страницу.'; return; }
      const data = await response.json();
      for (const message of data.messages) {
        if (list.querySelector(`[data-message-id="${CSS.escape(message.id)}"]`)) continue;
        const card = document.createElement('article');
        card.className = 'me-now-card';
        card.dataset.messageId = message.id;
        const sender = document.createElement('strong');
        sender.textContent = message.sender_id === list.dataset.actor ? 'Вы' : list.dataset.peerName;
        const text = document.createElement('p');
        text.className = 'mail-message-body';
        text.textContent = message.text;
        const time = document.createElement('time');
        time.dateTime = message.created_at;
        time.textContent = message.created_at_display;
        card.append(sender, text, time);
        list.append(card);
        last = message.id;
      }
      if (data.blocked) status.textContent = 'Новые сообщения заблокированы. Обновите страницу.';
      else status.textContent = '';
    } catch { status.textContent = 'Связь прервана. Повторим получение сообщений.'; }
    finally { busy = false; schedule(); }
  }
  document.addEventListener('visibilitychange', () => { if (!document.hidden) schedule(250); });
  schedule();
})();
