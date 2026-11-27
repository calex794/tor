'use strict';
// Shared historical book/group/source browser fixtures. Callers retain their own
// disposable loopback checks, report journeys, assertions and cleanup.
const {execFileSync} = require('node:child_process');
const assert = require('node:assert/strict');

module.exports = function reportHelpers({db,base}) {
  function sql(statement) {
    return execFileSync('psql', ['-X','-A','-t','-q','-v','ON_ERROR_STOP=1',
      '--dbname',db,'--command',statement], {
      encoding:'utf8', env:{...process.env, PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000'},
    }).trim();
  }
  function scalar(statement) {
    const raw = sql(statement);
    assert.match(raw, /^\d+$/, 'expected numeric database evidence');
    return Number(raw);
  }
  async function csrf(page) {
    const value = (await page.context().cookies()).find(cookie => cookie.name === 'tor-dev-csrf')?.value;
    assert.ok(value, 'account forms must issue the normal CSRF cookie');
    return value;
  }
  async function privateResponse(response, status=200) {
    assert.equal(response.status(), status);
    const headers = await response.allHeaders();
    assert.equal(headers['cache-control'], 'no-store');
    assert.match(headers['x-robots-tag'], /noindex/);
    assert.match(headers['x-robots-tag'], /nofollow/);
  }
  function chapter(book, number) {
    const id = scalar(`INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
      VALUES(${book},${number},'Source chapter ${number}','public','free',0) RETURNING id`);
    const version = scalar(`INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
      VALUES(${id},1,'Readable text') RETURNING id`);
    sql(`INSERT INTO chapter_publications(chapter_id,version_id) VALUES(${id},${version})`);
    return {chapter:id, version};
  }
  async function read(page, fixture) {
    assert.equal((await page.goto(base+`/read/${fixture.chapter}`)).status(), 200);
    assert.equal((await page.goto(base+`/reader/${fixture.chapter}`)).status(), 200);
    await page.waitForFunction(() => document.querySelector('[data-free-progress-chapter]')?.dataset.freeProgressReady === '1');
    await page.evaluate(async ({chapter,version,csrf}) => {
      const state = await fetch('/api/v1/me/progress/free?chapter_id='+chapter, {
        credentials:'same-origin',
      });
      if (!state.ok) throw new Error('C position lookup rejected: '+state.status);
      const initialRevision = (await state.json()).revision;
      async function save(offset, revision, expected=200) {
        const response = await fetch('/api/v1/me/progress/free', {
          method:'PUT', credentials:'same-origin',
          headers:{'Content-Type':'application/json','X-CSRF-Token':csrf},
          body:JSON.stringify({chapter_id:String(chapter),version_id:String(version),
            block_id:1,char_offset:offset,expected_revision:revision}),
        });
        if (response.status !== expected) throw new Error(`C progress ${response.status}, expected ${expected}`);
        return expected === 200 ? (await response.json()).revision : null;
      }
      const started = await save(3, initialRevision);
      const finished = await save(13, started);
      await save(13, finished, 409);
    }, {...fixture, csrf:await csrf(page)});
  }
  return {sql,scalar,csrf,privateResponse,chapter,read};
};
