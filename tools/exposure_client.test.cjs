'use strict';
// Execute the production observer with a deterministic clock/visibility source.
// Actual Chromium geometry and native navigation are covered separately.
const test=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const path=require('node:path');
const source=fs.readFileSync(path.join(__dirname,'../crates/web/assets/flow_observations.js'),'utf8');
function fixture(respond=()=>Promise.resolve({status:204})) {
  let now=0,next=0,intersection;
  const timers=new Map(),callbacks=new Map(),sent=[];
  class Element {
    constructor(){this.isConnected=true;this.handlers=new Map();}
    getAttribute(){return 'A'.repeat(43);}
    addEventListener(name,fn){this.handlers.set(name,fn);}
    contains(link){return link===this.link;}
    closest(){return this;}
  }
  const element=new Element();element.link=new Element();
  const document={visibilityState:'visible',querySelectorAll:()=>[element],addEventListener:(name,fn)=>callbacks.set(name,fn)};
  function IntersectionObserver(callback){intersection=callback;this.observe=()=>{};}
  const context={document,Element,IntersectionObserver,performance:{now:()=>now},
    window:{IntersectionObserver,addEventListener:(name,fn)=>callbacks.set(name,fn)},
    setTimeout:(fn,delay)=>{const id=++next;timers.set(id,{at:now+delay,fn});return id;},clearTimeout:id=>timers.delete(id),
    fetch:(url,options)=>{sent.push({url,options,body:JSON.parse(options.body)});return respond();}};
  vm.runInNewContext(source,context);
  async function flush(){for(let i=0;i<8;i++)await Promise.resolve();}
  async function advance(duration){await flush();const until=now+duration;for(;;){const due=[...timers].filter(([,t])=>t.at<=until).sort((a,b)=>a[1].at-b[1].at)[0];if(!due)break;timers.delete(due[0]);now=due[1].at;due[1].fn();await flush();}now=until;await flush();}
  return {sent,element,advance,ratio:value=>intersection([{target:element,intersectionRatio:value}]),
    visible:state=>{document.visibilityState=state;callbacks.get('visibilitychange')();},
    pagehide:()=>callbacks.get('pagehide')(),click:()=>element.handlers.get('click')({target:element.link})};
}
test('continuous 60% in foreground resets on hidden and below-threshold transitions',async()=>{
  const f=fixture();f.ratio(.59);await f.advance(3000);assert.equal(f.sent.filter(e=>e.body.kind==='view').length,0);
  f.ratio(.6);await f.advance(600);f.ratio(.59);await f.advance(600);assert.equal(f.sent.filter(e=>e.body.kind==='view').length,0);
  f.ratio(.6);await f.advance(600);f.visible('hidden');await f.advance(5000);assert.equal(f.sent.filter(e=>e.body.kind==='view').length,0);
  f.visible('visible');await f.advance(1009);assert.equal(f.sent.filter(e=>e.body.kind==='view').length,0);await f.advance(1);
  const views=f.sent.filter(e=>e.body.kind==='view');assert.equal(views.length,1);assert.equal(views[0].body.visible_percent,60);assert.equal(views[0].body.continuous_visible_ms,1010);assert.equal(views[0].body.foreground,true);
  f.ratio(0);f.ratio(1);await f.advance(5000);assert.equal(f.sent.filter(e=>e.body.kind==='view').length,1);
});
test('clicks neither await delivery nor manufacture views; each event is one-shot',async()=>{
  const f=fixture(()=>new Promise(()=>{}));
  assert.equal(f.click(),undefined);f.click();assert.deepEqual(f.sent.map(e=>e.body.kind),['rendered','click']);
  for(const event of f.sent){assert.equal(event.url,'/api/v1/flow/observations');assert.deepEqual(Object.keys(event.body).sort(),['kind','proof']);assert.equal(event.options.credentials,'same-origin');assert.equal(event.options.keepalive,true);}
  f.ratio(1);await f.advance(500);f.pagehide();await f.advance(3000);assert.equal(f.sent.filter(e=>e.body.kind==='view').length,0);
});
test('registration/outage retries are bounded and loss never creates a successful event',async()=>{
  const f=fixture(()=>Promise.resolve({status:425}));await f.advance(0);await f.advance(3000);
  assert.equal(f.sent.length,4);assert.ok(f.sent.every(e=>e.body.kind==='rendered'));await f.advance(100000);assert.equal(f.sent.length,4);
  const disconnected=fixture(()=>Promise.reject(new Error('offline')));await disconnected.advance(5000);assert.equal(disconnected.sent.length,1);
  disconnected.ratio(1);disconnected.element.isConnected=false;await disconnected.advance(1200);assert.equal(disconnected.sent.filter(e=>e.body.kind==='view').length,0);
});
