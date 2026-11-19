// UI-05 strict order and existing-node/observation preservation.
const test = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname, '../crates/web/assets/tor-home-order.js'), 'utf8');
const IDS = ['latest', 'releases', 'editorial', 'rising', 'genres'];
function fixture(saved, missing = []) {
  const handlers = {};
  const windowHandlers = {};
  const calls = [];
  let stored = saved;
  function node(attributes = {}) {
    const listeners = {};
    return {
      attributes, hidden: false, children: [],
      getAttribute(name) { return attributes[name] ?? null; },
      hasAttribute(name) { return Object.hasOwn(attributes, name); },
      querySelector(name) {
        if (name === 'section') return this.section || null;
        return this.buttons?.[name] || null;
      },
      addEventListener(name, callback) { listeners[name] = callback; },
      contains(target) { return !!target; }, listeners,
      appendChild(child) {
        this.children = this.children.filter(x => x !== child);
        this.children.push(child);
        calls.push(child);
      }
    };
  }
  const container = node();
  const list = node();
  const picker = node();
  picker.querySelector = name => name === '[data-tor-home-order-list]' ? list : null;
  const blocks = IDS.map(id => {
    const item = node({ 'data-tor-home-block': id });
    item.section = missing.includes(id) ? null : node();
    item.proof = 'proof-' + id;
    return item;
  });
  const rows = IDS.map(id => {
    const item = node({ 'data-tor-home-order-item': id });
    item.buttons = {
      '[data-tor-home-direction="up"]': { disabled: false },
      '[data-tor-home-direction="down"]': { disabled: false }
    };
    return item;
  });
  container.children = blocks.slice();
  list.children = rows.slice();
  const document = {
    readyState: 'loading',
    querySelector(selector) {
      if (selector === '[data-tor-home-blocks]') return container;
      if (selector === '[data-tor-home-order]') return picker;
      return null;
    },
    addEventListener(name, callback) { handlers[name] = callback; }
  };
  const window = {
    localStorage: {
      getItem() { return stored; },
      setItem(_, value) { stored = value; }
    },
    addEventListener(name, callback) { windowHandlers[name] = callback; }
  };
  vm.runInNewContext(source, { window, document });
  handlers.DOMContentLoaded();
  function click(id, direction) {
    const button = {
      getAttribute(name) { return name === 'data-tor-home-id' ? id : direction; },
      hasAttribute() { return false; }
    };
    picker.listeners.click({target: {closest() { return button; }}});
  }
  return {
    container, list, picker, blocks, rows, calls, click,
    getStored: () => stored, windowHandlers,
    ids: () => container.children.map(el => el.getAttribute('data-tor-home-block'))
  };
}
test('validated persisted order moves the exact SSR nodes with proofs intact', () => {
  const order = ['genres', 'latest', 'editorial', 'rising', 'releases'];
  const f = fixture(JSON.stringify(order));
  assert.deepEqual(f.ids(), order);
  assert.deepEqual(f.container.children.slice().sort((a,b) =>
    IDS.indexOf(a.getAttribute('data-tor-home-block')) - IDS.indexOf(b.getAttribute('data-tor-home-block'))), f.blocks);
  for (const block of f.blocks) assert.equal(block.proof, 'proof-' + block.getAttribute('data-tor-home-block'));
});
test('tampered and truncated orders fall back to SSR', () => {
  for (const bad of ['{', 'null', '{}', '["latest"]', JSON.stringify([...IDS.slice(1), 'evil']), JSON.stringify(['latest','latest','editorial','rising','genres'])]) {
    assert.deepEqual(fixture(bad).ids(), IDS);
  }
});
test('up/down moves only visible shelves, preserving hidden and storage', () => {
  const f = fixture(null, ['editorial', 'rising']);
  assert.equal(f.picker.hidden, false);
  assert.equal(f.rows[2].hidden, true);
  assert.equal(f.rows[3].hidden, true);
  f.click('genres', 'up');
  assert.deepEqual(f.ids(), ['latest','genres','editorial','rising','releases']);
  assert.deepEqual(JSON.parse(f.getStored()), ['latest','genres','editorial','rising','releases']);
  f.click('latest', 'up');
  assert.deepEqual(f.ids(), ['latest','genres','editorial','rising','releases']);
  f.click('rising', 'up');
  assert.deepEqual(f.ids(), ['latest','genres','editorial','rising','releases']);
});
test('storage-event synchronizes without creating or deleting any card', () => {
  const f = fixture(null);
  assert.equal(f.picker.hidden, false);
  const initial = f.blocks.slice();
  f.click('releases','up');
  assert.deepEqual(f.ids().slice(0,2), ['releases','latest']);
  assert.deepEqual(f.blocks, initial);
  f.windowHandlers.storage({key:'other'});
  assert.deepEqual(f.ids().slice(0,2), ['releases','latest']);
});
test('HTML and CSS wire the same allowlist without visual-only order', () => {
  const layout = fs.readFileSync(path.join(__dirname,'../crates/web/templates/layout.html'),'utf8');
  const home = fs.readFileSync(path.join(__dirname,'../crates/web/templates/home.html'),'utf8');
  const css = fs.readFileSync(path.join(__dirname,'../crates/web/assets/tor-appearance.css'),'utf8');
  assert(layout.includes('<script defer src="/assets/tor-home-order.js"></script>'));
  assert(home.includes('data-tor-home-blocks'));
  assert(home.indexOf('HOME_PROMO_STRIP') < home.indexOf('data-tor-home-blocks'));
  for (const id of IDS) {
    assert.equal(layout.split('data-tor-home-order-item="' + id + '"').length, 2);
    assert.equal(home.split('data-tor-home-block="' + id + '"').length, 2);
  }
  assert(!css.includes('.home-section { order:'));
});
