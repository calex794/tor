'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const childProcess = require('node:child_process');

// No database or browser is required for these negative controls.
const calls = [];
let outputs = [];
const originalExec = childProcess.execFileSync;
childProcess.execFileSync = (...args) => {
  calls.push(args);
  assert.ok(outputs.length, 'unexpected psql invocation');
  const next = outputs.shift();
  if (next instanceof Error) throw next;
  return next;
};
let create;
try {
  create = require('./audience_report_pg_helpers.cjs');
} finally {
  childProcess.execFileSync = originalExec;
}
const db = 'postgresql://fixture@127.0.0.1:5432/tor_ci_legacy_analytics';
const base = 'http://127.0.0.1:3108';
const h = create({db,base});
const cookies = value => ({context:()=>({cookies:async()=>value ?
  [{name:'irrelevant',value:'not-csrf'},{name:'tor-dev-csrf',value}] : []})});

test('psql is scoped, trimmed, bounded and fails closed', () => {
  calls.length=0;
  outputs=[' 17\n','not-a-number',new Error('psql failure'),'9'];
  assert.equal(h.scalar('SELECT 17'),17);
  assert.deepEqual(calls[0][0],'psql');
  assert.deepEqual(calls[0][1],['-X','-A','-t','-q','-v','ON_ERROR_STOP=1',
    '--dbname',db,'--command','SELECT 17']);
  assert.equal(calls[0][2].encoding,'utf8');
  assert.equal(calls[0][2].env.PGOPTIONS,'-c statement_timeout=5000 -c lock_timeout=2000');
  assert.throws(()=>h.scalar('invalid'),/numeric database evidence/);
  assert.throws(()=>h.sql('failed'),/psql failure/);
  const otherDb=db.replace('fixture@','other@');
  assert.equal(create({db:otherDb,base}).scalar('SELECT 9'),9);
  assert.equal(calls.at(-1)[1][7],otherDb);
  assert.equal(outputs.length,0);
});

test('CSRF missing is rejected', async () => {
  assert.equal(await h.csrf(cookies('token')),'token');
  await assert.rejects(h.csrf(cookies(null)),/CSRF cookie/);
});

test('private report requires status and all privacy headers', async () => {
  const good={'cache-control':'no-store','x-robots-tag':'noindex,nofollow'};
  const response=(status,headers)=>({status:()=>status,allHeaders:async()=>headers});
  await h.privateResponse(response(200,good));
  await h.privateResponse(response(403,good),403);
  await assert.rejects(h.privateResponse(response(401,good)));
  await assert.rejects(h.privateResponse(response(200,{...good,'cache-control':'public'})));
  await assert.rejects(h.privateResponse(response(200,{...good,'x-robots-tag':'noindex'})));
  await assert.rejects(h.privateResponse(response(200,{...good,'x-robots-tag':'nofollow'})));
});

test('chapter creates a version and publishes that exact version', () => {
  calls.length=0;
  outputs=['41\n','73\n',''];
  assert.deepEqual(h.chapter(12,3),{chapter:41,version:73});
  assert.equal(calls.length,3);
  assert.match(calls[0][1].at(-1),/VALUES\(12,3,'Source chapter 3','public','free',0\)/);
  assert.match(calls[1][1].at(-1),/VALUES\(41,1,'Readable text'\)/);
  assert.match(calls[2][1].at(-1),/VALUES\(41,73\)/);
  assert.equal(outputs.length,0);
});

test('read performs ordinary navigation, live revision saves and stale conflict', async () => {
  const requests=[], urls=[];
  const page={...cookies('token'),
    goto:async url=>{urls.push(url);return {status:()=>200};},
    waitForFunction:async predicate=>assert.match(String(predicate),/freeProgressReady/),
    evaluate:async (callback,input)=>callback(input)};
  const originalFetch=globalThis.fetch;
  let staleAccepted=false;
  globalThis.fetch=async (url,opts)=>{
    requests.push([url,opts]);
    if (requests.length===1) return {ok:true,json:async()=>({revision:7})};
    if (requests.length===2) return {status:200,json:async()=>({revision:8})};
    if (requests.length===3) return {status:200,json:async()=>({revision:9})};
    return {status:staleAccepted?200:409,json:async()=>({revision:10})};
  };
  try {
    await h.read(page,{chapter:41,version:73});
    assert.deepEqual(urls,[base+'/read/41',base+'/reader/41']);
    assert.equal(requests[0][0],'/api/v1/me/progress/free?chapter_id=41');
    assert.equal(requests[0][1].credentials,'same-origin');
    assert.equal(requests.length,4);
    for (const [url,opts] of requests.slice(1)) {
      assert.equal(url,'/api/v1/me/progress/free');
      assert.equal(opts.method,'PUT');
      assert.equal(opts.headers['X-CSRF-Token'],'token');
      assert.equal(opts.credentials,'same-origin');
    }
    assert.deepEqual(requests.slice(1).map(([,opts])=>JSON.parse(opts.body)),[
      {chapter_id:'41',version_id:'73',block_id:1,char_offset:3,expected_revision:7},
      {chapter_id:'41',version_id:'73',block_id:1,char_offset:13,expected_revision:8},
      {chapter_id:'41',version_id:'73',block_id:1,char_offset:13,expected_revision:9},
    ]);
    requests.length=0;
    staleAccepted=true;
    await assert.rejects(h.read(page,{chapter:41,version:73}),/expected 409/);
  } finally {
    globalThis.fetch=originalFetch;
  }
});
