/* UI-05: homepage-only DOM ordering. Never clone cards or manufacture observations. */
(() => {
  'use strict';
  const KEY = 'torHomeOrder.v1';
  const IDS = Object.freeze(['latest', 'releases', 'editorial', 'rising', 'genres']);
  const VALID = new Set(IDS);

  function normalize(value) {
    if (!Array.isArray(value) || value.length !== IDS.length ||
        new Set(value).size !== IDS.length || !value.every(id => VALID.has(id))) {
      return IDS.slice();
    }
    return value.slice();
  }
  function read() {
    try { return normalize(JSON.parse(window.localStorage.getItem(KEY))); }
    catch (_) { return IDS.slice(); }
  }
  function write(order) {
    try { window.localStorage.setItem(KEY, JSON.stringify(order)); }
    catch (_) { /* A blocked preference store must never break reading. */ }
  }
  function init() {
    const container = document.querySelector('[data-tor-home-blocks]');
    const picker = document.querySelector('[data-tor-home-order]');
    const list = picker?.querySelector('[data-tor-home-order-list]');
    if (!container || !picker || !list) return;

    const blocks = new Map();
    for (const child of Array.from(container.children)) {
      const id = child.getAttribute('data-tor-home-block');
      if (!VALID.has(id) || blocks.has(id)) return; // malformed shell: leave SSR unchanged
      blocks.set(id, child);
    }
    if (blocks.size !== IDS.length) return;
    const items = new Map();
    for (const item of Array.from(list.children)) {
      const id = item.getAttribute('data-tor-home-order-item');
      if (!VALID.has(id) || items.has(id)) return;
      items.set(id, item);
    }
    if (items.size !== IDS.length) return;

    // Empty or disabled shelves are not user controls. Their wrappers stay
    // intact and the canonical SSR order remains the fallback without JS.
    const available = id => blocks.get(id).querySelector('section') !== null;
    let order = read();

    function apply() {
      const active = order.filter(available);
      // Only already-generated DOM nodes move: exact anchors, per-response
      // opaque proofs and the same card elements survive without cloning.
      for (const id of order) container.appendChild(blocks.get(id));
      for (const id of order) {
        const item = items.get(id);
        item.hidden = !available(id);
        list.appendChild(item);
        const at = active.indexOf(id);
        const up = item.querySelector('[data-tor-home-direction="up"]');
        const down = item.querySelector('[data-tor-home-direction="down"]');
        if (up) up.disabled = at <= 0;
        if (down) down.disabled = at < 0 || at === active.length - 1;
      }
      picker.hidden = active.length < 2;
    }

    picker.addEventListener('click', event => {
      const target = event.target.closest?.('button[data-tor-home-id][data-tor-home-direction],button[data-tor-home-reset]');
      if (!target || !picker.contains(target)) return;
      if (target.hasAttribute('data-tor-home-reset')) {
        order = IDS.slice();
      } else {
        const id = target.getAttribute('data-tor-home-id');
        const direction = target.getAttribute('data-tor-home-direction');
        if (!VALID.has(id) || !available(id) || !['up', 'down'].includes(direction)) return;
        const active = order.filter(available);
        const index = active.indexOf(id);
        const other = active[index + (direction === 'up' ? -1 : 1)];
        if (!other) return;
        const a = order.indexOf(id), b = order.indexOf(other);
        [order[a], order[b]] = [order[b], order[a]];
      }
      apply();
      // Reparenting the settings row can drop keyboard focus. Restore it
      // to the exact control the reader activated, without jumping the page.
      // At the new boundary the Up/Down button may become disabled.
      // Keep focus inside the same row on the still-operable direction.
      const focusControl = target.disabled
        ? target.closest?.('[data-tor-home-order-item]')?.querySelector('button:not(:disabled)')
        : target;
      focusControl?.focus?.({ preventScroll: true });
      write(order);
    });
    window.addEventListener('storage', event => {
      if (event.key !== KEY) return;
      order = read();
      apply();
    });
    apply();
  }
  // The deferred script runs before DOMContentLoaded, so rearrangement
  // precedes the independently deferred flow impression observer.
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
  else init();
})();
