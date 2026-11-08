'use strict';
// An IntersectionObserver report is a CLIENT CLAIM, not evidence of reading.
(() => {
  if (window.__torRecommendationsMounted) return;
  window.__torRecommendationsMounted = true;
  const pending = new Map();
  const timers = new Map();
  const visibility = new Map();
  const sent = new WeakSet();
  let flushTimer = null;
  function csrf(card) { return card.closest('[data-rec-panel]')?.dataset.recCsrf || ''; }
  async function flush() {
    flushTimer = null;
    const entries = Array.from(pending.entries()).slice(0, 20);
    if (!entries.length) return;
    entries.forEach(([card]) => pending.delete(card));
    try {
      const response = await fetch('/api/v1/recommendations/impressions', {
        method: 'POST', credentials: 'same-origin', keepalive: true,
        headers: {'Content-Type': 'application/json', 'X-CSRF-Token': csrf(entries[0][0])},
        body: JSON.stringify({items: entries.map(([, claim]) => claim)})
      });
      if (response.ok) entries.forEach(([card]) => sent.add(card));
      // Missing telemetry remains unknown. No hot retry loop or fake view.
    } catch (_) { /* Analytics must not affect navigation. */ }
    if (pending.size) flushTimer = setTimeout(flush, 50);
  }
  function stop(card) {
    const timer = timers.get(card);
    if (timer !== undefined) clearTimeout(timer);
    timers.delete(card);
  }
  function assess(card) {
    stop(card);
    if (sent.has(card) || pending.has(card) || document.visibilityState !== 'visible' || (visibility.get(card) || 0) < 0.5) return;
    timers.set(card, setTimeout(() => {
      timers.delete(card);
      if (document.visibilityState !== 'visible' || (visibility.get(card) || 0) < 0.5 || sent.has(card)) return;
      pending.set(card, {ticket: card.dataset.recTicket, visible_fraction_per_mille: Math.floor((visibility.get(card) || 0) * 1000), continuous_visible_ms: 1000, document_visible: true});
      if (!flushTimer) flushTimer = setTimeout(flush, 50);
    }, 1000));
  }
  if ('IntersectionObserver' in window) {
    const observer = new IntersectionObserver(entries => {
      for (const entry of entries) {
        visibility.set(entry.target, entry.isIntersecting ? entry.intersectionRatio : 0);
        assess(entry.target);
      }
    }, {threshold: [0, 0.5, 1]});
    document.querySelectorAll('[data-rec-card][data-rec-ticket]').forEach(card => observer.observe(card));
    document.addEventListener('visibilitychange', () => visibility.forEach((_, card) => assess(card)));
    window.addEventListener('pagehide', () => timers.forEach((_, card) => stop(card)));
  }
  document.addEventListener('click', async event => {
    const anchor = event.target.closest?.('a[data-rec-link]');
    const card = anchor?.closest('[data-rec-card][data-rec-ticket]');
    if (!card || event.defaultPrevented || event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    const target = new URL(anchor.href, location.href);
    if (target.origin !== location.origin || !/^\/books\/[1-9][0-9]*$/.test(target.pathname)) return;
    event.preventDefault();
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 1500);
    try {
      await fetch('/api/v1/recommendations/activate', {
        method: 'POST', credentials: 'same-origin', signal: controller.signal,
        headers: {'Content-Type': 'application/x-www-form-urlencoded'},
        body: new URLSearchParams({ticket: card.dataset.recTicket, csrf_token: csrf(card)})
      });
    } catch (_) { /* Preserve ordinary canonical navigation on timeout/denial. */ }
    finally { clearTimeout(timeout); location.assign(target.pathname); }
  });
})();
