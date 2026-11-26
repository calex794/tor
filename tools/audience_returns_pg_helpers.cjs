'use strict';
// Historical returns fixtures; callers retain their own disposable-loopback guards,
// registration, independent report assertions and cleanup. Both original SQL
// timeout policies and the group-return AJAX URL assertion are explicit.
const {execFileSync} = require('node:child_process');
const assert = require('node:assert/strict');

module.exports = function returnsHelpers({db,base,boundedSql,requireReaderUrl}) {
  assert.equal(typeof boundedSql, 'boolean');
  assert.equal(typeof requireReaderUrl, 'boolean');
function sql(statement) {
  return execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1',
    '--dbname',db,'--command',statement], {
    encoding:'utf8', ...(boundedSql ? {timeout:15000} : {}),
    env:{...process.env, ...(boundedSql ? {PGCONNECT_TIMEOUT:'5'} : {}),
      PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'},
  }).trim();
}

function scalar(statement) {
  const raw = sql(statement);
  assert.match(raw, /^\d+$/, 'numeric database evidence');
  return Number(raw);
}

async function csrf(page) {
  const value = (await page.context().cookies()).find(cookie => cookie.name === 'tor-dev-csrf')?.value;
  assert.ok(value,'normal authenticated forms issue the CSRF cookie');
  return value;
}

async function privateResponse(response, status=200) {
  assert.equal(response.status(),status);
  const headers = await response.allHeaders();
  assert.equal(headers['cache-control'],'no-store');
  assert.match(headers['x-robots-tag'],/noindex/);
  assert.match(headers['x-robots-tag'],/nofollow/);
}

function chapter(book, number) {
  const chapter = scalar(`INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
    VALUES(${book},${number},'Return chapter ${number}','public','free',0) RETURNING id`);
  const version = scalar(`INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
    VALUES(${chapter},1,E'Readable first paragraph.\\n\\nSecond paragraph.') RETURNING id`);
  sql(`INSERT INTO chapter_publications(chapter_id,version_id) VALUES(${chapter},${version})`);
  return {book,chapter,version};
}

async function progress(page, fixture, offset, expected=200) {
  assert.equal((await page.goto(base+`/read/${fixture.chapter}`)).status(),200);
  assert.equal((await page.goto(base+`/reader/${fixture.chapter}`)).status(),200);
  await page.waitForFunction(() => document.querySelector('[data-free-progress-chapter]')?.dataset.freeProgressReady==='1');
  if (requireReaderUrl) {
    assert.equal(page.url(),base+`/reader/${fixture.chapter}`,'progress runs from the actual AJAX reader with its live session');
  }
  return page.evaluate(async ({chapter,version,offset,expected,csrf}) => {
    const state = await fetch('/api/v1/me/progress/free?chapter_id='+chapter, {
      credentials:'same-origin',cache:'no-store',
    });
    if (!state.ok) throw new Error('C position lookup rejected: '+state.status);
    const revision = (await state.json()).revision;
    const response = await fetch('/api/v1/me/progress/free', {
      method:'PUT',credentials:'same-origin',
      headers:{'Content-Type':'application/json','X-CSRF-Token':csrf},
      body:JSON.stringify({chapter_id:String(chapter),version_id:String(version),
        block_id:1,char_offset:offset,expected_revision:revision}),
    });
    if (response.status !== expected) {
      throw new Error(`C progress ${response.status}, expected ${expected}`);
    }
    return response.status;
  },{...fixture,offset,expected,csrf:await csrf(page)});
}

async function metric(locator,key) {
  return (await locator.locator(`[data-metric='${key}']`).innerText()).trim();
}
  return {sql,scalar,csrf,privateResponse,chapter,progress,metric};
};
