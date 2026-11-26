// Progressive enhancement only. Credentials and CSRF are submitted to the existing
// server handlers, never persisted or used to decide authorization in the browser.
(() => {
  'use strict';
  const dialog = document.getElementById('tor-auth-dialog');
  if (!dialog || typeof dialog.showModal !== 'function' || !window.fetch) return;

  const slot = dialog.querySelector('[data-auth-form-slot]');
  const status = dialog.querySelector('[data-auth-status]');
  const error = dialog.querySelector('[data-auth-error]');
  const standalone = dialog.querySelector('[data-auth-standalone]');
  const recover = dialog.querySelector('[data-auth-recover]');
  const close = dialog.querySelector('[data-auth-close]');
  const modes = [...dialog.querySelectorAll('[data-auth-mode]')];
  const dialogHeaders = { 'X-Tor-Auth-Dialog': '1' };
  let mode = 'login';
  let busy = false;
  let submitting = false;
  let opener = null;

  function showError(message) {
    error.textContent = message;
    error.hidden = !message;
  }

  function setBusy(value) {
    busy = value;
    for (const button of modes) button.disabled = value;
    const submit = slot.querySelector('button[type="submit"]');
    if (submit) submit.disabled = value;
    close.disabled = submitting;
    dialog.setAttribute('aria-busy', String(value));
  }

  function savedFields() {
    const form = slot.querySelector('form');
    // Do not carry passwords between modes or form requests.
    return Object.fromEntries(['email', 'display_name', 'manual_invite_code'].map(name =>
      [name, form?.elements.namedItem(name)?.value || '']));
  }

  function standalonePath() {
    const target = new URL('/' + mode, location.origin);
    // Retain the existing server allowlist for the no-JS / standalone fallback.
    if (/^\/(read|reader)\/[1-9][0-9]*$/.test(location.pathname)) {
      target.searchParams.set('next', location.pathname);
    }
    return target.pathname + target.search;
  }

  async function readDocument(response) {
    if (!(response.headers.get('content-type') || '').startsWith('text/html')) {
      throw new Error('unexpected auth document');
    }
    return new DOMParser().parseFromString(await response.text(), 'text/html');
  }

  async function loadForm(nextMode, saved = {}) {
    mode = nextMode;
    standalone.href = standalonePath();
    recover.hidden = mode !== 'login';
    for (const button of modes) {
      button.setAttribute('aria-pressed', String(button.dataset.authMode === mode));
    }
    slot.replaceChildren();
    status.textContent = 'Загрузка формы…';
    const response = await fetch('/' + mode, {
      credentials: 'same-origin', cache: 'no-store', redirect: 'error',
      headers: dialogHeaders, referrerPolicy: 'same-origin',
      signal: AbortSignal.timeout(15000)
    });
    if (!response.ok) throw new Error('auth form unavailable');
    const doc = await readDocument(response);
    const source = doc.querySelector('#main .auth-card--newboo form.auth-form');
    if (!source || source.getAttribute('method') !== 'post' ||
        source.getAttribute('action') !== '/' + mode ||
        !source.querySelector('input[type="hidden"][name="csrf_token"]')?.value) {
      throw new Error('unexpected auth form');
    }
    if (!dialog.open) return;
    // Import only the server-rendered form, not the page shell or scripts.
    const form = document.importNode(source, true);
    for (const [name, value] of Object.entries(saved)) {
      const input = form.elements.namedItem(name);
      if (input && value) input.value = value;
    }
    form.querySelector('input[type="password"]').value = '';
    slot.replaceChildren(form);
    status.textContent = '';
    if (dialog.open) form.querySelector('input:not([type="hidden"]):not([name="website"])')?.focus();
  }

  async function changeMode(nextMode) {
    if (busy) return;
    const saved = savedFields();
    setBusy(true);
    showError('');
    try {
      await loadForm(nextMode, saved);
    } catch (_) {
      status.textContent = '';
      showError('Не удалось открыть форму. Повторите попытку или откройте отдельную страницу.');
    } finally {
      setBusy(false);
    }
  }

  for (const button of modes) {
    button.addEventListener('click', () => changeMode(button.dataset.authMode));
  }
  close.addEventListener('click', () => { if (!submitting) dialog.close(); });
  dialog.addEventListener('cancel', event => {
    if (submitting) event.preventDefault();
  });
  dialog.addEventListener('close', () => {
    slot.replaceChildren();
    showError('');
    status.textContent = '';
    if (opener?.isConnected) opener.focus();
    opener = null;
  });

  dialog.addEventListener('submit', async event => {
    const form = event.target;
    if (form !== slot.querySelector('form')) return;
    event.preventDefault();
    if (busy) return;
    const body = new URLSearchParams(new FormData(form));
    const saved = savedFields();
    submitting = true;
    setBusy(true);
    showError('');
    status.textContent = mode === 'login' ? 'Выполняется вход…' : 'Создаётся аккаунт…';
    try {
      const response = await fetch('/' + mode, {
        method: 'POST', body, credentials: 'same-origin', cache: 'no-store',
        redirect: 'error', headers: dialogHeaders, referrerPolicy: 'same-origin',
        signal: AbortSignal.timeout(45000)
      });
      if (response.status === 204 && response.headers.get('x-tor-auth-dialog') === 'success') {
        // The server has issued the ordinary session cookie. Re-read the same
        // page with its full URL and fresh server-rendered access decisions.
        location.reload();
        return;
      }
      const doc = await readDocument(response);
      const message = doc.querySelector('.auth-form__error, .account-error__message')?.textContent?.trim();
      if (response.status === 403) {
        await loadForm(mode, saved);
      } else {
        form.querySelector('input[type="password"]').value = '';
      }
      showError(message || 'Не удалось завершить действие. Проверьте данные и повторите попытку.');
    } catch (_) {
      form.querySelector('input[type="password"]').value = '';
      showError('Не удалось подтвердить результат. Обновите страницу или откройте отдельную страницу входа.');
    } finally {
      status.textContent = '';
      submitting = false;
      setBusy(false);
    }
  });

  document.addEventListener('click', event => {
    if (event.defaultPrevented || event.button !== 0 || event.ctrlKey || event.metaKey ||
        event.shiftKey || event.altKey || busy || dialog.open) return;
    const link = event.target.closest?.('a[href]');
    if (!link || link.hasAttribute('download') || (link.target && link.target !== '_self')) return;
    const url = new URL(link.href, location.href);
    if (url.origin !== location.origin || !['/login', '/register'].includes(url.pathname)) return;
    if (url.hash || [...url.searchParams.keys()].some(key => key !== 'next')) return;
    // Links to an explicitly different protected destination retain native navigation.
    if (url.searchParams.has('next') && url.searchParams.get('next') !== location.pathname) return;
    event.preventDefault();
    opener = link;
    dialog.showModal();
    changeMode(url.pathname.slice(1));
  });
})();
