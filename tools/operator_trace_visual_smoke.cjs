'use strict';
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');

const origin = 'http://127.0.0.1:3099';
const password = process.env.TOR_TEST_OPERATOR_PASSWORD;
if (!password || password.length < 32) throw new Error('missing test-only operator credential');

(async () => {
  const browser = await chromium.launch({ headless: true });
  const output = process.env.TOR_SCREENSHOT_DIR || 'screenshots';
  fs.mkdirSync(output, { recursive: true });
  try {
    const stranger = await browser.newContext();
    const denied = await stranger.request.get(origin + '/ops/traces');
    if (denied.status() !== 401) throw new Error('operator page opened to anonymous user');
    const anonymousSummary = await stranger.request.get(origin + '/ops/overview');
    if (anonymousSummary.status() !== 401) throw new Error('overview opened to anonymous user');
    const wrong = await stranger.request.get(origin + '/ops/traces', {
      headers: { authorization: 'Basic ' + Buffer.from('operator:wrong').toString('base64') },
    });
    if (wrong.status() !== 401) throw new Error('wrong operator credential accepted');
    const wrongSummary = await stranger.request.get(origin + '/ops/overview', {
      headers: { authorization: 'Basic ' + Buffer.from('operator:wrong').toString('base64') },
    });
    if (wrongSummary.status() !== 401) throw new Error('overview allowed wrong operator password');
    await stranger.close();

    const context = await browser.newContext({
      httpCredentials: { username: 'operator', password },
      viewport: { width: 1280, height: 900 },
    });
    const page = await context.newPage();
    let response = await page.goto(origin + '/ops/traces');
    if (!response || response.status() !== 200) throw new Error('protected form inaccessible');
    if (!(await page.getByRole('heading', { name: 'Диагностика HTTP-запроса' }).isVisible()))
      throw new Error('operator page heading is not visible');
    response = await page.goto(origin + '/ops/overview');
    if (!response || response.status() !== 200) throw new Error('protected overview inaccessible');
    if (!(await page.getByRole('heading', { name: 'Состояние сайта TOR' }).isVisible()))
      throw new Error('overview heading not visible');
    const overviewText = await page.locator('main').innerText();
    for (const required of [
      'PostgreSQL', 'Не подключена (демонстрационный режим)', 'Ресурсы процесса TOR',
      'HTTP с запуска процесса', 'RIL: маршруты и потери', 'Очередь аналитики RIL',
      'Очередь обработки обложек', 'Доставка трасс', 'Нет измерения',
      'В этом процессе сборщик RIL не запускался', 'Живость отдельного исполнителя обложек',
    ]) {
      if (!overviewText.includes(required)) throw new Error('missing measured overview section: ' + required);
    }
    if (overviewText.includes(password) || overviewText.includes('SELECT *'))
      throw new Error('operator secrets or SQL leaked');
    for (const [label, width, height] of [['desktop', 1280, 900], ['mobile', 390, 844]]) {
      await page.setViewportSize({ width, height });
      const overflow = await page.evaluate(() =>
        document.documentElement.scrollWidth > document.documentElement.clientWidth + 1);
      if (overflow) throw new Error(label + ': overview horizontally overflows');
      await page.screenshot({ path: path.join(output, label + '-operator-overview.png'), fullPage: true });
    }
    await page.setViewportSize({ width: 1280, height: 900 });
    response = await page.goto(origin + '/ops/traces');
    if (!response || response.status() !== 200) throw new Error('trace page no longer reachable');

    const malformed = await page.goto(origin + '/ops/traces?id=%3Cscript%3E');
    if (!malformed || malformed.status() !== 400) throw new Error('invalid trace ID accepted');
    if ((await page.content()).includes('<script>')) throw new Error('untrusted content escaped incorrectly');

    // RIL request IDs use a numeric sequence. Every 16th completed fast 2xx
    // request is retained as one complete trace, then queued to the socket writer.
    let chosen = null;
    for (let i = 0; i < 64; i++) {
      const check = await page.request.get(origin + '/healthz');
      const id = check.headers()['x-request-id'];
      if (!check.ok() || !/^[0-9a-f]{32}$/.test(id)) throw new Error('missing server request ID');
      if (BigInt('0x' + id) % 16n === 0n) {
        chosen = id;
        break;
      }
    }
    if (!chosen) throw new Error('unable to find a deterministic sampled request');

    let saved = false;
    for (let attempt = 0; attempt < 40; attempt++) {
      response = await page.goto(origin + '/ops/traces?id=' + chosen);
      if (response && response.status() === 200) {
        saved = true;
        break;
      }
      if (!response || response.status() !== 404)
        throw new Error('unexpected trace response: ' + (response && response.status()));
      await new Promise(resolve => setTimeout(resolve, 150));
    }
    if (!saved) throw new Error('complete request trace did not reach standalone SQLite');
    if (!(await page.getByRole('heading', { name: 'Трасса ' + chosen }).isVisible()))
      throw new Error('requested trace not shown');
    const body = await page.locator('main').innerText();
    if (!body.includes('SQLx не измерялась')) throw new Error('unknown SQLx state silently labeled as zero');
    if (body.includes('user_id') || body.includes('password='))
      throw new Error('sensitive values leaked into operator page');

    for (const [name, width, height] of [['desktop', 1280, 900], ['mobile', 390, 844]]) {
      await page.setViewportSize({ width, height });
      const overflow = await page.evaluate(() =>
        document.documentElement.scrollWidth > document.documentElement.clientWidth + 1);
      if (overflow) throw new Error(name + ': diagnostics page overflows horizontally');
      await page.screenshot({ path: path.join(output, name + '-operator-trace.png'), fullPage: true });
    }

    response = await page.goto(origin + '/ops/traces?id=ffffffffffffffffffffffffffffffff');
    if (!response || response.status() !== 404)
      throw new Error('missing saved trace must return truthful 404');
    if (!(await page.getByText('Запись не обнаружена').isVisible()))
      throw new Error('missing trace explanation not visible');

    await context.close();
    console.log('OBS-04/05 browser PASS: protected overview and traces, isolated SQLite, 404 and responsive layout');
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
