'use strict';
// Disposable process acceptance for the actual main/flow_runtime wiring.
// Run after building tor-web with both durable H and F collectors integrated.
const {spawn,execFileSync} = require('node:child_process');
const {createServer,createConnection} = require('node:net');
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const db=process.env.APP_URL||process.env.TEST_DATABASE_URL||'';
const parsed=new URL(db);
if(parsed.protocol!=='postgresql:'||parsed.hostname!=='127.0.0.1'
    ||parsed.port!=='5432'||parsed.pathname!=='/tor_ci'||parsed.username!=='tor_app') {
  throw new Error('Lifecycle proof requires isolated loopback tor_app/tor_ci');
}
const binary=process.env.TOR_FLOW_SERVER_BIN||path.resolve('target/debug/tor-web');
const logs=process.env.TOR_FLOW_LIFECYCLE_LOGS||'/tmp/flow-lifecycle';
fs.mkdirSync(logs,{recursive:true});
const ua='Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36';
function sql(statement) {
  return execFileSync('psql',['-XAtq','-v','ON_ERROR_STOP=1','--dbname',db,'--command',statement],{
    encoding:'utf8',env:{...process.env,PGOPTIONS:'-c statement_timeout=2000 -c lock_timeout=100'},
  }).trim();
}
function number(statement){const raw=sql(statement);assert.match(raw,/^\d+$/);return Number(raw);}
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
async function deadline(promise,ms,label){let timer;try{return await Promise.race([promise,new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error(label)),ms);})]);}finally{clearTimeout(timer);}}
async function until(check,label,budget=10000) {
  const end=Date.now()+budget;
  while(Date.now()<end){const value=await check();if(value)return value;await delay(10);}
  throw new Error('Not observed: '+label);
}
async function port() {
  const server=createServer();await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const value=server.address().port;await new Promise(resolve=>server.close(resolve));return value;
}
function get(base,route) {
  return new Promise((resolve,reject)=>{
    const request=http.get(base+route,{agent:false,headers:{'user-agent':ua,'connection':'close'}},response=>{
      let body='';response.setEncoding('utf8');response.on('data',part=>body+=part);
      response.on('end',()=>resolve({status:response.statusCode,body}));
      response.on('error',reject);
    });
    request.setTimeout(15000,()=>request.destroy(new Error('fixture HTTP deadline')));
    request.on('error',reject);
  });
}
function refusing(port) {
  return new Promise(resolve=>{
    const socket=createConnection({host:'127.0.0.1',port});
    socket.setTimeout(100);
    socket.once('connect',()=>{socket.destroy();resolve(false);});
    socket.once('error',error=>resolve(error.code==='ECONNREFUSED'));
    socket.once('timeout',()=>{socket.destroy();resolve(false);});
  });
}
function rows(table,after) {
  return JSON.parse(sql("SELECT coalesce(json_agg(row_to_json(r)),'[]'::json) FROM (SELECT epoch,retired FROM "+
    table+" WHERE epoch>"+after+" ORDER BY epoch)r"));
}
function captured(before) {
  return {html:rows('ril_flow_health_epochs',before.html),cards:rows('flow_observation_epochs',before.cards)};
}
function maxima() {
  return {html:number('SELECT coalesce(max(epoch),0) FROM ril_flow_health_epochs'),
    cards:number('SELECT coalesce(max(epoch),0) FROM flow_observation_epochs')};
}
async function start(kind,before) {
  const listen=await port(),base='http://127.0.0.1:'+listen;
  const name='flow_lifecycle_'+kind+'_'+Date.now();
  const url=new URL(db);url.searchParams.set('application_name',name);
  const log=fs.openSync(path.join(logs,kind+'.log'),'w');
  const child=spawn(binary,[],{env:{...process.env,DATABASE_URL:url.toString(),
    TOR_AUTH_ENABLED:'1',TOR_PUBLIC_ORIGIN:base,TOR_LISTEN_ADDR:'127.0.0.1:'+listen,
    TOR_FLOW_OBSERVATIONS_ENABLED:'1',TOR_FLOW_SIGNALS_ENABLED:'0'},stdio:['ignore',log,log]});
  fs.closeSync(log);
  let spawnError;
  const done=new Promise(resolve=>{
    child.once('exit',(code,signal)=>resolve({code,signal}));
    child.once('error',error=>{spawnError=error;resolve({error:String(error)});});
  });
  try {
    await until(async()=>{if(spawnError)throw spawnError;if(child.exitCode!==null||child.signalCode!==null)throw new Error('server exited before readiness');
      try{return (await get(base,'/readyz')).status===200;}catch{return false;}},'HTTP ready');
    await until(()=>{const value=captured(before);return value.html.some(x=>!x.retired)&&value.cards.some(x=>!x.retired);},
      'both H and F registered workers');
    return {child,done,before,listen,base,name};
  } catch(error) {
    child.kill('SIGKILL');await done;throw error;
  }
}
async function heldBooksLock() {
  const child=spawn('psql',['-XAtq','-v','ON_ERROR_STOP=1','--dbname',db],{
    env:{...process.env,PGOPTIONS:'-c statement_timeout=2000 -c idle_in_transaction_session_timeout=5000'},
    stdio:['pipe','pipe','pipe'],
  });
  const done=new Promise(resolve=>child.once('exit',(code,signal)=>resolve({code,signal})));
  let data='',errors='';
  child.stdout.setEncoding('utf8');child.stdout.on('data',part=>data+=part);
  child.stderr.setEncoding('utf8');child.stderr.on('data',part=>errors+=part);
  child.stdin.write("BEGIN; LOCK TABLE books IN ACCESS EXCLUSIVE MODE; SELECT 'HELD:'||pg_backend_pid();\n");
  try {
    const pid=await until(()=>{const m=data.match(/HELD:(\d+)/);if(m)return Number(m[1]);
      if(child.exitCode!==null)throw new Error(errors);return false;},'owned books lock',3000);
    return {pid,async release(){child.stdin.end("ROLLBACK;\n\\q\n");const exit=await done;assert.equal(exit.code,0,errors);}};
  } catch(error){child.kill('SIGKILL');await done;throw error;}
}
function blocked(server,pid) {
  return number("SELECT count(*) FROM pg_stat_activity WHERE application_name='"+server.name+
    "' AND "+pid+"=ANY(pg_blocking_pids(pid))");
}
function selected(snapshot,live) {
  for(const field of ['html','cards']) {
    const values=snapshot[field].filter(row=>live[field].includes(row.epoch));
    assert.equal(values.length,live[field].length);
    snapshot[field]=values;
  }
  return snapshot;
}
(async()=>{
  let server,lock,book,normalBefore,abortedBefore;
  try {
    assert.equal(sql("SELECT to_regclass('ril_flow_health_epochs') IS NOT NULL AND to_regclass('flow_observation_epochs') IS NOT NULL"),'t');
    book=number("INSERT INTO books(source_identity,target_language,title,visibility,access_level) VALUES('ci:flow:lifecycle:"+
      Date.now()+"','ru','FLOW lifecycle owned book','public','basic') RETURNING id");
    normalBefore=maxima();server=await start('drained',normalBefore);
    const initial=captured(server.before);
    const live={html:initial.html.filter(x=>!x.retired).map(x=>x.epoch),
      cards:initial.cards.filter(x=>!x.retired).map(x=>x.epoch)};
    const htmlBefore=number('SELECT coalesce(sum(html_requests),0)::bigint FROM ril_flow_observed_hourly WHERE surface_code=1');
    lock=await heldBooksLock();
    let completed=false;
    const request=get(server.base,'/catalog?sort=newest');
    request.then(()=>completed=true,()=>completed=true);
    await until(()=>blocked(server,lock.pid)>0,'actual HTTP query blocked by the owned transaction',1000);
    assert.equal(completed,false);
    server.child.kill('SIGTERM');
    await until(()=>refusing(server.listen),'listener stopped accepting after SIGTERM',1000);
    assert.ok(blocked(server,lock.pid)>0,'request must still be in flight after the listener closed');
    const waiting=selected(captured(server.before),live);
    assert.ok(waiting.html.every(x=>!x.retired),'H retired before HTTP drained');
    assert.ok(waiting.cards.every(x=>!x.retired),'F retired before HTTP drained');
    assert.equal(server.child.exitCode,null);
    console.log('FLOW_LIFECYCLE_BEFORE_DRAIN '+JSON.stringify({listenerClosed:true,httpSqlBlocked:true,
      htmlRetired:waiting.html.map(x=>x.retired),cardsRetired:waiting.cards.map(x=>x.retired)}));
    await lock.release();lock=null;
    const response=await request;
    assert.equal(response.status,200);
    assert.ok(response.body.includes('/books/'+book));
    assert.ok(response.body.includes('data-flow-proof='));
    const exit=await deadline(server.done,25000,'drained server did not exit');
    assert.deepEqual(exit,{code:0,signal:null});
    const stopped=captured(server.before);
    assert.ok(stopped.html.every(x=>x.retired));
    assert.ok(stopped.cards.every(x=>x.retired));
    assert.equal(number('SELECT coalesce(sum(html_requests),0)::bigint FROM ril_flow_observed_hourly WHERE surface_code=1'),htmlBefore+1,
      'the final successful in-flight HTML response must be persisted once');
    assert.equal(number('SELECT coalesce(sum(issued),0)::bigint FROM flow_observation_hourly WHERE book_id='+book),1,
      'the final server-issued card must settle before F retires');
    console.log('FLOW_LIFECYCLE_AFTER_DRAIN '+JSON.stringify({status:response.status,exit,
      htmlRetired:stopped.html.map(x=>x.retired),cardsRetired:stopped.cards.map(x=>x.retired)}));
    server=null;
    abortedBefore=maxima();server=await start('aborted',abortedBefore);
    const active=captured(server.before);
    const abortedIds={html:active.html.filter(x=>!x.retired).map(x=>x.epoch),
      cards:active.cards.filter(x=>!x.retired).map(x=>x.epoch)};
    server.child.kill('SIGKILL');
    const killed=await server.done;
    assert.equal(killed.signal,'SIGKILL');
    const unresolved=selected(captured(server.before),abortedIds);
    assert.ok(unresolved.html.every(x=>!x.retired));
    assert.ok(unresolved.cards.every(x=>!x.retired));
    console.log('FLOW_LIFECYCLE_ABORT '+JSON.stringify({signal:killed.signal,
      htmlRetired:unresolved.html.map(x=>x.retired),cardsRetired:unresolved.cards.map(x=>x.retired)}));
    server=null;
    console.log('FLOW lifecycle PASS: actual main, SIGTERM listener close before drain, H/F wait for HTTP, final HTML/card preserved once, normal retirement, SIGKILL unresolved');
  } finally {
    if(lock)await lock.release().catch(()=>{});
    if(server){server.child.kill('SIGKILL');await server.done;}
    // Only this fixture's disposable rows are removed AFTER the assertions.
    // Production never deletes an unresolved epoch to invent completeness.
    const first=normalBefore||abortedBefore;
    if(first)sql("BEGIN; DELETE FROM ril_flow_health_hourly WHERE epoch>"+first.html+
      "; DELETE FROM ril_flow_health_epochs WHERE epoch>"+first.html+
      "; DELETE FROM flow_observation_health_hourly WHERE epoch>"+first.cards+
      "; DELETE FROM flow_observation_epochs WHERE epoch>"+first.cards+"; COMMIT");
    if(book)sql("BEGIN; DELETE FROM flow_observation_proofs WHERE book_id="+book+
      "; DELETE FROM flow_observation_hourly WHERE book_id="+book+"; DELETE FROM books WHERE id="+book+"; COMMIT");
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
