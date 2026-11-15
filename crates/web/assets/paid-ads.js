/* Paid ad viewability is separate from FLOW observations and reading history. */
(() => {
  'use strict';
  if (window.__torPaidAds) return;
  window.__torPaidAds = true;
  for (const form of document.querySelectorAll('[data-ads-daily]')) {
    const offer = form.elements.offer, days = form.elements.days, date = form.elements.start_date;
    const today = new Date().toISOString().slice(0, 10);
    date.min = today;
    if (!date.value) date.value = today;
    const update = () => {
      const rate = offer.selectedOptions[0]?.dataset.rateMinor;
      if (!rate || !/^[1-9][0-9]*$/.test(days.value) || Number(days.value) > 366) return;
      const amount = BigInt(rate) * BigInt(days.value);
      form.querySelector('[data-ads-total]').textContent = `К оплате: ${amount / 100n}.${String(amount % 100n).padStart(2, '0')} ₽`;
    };
    offer.addEventListener('change', update);
    days.addEventListener('input', update);
    update();
  }
  for (const form of document.querySelectorAll('[data-ads-admin-cancel]')) {
    form.addEventListener('submit', event => {
      const product = form.querySelector('[data-ads-product]').value;
      const id = form.querySelector('[data-ads-id]').value;
      if (!['daily', 'chapter'].includes(product) || !/^[1-9][0-9]{0,18}$/.test(id)) {
        event.preventDefault(); return;
      }
      form.action = `/admin/promotion/paid/${product}/${id}/cancel`;
    });
  }
  const csrf = document.querySelector('[data-free-progress-csrf]')?.dataset.freeProgressCsrf
    || document.querySelector('input[name="csrf_token"]')?.value;
  if (!csrf || !('IntersectionObserver' in window)) return;
  const post = (path, body) => fetch(path, {
    method: 'POST', credentials: 'same-origin', cache: 'no-store', keepalive: true,
    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': csrf }, body: JSON.stringify(body)
  });
  for (const card of document.querySelectorAll('[data-paid-ad-token]')) {
    const token = card.dataset.paidAdToken;
    if (!/^[A-Za-z0-9_-]{43}$/.test(token)) continue;
    let ratio = 0, since = 0, timer = null, done = false, busy = false, retries = 0;
    const reset = () => { clearTimeout(timer); timer = null; since = 0; };
    const visible = () => document.visibilityState === 'visible' && document.hasFocus() && ratio >= .6;
    const send = async () => {
      if (done || busy || !visible() || !since) return;
      const elapsed = Math.floor(performance.now() - since);
      if (elapsed < 1000) { timer = setTimeout(send, 1000 - elapsed + 10); return; }
      busy = true;
      try {
        const response = await post('/api/v1/ads/chapter/impressions', {
          campaign_id: card.dataset.paidCampaign, generation_id: card.dataset.paidGeneration, assignment_id: card.dataset.paidAssignment,
          viewability_token: token, observed_visible_fraction: ratio, observed_duration_ms: elapsed
        });
        if (response.ok) { done = true; card.dataset.paidAdAccepted = 'true'; }
        else if (response.status < 500) done = true;
      } catch (_) { /* Retry the same opaque opportunity; never create another charge. */ }
      finally {
        busy = false;
        if (!done && visible() && retries++ < 3) timer = setTimeout(send, 1500);
      }
    };
    const update = () => {
      if (!visible()) { reset(); return; }
      if (!since && !done) { since = performance.now(); timer = setTimeout(send, 1020); }
    };
    const observer = new IntersectionObserver(entries => {
      ratio = entries[0].isIntersecting ? entries[0].intersectionRatio : 0;
      update();
    }, { threshold: [0, .59, .6, 1] });
    observer.observe(card);
    document.addEventListener('visibilitychange', update);
    window.addEventListener('blur', update);
    window.addEventListener('focus', update);
    card.addEventListener('click', async event => {
      const link = event.target.closest('a[href]');
      if (!event.isTrusted || !link || event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
      event.preventDefault();
      const destination = link.href;
      try {
        await Promise.race([
          post('/api/v1/ads/chapter/click', { viewability_token: token }),
          new Promise(resolve => setTimeout(resolve, 1500))
        ]);
      } catch (_) { /* Navigation still works without optional attribution. */ }
      window.location.assign(destination);
    });
  }
})();
