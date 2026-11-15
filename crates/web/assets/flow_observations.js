/* Independent short-lived card observations. No storage, cookies, account IDs or history. */
(() => {
  'use strict';
  if (!('IntersectionObserver' in window)) return;
  const states = new Map();
  const endpoint = '/api/v1/flow/observations';
  function emit(state, kind, details = {}) {
    if (state.sent.has(kind)) return;
    state.sent.add(kind);
    const body = JSON.stringify({proof: state.proof, kind, ...details});
    async function attempt(retry) {
      try {
        const response = await fetch(endpoint, {
          method: 'POST', credentials: 'same-origin', cache: 'no-store',
          referrerPolicy: 'no-referrer', keepalive: true,
          headers: {'Content-Type': 'application/json', 'X-Tor-Observation': '1'}, body
        });
        if ((response.status === 425 || response.status === 503) && retry < 3) {
          setTimeout(() => attempt(retry + 1), [150, 400, 1200][retry]);
        }
      } catch (_) { /* An absent signal is unknown. Navigation never waits for analytics. */ }
    }
    void attempt(0);
  }
  function reset(state) {
    clearTimeout(state.timer); state.timer = null; state.since = null;
  }
  function update(state) {
    if (document.visibilityState !== 'visible' || state.ratio < 0.6 || !state.element.isConnected) {
      reset(state); return;
    }
    if (state.timer || state.sent.has('view')) return;
    state.since = performance.now();
    state.timer = setTimeout(() => {
      state.timer = null;
      const duration = performance.now() - state.since;
      if (document.visibilityState === 'visible' && state.ratio >= 0.6 && state.element.isConnected && duration >= 1000) {
        emit(state, 'view', {visible_percent: Math.floor(state.ratio * 100), continuous_visible_ms: Math.min(900000, Math.floor(duration)), foreground: true});
      }
    }, 1010);
  }
  const observer = new IntersectionObserver(entries => {
    for (const entry of entries) {
      const state = states.get(entry.target); if (!state) continue;
      state.ratio = entry.intersectionRatio; update(state);
    }
  }, {threshold: [0, 0.6, 1]});
  for (const element of Array.from(document.querySelectorAll('[data-flow-proof]')).slice(0, 128)) {
    const proof = element.getAttribute('data-flow-proof');
    if (!/^[A-Za-z0-9_-]{43}$/.test(proof || '')) continue;
    const state = {element, proof, ratio: 0, timer: null, since: null, sent: new Set()};
    states.set(element, state); emit(state, 'rendered'); observer.observe(element);
    element.addEventListener('click', event => {
      const link = event.target instanceof Element ? event.target.closest('a[data-flow-link]') : null;
      if (link && element.contains(link)) emit(state, 'click');
    });
  }
  document.addEventListener('visibilitychange', () => {
    for (const state of states.values()) { reset(state); update(state); }
  });
  window.addEventListener('pagehide', () => { for (const state of states.values()) reset(state); });
})();
