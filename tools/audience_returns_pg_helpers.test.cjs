'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const childProcess = require('node:child_process');

const calls = [];
let outputs = [];
const exec = childProcess.execFileSync;
childProcess.execFileSync = (...args) => {
  calls.push(args);
  assert.ok(outputs.length, 'unexpected psql invocation');
  const next = outputs.shift();
  if (next instanceof Error) throw next;
  return next;
};
let create;
try {
  create = require('./audience_returns_pg_helpers.cjs');
} finally {
  childProcess.execFileSync = exec;
}
const db = 'postgresql://fixture@127.0.0.1:5432/tor_ci_legacy_analytics';
const base = 'http://127.0.0.1:3108';
const book = create({db,base,boundedSql:true,requireReaderUrl:false});
const group = create({db,base,boundedSql:true,requireReaderUrl:true});
const overall = create({db,base,boundedSql:false,requireReaderUrl:false});
const cookies = value => ({context:()=>({cookies:async()=>value ?
  [{name:'tor-dev-csrf',value}] : []})});

test('both SQL timeout policies and errors are preserved', () => {
  calls.length=0;
  outputs=[' 12\n','13','bad',new Error('psql timeout')];
  assert.equal(book.scalar('SELECT 12'),12);
  assert.equal(overall.scalar('SELECT 13'),13);
  for (const [cmd,args,opts] of calls) {
    assert.equal(cmd,'psql');
    assert.deepEqual(args.slice(0,8),['-X','-A','-t','-q','-v','ON_ERROR_STOP=1','--dbname',db]);
    assert.equal(opts.encoding,'utf8');
    assert.equal(opts.env.PGOPTIONS,'-c statement_timeout=5000 -c lock_timeout=2000');
  }
  assert.equal(calls[0][2].timeout,15000);
  assert.equal(calls[0][2].env.PGCONNECT_TIMEOUT,'5');
  assert.equal(Object.hasOwn(calls[1][2],'timeout'),false);
  assert.equal(calls[1][2].env.PGCONNECT_TIMEOUT,process.env.PGCONNECT_TIMEOUT);
  assert.throws(()=>group.scalar('bad'),/numeric database evidence/);
  assert.throws(()=>book.sql('timeout'),/psql timeout/);
  assert.throws(()=>create({db,base}),/boolean/);
  assert.equal(outputs.length,0);
});

test('private response, CSRF and metric remain strict', async () => {
  assert.equal(await book.csrf(cookies('csrf-token')),'csrf-token');
  await assert.rejects(book.csrf(cookies(null)),/CSRF cookie/);
  const good={'cache-control':'no-store','x-robots-tag':'noindex,nofollow'};
  const response=(status,headers)=>({status:()=>status,allHeaders:async()=>headers});
  await group.privateResponse(response(403,good),403);
  await assert.rejects(group.privateResponse(response(401,good),403));
  await assert.rejects(group.privateResponse(response(200,{...good,'cache-control':'public'})));
  await assert.rejects(group.privateResponse(response(200,{...good,'x-robots-tag':'nofollow'})));
  await assert.rejects(group.privateResponse(response(200,{...good,'x-robots-tag':'noindex'})));
  assert.equal(await overall.metric({locator:selector=>{
    assert.equal(selector,"[data-metric='cohort']");
    return {innerText:async()=>'  17  '};
  }},'cohort'),'17');
});

test('return chapter retains both paragraphs, book id and publication', () => {
  calls.length=0;
  outputs=['41','73',''];
  assert.deepEqual(group.chapter(12,3),{book:12,chapter:41,version:73});
  assert.equal(calls.length,3);
  assert.ok(calls[0][1].at(-1).includes("VALUES(12,3,'Return chapter 3','public','free',0)"));
  assert.ok(calls[1][1].at(-1).includes("E'Readable first paragraph.\\n\\nSecond paragraph.'"));
  assert.ok(calls[2][1].at(-1).includes('VALUES(41,73)'));
  assert.equal(outputs.length,0);
});

test('all progress calls use the live revision and preserve expected 409', async () => {
  const requests=[],urls=[];
  let activeUrl='';
  let status=200;
  const page={...cookies('csrf-token'),
    goto:async url=>{urls.push(url);activeUrl=url;return {status:()=>200};},
    url:()=>activeUrl,
    waitForFunction:async callback=>assert.match(String(callback),/freeProgressReady/),
    evaluate:async (callback,input)=>callback(input)};
  const oldFetch=globalThis.fetch;
  globalThis.fetch=async (url,opts)=>{
    requests.push([url,opts]);
    return requests.length%2===1
      ? {ok:true,json:async()=>({revision:7})}
      : {status};
  };
  try {
    const fixture={book:12,chapter:41,version:73};
    assert.equal(await group.progress(page,fixture,3),200);
    status=409;
    assert.equal(await book.progress(page,fixture,3,409),409);
    await assert.rejects(overall.progress(page,fixture,3),/expected 200/);
    assert.deepEqual(urls,Array(3).fill([base+'/read/41',base+'/reader/41']).flat());
    for(let i=0;i<requests.length;i+=2) {
      assert.equal(requests[i][0],'/api/v1/me/progress/free?chapter_id=41');
      assert.equal(requests[i][1].credentials,'same-origin');
      assert.equal(requests[i][1].cache,'no-store');
      const [url,opts]=requests[i+1];
      assert.equal(url,'/api/v1/me/progress/free');
      assert.equal(opts.method,'PUT');
      assert.equal(opts.credentials,'same-origin');
      assert.equal(opts.headers['X-CSRF-Token'],'csrf-token');
      assert.deepEqual(JSON.parse(opts.body),{chapter_id:'41',version_id:'73',
        block_id:1,char_offset:3,expected_revision:7});
    }
  } finally {
    globalThis.fetch=oldFetch;
  }
});

test('only group returns requires the exact AJAX URL before saving', async () => {
  const page={...cookies('csrf-token'),
    goto:async()=>({status:()=>200}),
    url:()=>base+'/wrong-reader',
    waitForFunction:async()=>{},
    evaluate:async()=>200};
  const fixture={book:12,chapter:41,version:73};
  await assert.rejects(group.progress(page,fixture,3),/actual AJAX reader/);
  assert.equal(await book.progress(page,fixture,3),200);
  assert.equal(await overall.progress(page,fixture,3),200);
});
