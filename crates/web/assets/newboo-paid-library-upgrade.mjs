// NEwBOO library: explicit, owner-scoped paid edition update.
for (const button of document.querySelectorAll('[data-paid-upgrade]')) {
  button.addEventListener('click', async () => {
    const chapter = button.dataset.chapterId;
    const purchase = button.dataset.purchaseId;
    const status = button.parentElement.querySelector('[data-paid-upgrade-status]');
    const csrf = document.cookie.split('; ').find(x => x.startsWith('__Host-tor-csrf=') || x.startsWith('tor-dev-csrf='));
    if (!chapter || !purchase || !csrf) {
      status.textContent = 'Не удалось подтвердить запрос. Обновите страницу.';
      return;
    }
    button.disabled = true;
    status.textContent = 'Проверяем редакцию…';
    try {
      const response = await fetch('/api/v1/me/purchases/' + encodeURIComponent(chapter) + '/upgrade', {
        method: 'POST',
        credentials: 'same-origin',
        headers: {
          'Content-Type': 'application/json',
          'X-CSRF-Token': decodeURIComponent(csrf.split('=').slice(1).join('=')),
        },
        body: JSON.stringify({ purchase_id: purchase }),
      });
      if (!response.ok) throw new Error('upgrade denied');
      const data = await response.json();
      if (!Number.isSafeInteger(data.version_id) || data.version_id <= 0) throw new Error('invalid version');
      status.textContent = 'Редакция обновлена. Открываем библиотеку…';
      window.location.reload();
    } catch {
      status.textContent = 'Не удалось обновить редакцию. Попробуйте позже.';
      button.disabled = false;
    }
  });
}
