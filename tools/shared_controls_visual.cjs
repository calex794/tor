// H-01/UI-01: exercise the production author templates in the real page shell.
// The fixture never submits forms or bypasses a server permission check.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const templates = path.join(__dirname, '../crates/web/templates');
const fixtures = [
  ['book', 'author_new_book.html', '#book-title', '#book-description'],
  ['chapter', 'author_new_chapter.html', '#chapter-title', '#chapter-body'],
];

async function inspect(page, input, textarea) {
  return page.evaluate(([input, textarea]) => {
    const style = selector => {
      const node = document.querySelector(selector);
      if (!node) return null;
      const s = getComputedStyle(node);
      return { color: s.color, background: s.backgroundColor, border: s.borderTopColor,
        radius: s.borderTopLeftRadius, outline: s.outlineWidth, outlineColor: s.outlineColor };
    };
    const token = name => {
      const probe = document.createElement('span');
      probe.style.color = `var(${name})`;
      document.body.append(probe);
      const value = getComputedStyle(probe).color;
      probe.remove();
      return value;
    };
    return {
      input: style(input), textarea: style(textarea), select: style('select'),
      button: style('.auth-form .button'), footer: style('.site-footer__link'),
      surface: token('--bg-surface'), text: token('--text-main'),
      border: token('--border-dark'), muted: token('--text-muted'),
      action: token('--action-primary-bg'), actionText: token('--action-primary-text'),
      scroll: document.documentElement.scrollWidth, width: innerWidth,
    };
  }, [input, textarea]);
}

function contrast(a, b) {
  const luminance = color => {
    const rgb = color.match(/[\d.]+/g).slice(0, 3).map(n => Number(n) / 255)
      .map(n => n <= 0.04045 ? n / 12.92 : ((n + 0.055) / 1.055) ** 2.4);
    return rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
  };
  const x = luminance(a), y = luminance(b);
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

async function checkSharedControls(browser, base, out) {
  for (const width of [390, 768, 1024, 1440]) {
    for (const colorScheme of ['light', 'dark']) {
      const context = await browser.newContext({
        viewport: { width, height: 900 }, colorScheme, bypassCSP: true,
      });
      try {
        const page = await context.newPage();
        await page.goto(base + '/catalog', { waitUntil: 'networkidle' });
        for (const [name, file, input, textarea] of fixtures) {
          const markup = fs.readFileSync(path.join(templates, file), 'utf8')
            .replace('<!--CHAPTER_HEADING-->', 'Новая бесплатная глава')
            .replace('<!--CHAPTER_INFO-->', 'Сохранение главы в существующей книге.')
            .replace('<!--SAVE_LABEL-->', 'Сохранить главу')
            .replaceAll('<!--BOOK_ID-->', '42').replace('<!--NEXT_NUMBER-->', '1');
          await page.locator('#main').evaluate((main, html) => { main.innerHTML = html; }, markup);
          const state = await inspect(page, input, textarea);
          const label = `${name}/${width}/${colorScheme}`;
          for (const field of ['input', 'textarea', 'select']) {
            // The chapter template has no select until access controls are enabled.
            if (name === 'chapter' && field === 'select') continue;
            assert.equal(state[field].background, state.surface, label + ': field surface');
            assert.equal(state[field].color, state.text, label + ': field text');
            assert.equal(state[field].border, state.border, label + ': field border');
          }
          assert.equal(state.footer.color, state.muted, label + ': legacy footer color leaked');
          assert.equal(state.button.background, state.action, label + ': action background');
          assert.equal(state.button.color, state.actionText, label + ': action text');
          assert.ok(contrast(state.button.color, state.button.background) >= 4.5,
            label + ': primary action has insufficient contrast');
          assert.ok(state.scroll <= state.width + 1, label + ': horizontal overflow');
          for (const selector of [input, textarea]) {
            await page.locator(selector).focus();
            const focus = await page.locator(selector).evaluate(node => {
              const css = getComputedStyle(node);
              return { width: parseFloat(css.outlineWidth), shadow: css.boxShadow };
            });
            assert.ok(focus.width >= 2 || focus.shadow !== 'none', label + ': missing field focus');
          }
          if (out && [390, 1440].includes(width)) {
            await page.locator(input).focus();
            await page.screenshot({ path: path.join(out,
              `shared-controls-${name}-${width}-${colorScheme}.png`), fullPage: true });
          }
        }
        await page.locator('#main').evaluate(main => {
          main.innerHTML = '<section class="section container"><h1>Проверка состояний</h1>' +
            '<button class="button button--secondary">Вторичное действие</button>' +
            '<button class="ui-button" disabled>Недоступное действие</button>' +
            '<label>Сообщение<textarea class="ui-textarea" rows="3"></textarea></label></section>';
        });
        const secondary = page.locator('.button--secondary');
        await page.mouse.move(0, 0);
        await secondary.hover();
        await page.waitForFunction(() => {
          const probe = document.createElement('span');
          probe.style.color = 'var(--bg-secondary)';
          document.body.append(probe);
          const expected = getComputedStyle(probe).color;
          probe.remove();
          return getComputedStyle(document.querySelector('.button--secondary')).backgroundColor === expected;
        });
        const disabled = page.locator('.ui-button');
        const disabledBefore = await disabled.evaluate(node => getComputedStyle(node).backgroundColor);
        await disabled.hover({ force: true });
        assert.equal(await disabled.evaluate(node => getComputedStyle(node).backgroundColor),
          disabledBefore, 'disabled action must not acquire an enabled hover');
      } finally {
        await context.close();
      }
    }
  }
  console.log('Shared controls PASS: real author templates, 4 widths, light/dark, focus and contrast');
}

module.exports = { checkSharedControls };
