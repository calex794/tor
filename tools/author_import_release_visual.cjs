// Shared visual checks run on live, owner-only import/release pages.
const path = require('node:path');
function assert(ok, reason) { if (!ok) throw new Error(reason); }

async function inspectImportRelease(page, viewport, width, height, kind, out) {
  const root = page.locator('.author-' + kind);
  assert(await root.count() === 1, kind + ': shared workspace missing');
  assert(await root.locator('.account-sidebar').count() === 1, kind + ': account navigation missing');
  assert(await root.locator('.dashboard-nav a[href="/me/books"][aria-current="location"]').count() === 1,
    kind + ': wrong parent navigation');
  const tokens = await page.locator('input[name="csrf_token"], #chapter-import-csrf')
    .evaluateAll(nodes => nodes.map(node => node.value));
  assert(tokens.length >= (kind === 'import' ? 2 : 3) && tokens[0]
    && tokens.every(token => token === tokens[0]), kind + ': page actions disagree on CSRF');
  const state = () => root.locator('input, select, textarea').evaluateAll(nodes =>
    JSON.stringify(nodes.map(node => [node.id || node.name, node.value, node.disabled, node.hidden])));
  const before = await state();
  if (kind === 'import') {
    const details = root.locator('details').first();
    const summary = details.locator('summary');
    assert(await details.getAttribute('open') === null, 'format help must start collapsed');
    await summary.focus();
    await page.keyboard.press('Enter');
    assert(await details.getAttribute('open') !== null, 'format help cannot open by keyboard');
    await page.keyboard.press('Enter');
    assert(await details.getAttribute('open') === null, 'format help cannot close by keyboard');
    assert(await page.locator('#chapter-import-cancel').isHidden(), 'idle import exposes job cancellation');
  }
  for (const size of viewport === 'desktop' ? [1440, 1024, 768, 390] : [390]) {
    await page.setViewportSize({ width: size, height: 844 });
    for (const colorScheme of ['light', 'dark']) {
      await page.emulateMedia({ colorScheme });
      await page.evaluate(() => window.scrollTo({ top: 0, behavior: 'instant' }));
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      const layout = await root.evaluate(node => {
        const panel = node.querySelector('.author-editor__panel');
        const title = node.querySelector('h1').getBoundingClientRect();
        const control = node.querySelector('input[type="file"], input[type="datetime-local"]');
        return {
          overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
          titleBottom: title.bottom,
          panelColor: getComputedStyle(panel).backgroundColor,
          surface: getComputedStyle(node).getPropertyValue('--bg-surface').trim(),
          controlHeight: control.getBoundingClientRect().height,
          controlWidth: control.getBoundingClientRect().width,
        };
      });
      assert(!layout.overflow, kind + '/' + size + '/' + colorScheme + ': horizontal overflow');
      assert(layout.controlHeight >= 44 && layout.controlWidth > 100, kind + ': unusable control size');
      // Browser normalizes computed colors, so resolve the actual design token in the same engine.
      const surface = await root.evaluate(node => {
        const probe = document.createElement('span');
        probe.style.background = 'var(--bg-surface)'; node.append(probe);
        const color = getComputedStyle(probe).backgroundColor; probe.remove(); return color;
      });
      assert(layout.panelColor === surface, kind + ': legacy CSS replaced shared surface');
      if (size === 390) {
        assert(layout.titleBottom <= 844, kind + ': mobile title pushed below first screen');
        if (kind === 'import') {
          const file = await page.locator('#chapter-import-file').boundingBox();
          assert(file.y >= 0 && file.y + file.height <= 844, 'file picker is below first mobile screen');
        }
      }
      await page.screenshot({
        path: path.join(out, 'author-' + viewport + '-' + size + '-' + colorScheme + '-' + kind + '.png'),
        fullPage: true,
      });
    }
  }
  assert(await state() === before, kind + ': viewport/theme change modified form state');
  await page.setViewportSize({ width, height });
  await page.emulateMedia({ colorScheme: 'light' });
  const scrollable = page.locator(kind === 'import' ? '#chapter-import-results' : '.author-release-preview');
  await scrollable.focus();
  assert(await scrollable.evaluate(node => document.activeElement === node), kind + ': preview cannot receive keyboard focus');
  assert(await scrollable.evaluate(node => getComputedStyle(node).outlineStyle !== 'none'),
    kind + ': preview has no visible focus indicator');
  if (kind === 'release') {
    assert(await scrollable.evaluate(node => node.scrollHeight > node.clientHeight),
      'long release preview is not bounded');
    await page.keyboard.press('End');
    await scrollable.evaluate(node => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    assert(await scrollable.evaluate(node => node.scrollTop > 0), 'release preview cannot scroll by keyboard');
    const schedule = root.locator('form[action$="/schedule"]');
    assert(await schedule.evaluate(form => !form.checkValidity()), 'empty UTC schedule bypasses required validation');
    assert(await root.locator('form[action$="/release"] input[name="version_id"]').inputValue()
      === await schedule.locator('input[name="version_id"]').inputValue(), 'release/schedule pin different editions');
  }
  await page.evaluate(() => window.scrollTo({ top: 0, behavior: 'instant' }));
}
module.exports = { inspectImportRelease };
