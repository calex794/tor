#!/usr/bin/env python3
"""One-use reviewed fixes for REC-13 integration; no service or credential access."""
from pathlib import Path
import json
ROOT=Path(__file__).resolve().parents[1]
STAMP=ROOT/'tools/rec13-hardening.applied'
if STAMP.exists():
    print('REC13 hardening already applied')
    raise SystemExit(0)
def read(path): return (ROOT/path).read_text(encoding='utf-8')
def write(path,text): (ROOT/path).write_text(text,encoding='utf-8')
def change(path,old,new):
    text=read(path)
    if text.count(old)!=1: raise RuntimeError('hardening exact match failed: '+path+': '+old[:80])
    write(path,text.replace(old,new,1))
# PostgreSQL named make_interval argument is integer, not an unknown/text type.
change('crates/recommendations/src/repositories/feedback/conversions.rs','make_interval(days=>$3)','make_interval(days=>$3::int)')
# Actual HTTP -> validated C -> outbox -> F, not a direct repository test shortcut.
p='crates/web/tests/recommendation_feedback_http_pg.rs'
t=read(p).replace('use tor_reading::{repositories::{PgReader,PgReadingSync},services::progress::ReadingPlace};','use tor_reading::repositories::PgReader;')
a=t.index('    let sync=PgReadingSync::new(pool.clone());')
b=t.index('    store.refresh_quality().await.unwrap();',a)
t=t[:a]+'''    let start=json!({"chapter_id":chapter.to_string(),"version_id":version.to_string(),"block_id":1,"char_offset":3,"expected_revision":0}).to_string();
    let started=call(&app,"PUT","/api/v1/me/progress/free",&cookie,ORIGIN,&csrf,&start,true).await;
    assert_eq!(started.status(),StatusCode::OK);
    let receipt=data(started).await;
    for event in store.pending_read_events().await.unwrap() {store.accept_read_event(&event).await.unwrap();}
    let finish=json!({"chapter_id":chapter.to_string(),"version_id":version.to_string(),"block_id":1,"char_offset":13,"expected_revision":receipt["revision"].as_i64().unwrap()}).to_string();
    assert_eq!(call(&app,"PUT","/api/v1/me/progress/free",&cookie,ORIGIN,&csrf,&finish,true).await.status(),StatusCode::OK);
    for event in store.pending_read_events().await.unwrap() {store.accept_read_event(&event).await.unwrap();}
'''+t[b:]
write(p,t)
# Re-saving an unchanged consent must NOT erase personal choices or rotate tickets.
p='crates/recommendations/src/repositories/feedback/consent.rs';t=read(p)
t=t.replace('SELECT subject_id::text,revision FROM rec_subjects','SELECT subject_id::text,revision,enabled FROM rec_subjects')
anchor='        let subject: String = state.try_get("subject_id")?;'
replacement='''        if state.try_get::<bool,_>("enabled")? == enabled {
            sqlx::query("UPDATE rec_subjects SET revision=revision+1,changed_at=now() WHERE user_id=$1")
                .bind(actor).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO rec_preference_commands(user_id,command_key,payload_hash,result_revision,command_type) VALUES($1,$2,$3,$4,'consent')")
                .bind(actor).bind(key.as_slice()).bind(payload.as_slice()).bind(revision+1).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(ConsentState {enabled,revision:revision+1});
        }
'''+anchor
if t.count(anchor)!=1: raise RuntimeError('consent anchor')
t=t.replace(anchor,replacement,1);write(p,t)
# Counter oracle correction from accepted differential evidence in PR309.
p='tools/check_contracts.py'
change(p,'totals.get(book_id, 0) + min(50, quality)','totals.get(book_id, 0) + min(50, max(0, min(1000, quality)) // 20)')
# Owners need a visible route into B recommendation policy settings.
p='crates/web/src/author_pages.rs';t=read(p)
t=t.replace('href=\\"/me/books/{}\\">{}</a></li>', 'href=\\"/me/books/{}\\">{}</a> <a href=\\"/me/books/{}/recommendations\\">Рекомендации</a></li>',1)
t=t.replace('escape_html(&book.title)\n            );','escape_html(&book.title),\n                book.id\n            );',1)
write(p,t)
# Replace F's direct chapter query with a narrow B-owned readable-metadata port.
p='crates/recommendations/src/repositories/feedback/observations.rs';t=read(p)
a=t.index('               EXISTS(SELECT 1 FROM rec_public_books b JOIN chapters c ON c.book_id=b.id')
b=t.index(' AS readable,',a)
t=t[:a]+'               EXISTS(SELECT 1 FROM rec_free_readable_books WHERE book_id=a.book_id)'+t[b:]
write(p,t)
# Exact acceptance workflow includes real Chromium, not a synthetic screen snapshot.
for p in ['.github/workflows/tor-integration.yml','.github/workflows/tor-pg-runtime.yml']:
    t=read(p)
    anchor='          cargo test -p tor-web --test recommendation_feedback_http_pg -- --ignored --test-threads=1'
    if t.count(anchor)!=1: raise RuntimeError('PG browser registration')
    t=t.replace(anchor,anchor+'\n          bash tools/rec13_browser_acceptance.sh',1)
    write(p,t)
# Test map evidence remains in the real PostgreSQL workflow, no skipped ignored test.
p='tools/ci_test_map.py';t=read(p).replace('TEST_SUITES = {','TEST_SUITES = {\n    "tools/rec13_browser_acceptance.sh": {"postgres"},\n    "tools/rec13_browser.cjs": {"postgres"},',1);write(p,t)
# OpenAPI actuator and operator payloads are strict and documented, not arbitrary JSON.
p='docs/contracts/openapi-v1.json';s=json.loads(read(p));defs=s['components']['schemas']
defs['RecActivation']={'type':'object','additionalProperties':False,'required':['ticket','csrf_token'],'properties':{'ticket':{'type':'string','pattern':'^[A-Za-z0-9_-]{43}$'},'csrf_token':{'type':'string','pattern':'^[A-Za-z0-9_-]{43}$'}}}
defs['RecOperatorCommand']={'type':'object','additionalProperties':False,'required':['action','expected_revision'],'properties':{'action':{'enum':['refresh','activate','hold','rollback']},'expected_revision':{'type':'integer','minimum':0},'generation':{'type':['string','null'],'format':'uuid'},'percent':{'enum':[0,1,5,10,25,50,100,None]}}}
for path,kind,content in [('/api/v1/recommendations/activate','RecActivation','application/x-www-form-urlencoded'),('/api/v1/admin/recommendations/control','RecOperatorCommand','application/json')]:
    s['paths'][path]['post']['requestBody']={'required':True,'content':{content:{'schema':{'$ref':'#/components/schemas/'+kind}}}}
s['paths']['/api/v1/recommendations/activate']['post']['responses']['303']={'description':'Canonical book navigation, not paid entitlement'}
write(p,json.dumps(s,ensure_ascii=False,indent=2)+'\n')
STAMP.write_text('REC13 hardening v1 applied\n')
print('REC13 hardening complete; tests remain required')
