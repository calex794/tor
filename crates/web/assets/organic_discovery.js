'use strict';
// Optional, first-party, consent-gated navigation receipt. A failed analytics
// request never prevents normal reading or exposes a ticket in the book URL.
(() => {
  const meta = document.querySelector('[data-organic-csrf]');
  if (!meta) return;
  const csrf = meta.getAttribute('data-organic-csrf');
  if (!csrf) return;
  document.addEventListener('click', event => {
    const link = event.target?.closest?.('a.book-mini-card__link[data-organic-ticket]');
    if (!link || event.defaultPrevented || event.button !== 0 ||
        event.altKey || event.ctrlKey || event.metaKey || event.shiftKey ||
        link.target === '_blank' || link.hasAttribute('download')) return;
    const ticket = link.getAttribute('data-organic-ticket');
    const href = link.getAttribute('href');
    if (!ticket || !/^\/books\/[1-9]\d*$/.test(href || '')) return;
    event.preventDefault();
    let navigated = false;
    const navigate = () => {
      if (navigated) return;
      navigated = true;
      window.location.assign(href);
    };
    const payload = new URLSearchParams({ticket, csrf_token: csrf});
    const deadline = window.setTimeout(navigate, 500);
    fetch('/api/v1/discovery/activate', {
      method: 'POST',
      credentials: 'same-origin',
      keepalive: true,
      headers: {'Content-Type': 'application/x-www-form-urlencoded'},
      body: payload,
    }).catch(() => {}).finally(() => {
      window.clearTimeout(deadline);
      navigate();
    });
  });
})();
