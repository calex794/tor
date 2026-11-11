'use strict';
// Actual administrative Chromium/HTTP on the disposable current-schema server.
const {chromium} = require('playwright');
const {execFileSync} = require('node:child_process');
const {randomBytes} = require('node:crypto');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const db = process.env.TEST_DATABASE_URL || '';
const base = process.env.TOR_SURPRISE_BASE || 'http://127.0.0.1:3108';
const shots = process.env.TOR_SURPRISE_SHOTS || '/tmp/tor-surprise-shots';
const parsed = new URL(db);
assert.equal(process.env.GITHUB_ACTIONS, 'true');
assert.equal(parsed.protocol, 'postgresql:');
assert.equal(parsed.hostname, '127.0.0.1');
assert.equal(parsed.port, '5432');
assert.equal(parsed.pathname, '/tor_ci');
assert.equal(base, 'http://127.0.0.1:3108');
fs.mkdirSync(shots, {recursive:true});
function sql(statement) {
  return execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1',
    '--dbname',db,'--command',statement], {
    encoding:'utf8', env:{...process.env,PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'},
  }).trim();
}
const literal = value => "'" + String(value).replace(/'/g,"''") + "'";
function scalar(statement) {
  const value = sql(statement);
  assert.match(value, /^\d+$/);
  return Number(value);
}
async function save(page) {
  const done = page.waitForResponse(response =>
    new URL(response.url()).pathname === '/admin/surprises/policy' &&
    response.request().method() === 'POST');
  await page.locator('form[action="/admin/surprises/policy"]')
    .getByRole('button', {name:'Сохранить настройки'}).click();
  assert.equal((await done).status(),303);
  await page.waitForLoadState('networkidle');
}
(async () => {
  let browser;
  let user;
  let run;
  try {
    assert.equal(sql('SELECT enabled::text FROM surprise_runtime_policy WHERE singleton'),'false');
    browser = await chromium.launch({headless:true});
    const context = await browser.newContext({viewport:{width:1440,height:1000}});
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    const email = `surprise-report-${Date.now()}-${randomBytes(4).toString('hex')}@tor-ci.example`;
    assert.equal((await page.goto(base+'/register')).status(),200);
    await page.getByLabel('Отображаемое имя').fill('Experiment report QA');
    await page.getByLabel('Электронная почта').fill(email);
    await page.getByLabel('Пароль (12–128 символов)').fill('SurpriseExperimentBrowserPass2026');
    await page.getByRole('button',{name:'Создать учётную запись'}).click();
    await page.waitForURL('**/me');
    user = scalar(`SELECT id FROM users WHERE email_normalized=${literal(email)}`);
    assert.equal((await page.goto(base+'/admin/surprises')).status(),403);
    sql(`INSERT INTO account_roles(user_id,role,is_active) VALUES(${user},'administrator',true)`);
    const response = await page.goto(base+'/admin/surprises');
    assert.equal(response.status(),200);
    assert.equal(response.headers()['cache-control'],'no-store');
    assert.equal(await page.getByRole('heading',{name:'Эксперимент: подтверждённое чтение'}).count(),1);
    const policy = page.locator('form[action="/admin/surprises/policy"]');
    await policy.locator('[name=enabled]').check();
    await policy.locator('[name=experiment_enabled]').check();
    await policy.locator('[name=probability_per_mille]').fill('1000');
    await save(page);
    run = scalar('SELECT experiment_run_id FROM surprise_runtime_policy WHERE singleton');
    assert.ok((await page.locator('main').innerText()).includes('Значения скрыты'));
    assert.ok((await page.locator('[data-surprise-experiment]').innerText()).includes('вывод об эффекте запрещён'));
    await page.screenshot({path:path.join(shots,'surprise-experiment-insufficient.png'),fullPage:true});

    // Anonymous synthetic receipts only in tor_ci, solely to exercise rendering.
    sql(`INSERT INTO surprise_experiment_participants(run_id,arm,enrolled_at,meaningful_new_book)
      SELECT ${run},arm,now()-interval '216 hours',i<=5
      FROM unnest(ARRAY['control','gentle','standard']) arm CROSS JOIN generate_series(1,20) i;
      SELECT surprise_refresh_totals(${run})`);
    assert.equal((await page.reload()).status(),200);
    assert.equal(await page.getByRole('cell',{name:'5/20 (25.0%)',exact:true}).count(),3);
    assert.ok((await page.locator('main').innerText()).includes('1000 участников'));
    assert.ok(!(await page.locator('main').innerText()).includes('Положительный результат подтверждён'));
    await page.screenshot({path:path.join(shots,'surprise-experiment-desktop.png'),fullPage:true});
    await page.setViewportSize({width:390,height:844});
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth>innerWidth+1),false);
    await page.screenshot({path:path.join(shots,'surprise-experiment-mobile.png'),fullPage:true});

    const csrf = await policy.locator('[name=csrf_token]').inputValue();
    const injected = await context.request.post(base+'/admin/surprises/policy',{
      headers:{origin:base}, form:{csrf_token:csrf,enabled:'on',experiment_enabled:'on',
        probability_per_mille:'1000',experiment_arm:'control'},
    });
    assert.equal(injected.status(),400,'a browser cannot supply an assignment');
    await policy.locator('[name=experiment_enabled]').uncheck();
    await save(page);
    assert.equal(sql('SELECT enabled::text FROM surprise_runtime_policy WHERE singleton'),'false');
    assert.equal(sql(`SELECT decision FROM surprise_experiment_runs WHERE id=${run}`),'operator_stopped');
    assert.equal(await policy.locator('[name=enabled]').isChecked(),false);
    assert.ok((await page.locator('[data-surprise-experiment]').innerText()).includes('Остановлен администратором'));
    sql(`UPDATE account_roles SET is_active=false WHERE user_id=${user} AND role='administrator'`);
    assert.equal((await page.goto(base+'/admin/surprises')).status(),403);
    assert.deepEqual(errors,[]);
    console.log('F_SURPRISE_04B_BROWSER_SUCCESS desktop mobile suppression ITT report stop role-revocation');
  } finally {
    if (browser) await browser.close();
    sql('UPDATE surprise_runtime_policy SET enabled=false,experiment_enabled=false WHERE singleton');
    if (run) sql(`UPDATE surprise_runtime_policy SET experiment_run_id=NULL WHERE experiment_run_id=${run};
      DELETE FROM surprise_experiment_participants WHERE run_id=${run};
      DELETE FROM surprise_experiment_totals WHERE run_id=${run};
      DELETE FROM surprise_experiment_runs WHERE id=${run}`);
    if (user) sql(`DELETE FROM surprise_admin_events WHERE actor_user_id=${user};
      DELETE FROM account_roles WHERE user_id=${user}; DELETE FROM users WHERE id=${user}`);
  }
})().catch(error => {console.error(error);process.exit(1);});
