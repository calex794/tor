// No message body, user ID or cursor is persisted in localStorage or logs.
(() => {
  'use strict';
  const list = document.getElementById('mail-messages');
  if (!list) return;
  const status = document.getElementById('mail-status');
  let last = list.dataset.lastId;
  let busy = false;
  async function poll() {
    if (busy || document.hidden) return;
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
        sender.textContent = 'Пользователь №' + message.sender_id;
        const text = document.createElement('p');
        text.className = 'mail-message-body';
        text.textContent = message.text;
        const time = document.createElement('time');
        time.textContent = message.created_at;
        card.append(sender, text, time);
        list.append(card);
        if (list.children.length > 200) list.firstElementChild.remove();
        last = message.id;
      }
      if (data.blocked) status.textContent = 'Новые сообщения заблокированы. Обновите страницу.';
      else status.textContent = '';
      const read = document.querySelector('input[name="through_id"]');
      if (read && last !== '0') { read.value = last; read.form.hidden = false; }
    } catch { status.textContent = 'Связь прервана. Повторим получение сообщений.'; }
    finally { busy = false; }
  }
  setInterval(poll, 5000);
})();
