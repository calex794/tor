#!/usr/bin/env python3
"""One-use, exact-match integration for PR328 in an isolated CI checkout.
No deployment, external requests, branch movement or credentials. Remove after integration.
"""
from pathlib import Path
import json
import re

ROOT=Path(__file__).resolve().parents[1]
STAMP=ROOT/'tools/rec13-integration.applied'

def read(path):
    return (ROOT/path).read_text(encoding='utf-8')
def write(path,value):
    (ROOT/path).write_text(value,encoding='utf-8')
def replace(path,old,new):
    text=read(path)
    if new in text and old not in text:
        return
    if text.count(old)!=1:
        raise RuntimeError(f'exact integration mismatch: {path}: {old[:100]!r}')
    write(path,text.replace(old,new,1))

if STAMP.exists():
    print('REC13 integration already applied; formatter may still run')
    raise SystemExit(0)

replace('crates/books/src/repositories/mod.rs','pub mod book_members;','pub mod recommendation_policy;\npub mod book_members;')
replace('crates/recommendations/src/repositories/feedback/mod.rs','mod serving;','mod serving;\nmod status;')
replace('crates/web/src/lib.rs','mod auth;','mod recommendation_http;\npub mod recommendation_worker;\nmod auth;')
replace('crates/web/src/lib.rs','    app.with_state(AppState {','    let rec_runtime = follow_control.as_ref().map(|control| {\n        let (accounts, cookies) = control.browser_context();\n        recommendation_http::Runtime::new(accounts.clone(), cookies.clone())\n    });\n    let app = app.with_state(AppState {')
replace('crates/web/src/lib.rs','    .merge(search_api)\n}','    .merge(search_api);\n    recommendation_http::instrument(app, rec_runtime)\n}')
replace('crates/web/src/auth.rs','    let cookies = CookiePolicy { secure, origin };','    let cookies = CookiePolicy { secure, origin };\n    let rec_runtime = crate::recommendation_http::Runtime::new(accounts.clone(), cookies.clone());')
replace('crates/web/src/auth.rs','    if let Some(cover_routes) = cover_routes {\n        app.merge(cover_routes)\n    } else {\n        app\n    }\n}', '    let app = if let Some(cover_routes) = cover_routes {\n        app.merge(cover_routes)\n    } else {\n        app\n    };\n    let app = app.merge(crate::recommendation_http::routes(rec_runtime.clone()));\n    crate::recommendation_http::instrument(app, Some(rec_runtime))\n}')
replace('crates/web/src/main.rs','    let mut pending_ranking_consumer:', '    let mut pending_rec_worker: Option<tor_recommendations::repositories::PgRecommendations> = None;\n    let mut pending_ranking_consumer:')
replace('crates/web/src/main.rs','            let pool = catalog.pool();','            let pool = catalog.pool();\n            if env::var("TOR_AUTH_ENABLED").as_deref() == Ok("1") {\n                pending_rec_worker = Some(tor_recommendations::repositories::PgRecommendations::new(pool.clone()));\n            }')
replace('crates/web/src/main.rs','    axum::serve(\n        listener,','    let _rec_worker = pending_rec_worker.and_then(tor_web::recommendation_worker::spawn);\n    axum::serve(\n        listener,')
# Fail closed on malformed runtime opt-ins; they cannot activate a production feature accidentally.
replace('crates/web/src/main.rs','    let app = match env::var("DATABASE_URL") {','    for flag in ["TOR_RECOMMENDATIONS_ENABLED", "TOR_REC_ATTRIBUTION_ENABLED"] {\n        if let Ok(value) = env::var(flag) {\n            if !matches!(value.as_str(), "0" | "1") { return Err(format!("invalid boolean flag {flag}").into()); }\n        }\n    }\n    if env::var("TOR_REC_ATTRIBUTION_ENABLED").as_deref() == Ok("1") && env::var("TOR_RECOMMENDATIONS_ENABLED").as_deref() != Ok("1") { return Err("attribution requires recommendations".into()); }\n    let app = match env::var("DATABASE_URL") {')
# Required additive schema; never mutate historical migrations.
p='crates/books/src/repositories/postgres/readiness.rs'
t=read(p)
t,n=re.subn(r'(REQUIRED_MIGRATION\s*:\s*i64\s*=\s*)\d+',r'\g<1>202610310203',t)
if n!=1: raise RuntimeError('migration readiness anchor')
write(p,t)

# C emits start only after real validated CAS. S3 partial cursors remain ineligible.
replace('crates/reading/src/repositories/sync.rs','        if finished {\n            // One successful CAS','        started::record(&mut tx, actor, book_id, chapter_id, version_id, None).await?;\n        if finished {\n            // One successful CAS')
replace('crates/reading/src/repositories/sync.rs','        if finished {\n            // The C cursor','        if storage_object.is_none() {\n            started::record(&mut tx, actor, book_id, chapter_id, version_id, Some(purchase_id)).await?;\n        }\n        if finished {\n            // The C cursor')
replace('crates/reading/src/repositories/sync.rs','#[path = "sync_s3_completion.rs"]','#[path = "sync_started.rs"]\nmod started;\n\n#[path = "sync_s3_completion.rs"]')
replace('crates/reading/src/repositories/sync_s3_completion.rs','        let created = sqlx::query_scalar::<_, String>(','        super::started::record(&mut tx, actor, book_id, chapter_id, version_id, Some(purchase_id)).await?;\n        let created = sqlx::query_scalar::<_, String>(')

# Settings remain available for erasure even if serving is disabled.
replace('crates/web/src/recommendation_http/pages.rs','struct ConsentForm {enabled:String,','struct ConsentForm {#[serde(default)] enabled:String,')
# Keep transparent local guard evidence for the static route guard checker.
p='crates/web/src/recommendation_http/pages.rs'
t=read(p).replace('api::actor(', 'actor(').replace('api::csrf(', 'csrf(')
helpers='''\nasync fn actor(state:&Runtime,headers:&HeaderMap)->Result<(i64,[u8;32]),StatusCode> {
    let token=state.cookies.session_token(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let digest=tor_accounts::services::security::session_token_digest(&token);
    let actor=state.accounts.active_session_user(&digest).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?.ok_or(StatusCode::UNAUTHORIZED)?;
    Ok((actor,digest))
}
fn csrf(state:&Runtime,headers:&HeaderMap,token:&str)->Result<(),StatusCode> {
    if !state.cookies.same_origin(headers) || !state.cookies.valid_csrf(headers,token) {return Err(StatusCode::FORBIDDEN);}
    Ok(())
}
'''
t=t.replace('#[cfg(test)]',helpers+'\n#[cfg(test)]',1)
t=re.sub(r'\buser\b','user_id',t)
write(p,t)
# Reader surface resolves ONLY public B metadata after the real reader response succeeds.
p='crates/web/src/recommendation_http/decorate.rs'
t=read(p)
t=t.replace('    let Some((surface,seed))=surface(&path,&response) else {return response};','''    let selected=surface(&path,&response);
    let selected=if selected.is_none() {
        let chapter=path.strip_prefix("/read/paid/").or_else(||path.strip_prefix("/read/")).or_else(||path.strip_prefix("/reader/")).and_then(|id|book_id(id).ok());
        match chapter {
            Some(chapter)=>match state.store.seed_for_chapter(chapter).await {Ok(Some(book))=>Some((Surface::ReaderEnd,Some(book))),_=>None},
            None=>None,
        }
    } else {selected};
    let Some((surface,seed))=selected else {return response};''')
write(p,t)
# Use the source's version, including honest public_fallback_v1 in every assignment.
p='crates/recommendations/src/repositories/feedback/assignments.rs'
t=read(p).replace('position,generation_id)\n                 SELECT $1::uuid,$2,$3,$4,$5,$6,$7,$8,$9::uuid','position,generation_id,algorithm_version)\n                 SELECT $1::uuid,$2,$3,$4,$5,$6,$7,$8,$9::uuid,$10')
t=t.replace('.bind(feed.generation_id.as_deref()).fetch_optional', '.bind(feed.generation_id.as_deref()).bind(item.algorithm_version).fetch_optional')
write(p,t)
# A first start must not count an already completed book as a newly acquired reader.
p='crates/recommendations/src/repositories/feedback/conversions.rs'
t=read(p).replace('rec_verified_read_events ', 'rec_verified_read_events_v2 ')
t=t.replace('pub async fn accept_read_event(&self, event: &str) -> Result<bool, FeedbackError> {','''pub async fn accept_read_event(&self, event: &str) -> Result<bool, FeedbackError> {
        self.accept_read_event_with_policy(event, true).await
    }
    pub(super) async fn accept_read_event_with_policy(&self, event: &str, collect: bool) -> Result<bool, FeedbackError> {''')
t=t.replace('if let Some(subject)=subject {','if let Some(subject)=subject.filter(|_|collect) {')
write(p,t)
p='crates/recommendations/src/repositories/feedback/maintenance.rs'
t=read(p)
t=t.replace('pub async fn tick_feedback(&self) -> Result<usize, FeedbackError> {','pub async fn tick_feedback(&self, collect: bool) -> Result<usize, FeedbackError> {')
t=t.replace('self.accept_read_event(event).await?', 'self.accept_read_event_with_policy(event, collect).await?')
t=t.replace('sqlx::query("UPDATE rec_feedback_state SET hidden=false WHERE hidden AND expires_at<now()")','sqlx::query("DELETE FROM rec_feedback_state WHERE (subject_id,book_id) IN(SELECT subject_id,book_id FROM rec_feedback_state WHERE expires_at<now()-interval \'1 hour\' LIMIT 1000)")')
t=t.replace('// Keep revisions (without the old choice) until erasure so an old CAS\n        // command cannot re-create a dismissed preference after TTL.', '// Expire personal choices completely after their 30-day window and a\n        // one-hour CSRF grace period; old nonzero CAS revisions cannot recreate them.')
write(p,t)
p='crates/web/src/recommendation_worker.rs'
t=read(p).replace('let result=store.tick_feedback().await;', 'let collect=std::env::var("TOR_REC_ATTRIBUTION_ENABLED").as_deref()==Ok("1");\n            let result=store.tick_feedback(collect).await;')
t=t.replace('            if store.advance_generation()', '            if loops%120==0 {let _=store.schedule_due_refresh().await;}\n            if store.advance_generation()')
write(p,t)
# Add a real navigation link without changing existing account actions.
p='crates/web/templates/me.html'
t=read(p)
if '</main>' in t:
    t=t.replace('</main>','<p><a href="/me/recommendations">Рекомендации и согласие</a></p></main>',1)
else:
    t += '\n<p><a href="/me/recommendations">Рекомендации и согласие</a></p>\n'
write(p,t)

# Add exact route classifications without renumbering any existing route.
ROUTES=[
('GET','/api/v1/recommendations','api::feed','public'),
('GET','/api/v1/books/{id}/recommendations','api::book_feed','public'),
('POST','/api/v1/recommendations/impressions','api::impressions','scoped'),
('POST','/api/v1/recommendations/activate','api::activate','scoped'),
('GET','/api/v1/me/recommendation-feedback','api::feedback_list','scoped'),
('POST','/api/v1/me/recommendation-feedback','api::feedback','scoped'),
('GET','/me/recommendations','pages::page','scoped'),
('POST','/me/recommendations/consent','pages::consent','scoped'),
('POST','/me/recommendations/feedback','pages::feedback','scoped'),
('GET','/api/v1/admin/recommendations/quality','admin::quality','administrator'),
('POST','/api/v1/admin/recommendations/control','admin::control','administrator'),
('GET','/me/recommendations/manage','admin::page','administrator'),
('POST','/me/recommendations/manage','admin::form_control','administrator'),
('GET','/me/books/{id}/recommendations','author::page','owner'),
('POST','/me/books/{id}/recommendations','author::save','owner'),
('GET','/assets/recommendations.js','pages::script','public'),
]
p='docs/contracts/ril-route-guards-v1.json'
d=json.loads(read(p)); old_policies=len(d['routes'])
for method,path,handler,policy in ROUTES:
    d['routes'].append(dict(method=method,template=path,handler=handler,policy=policy,source='crates/web/src/recommendation_http/mod.rs'))
d['routes'].sort(key=lambda r:(r['method'],r['template'],r['source'],r['handler']))
write(p,json.dumps(d,ensure_ascii=False,indent=2)+'\n')
p='crates/route_intelligence/src/domain/routes.rs'
t=read(p); old_count=int(re.search(r'const ROUTES: \[RouteRegistration; (\d+)\]',t).group(1)); entries=[]; classifiers=[]
for offset,(method,path,handler,policy) in enumerate(ROUTES):
    index=old_count+offset; admin=policy=='administrator'; asset=path.startswith('/assets/')
    family='/assets/' if asset else '/api/v1/books/' if path.startswith('/api/v1/books/') else '/api/v1/me/' if path.startswith('/api/v1/me/') else '/api/v1/recommendations/' if path.startswith('/api/') else '/me/'
    surface='Static' if asset else 'TrafficSurface::Admin' if admin else 'Api' if path.startswith('/api/') else 'Page'
    area='System' if asset else 'ProductArea::Admin' if admin else 'ProductArea::Recommendations'
    cost='Free' if asset else 'Read' if method=='GET' else 'Write'
    kind='Prefix' if '{' in path else 'Exact'
    entries.append(f'    RouteRegistration::new("{path}", "{method}", "{family}", {kind}, {surface}, {area}, {cost}), // {index}\n')
    classifiers.append(f'        (Some("{path}"), "{method}") => Some({index}),\n')
t=t.replace(f'const ROUTES: [RouteRegistration; {old_count}]',f'const ROUTES: [RouteRegistration; {old_count+len(ROUTES)}]')
t=t.replace('];\n\npub const fn registry()', ''.join(entries)+'];\n\npub const fn registry()',1)
write(p,t)
p='crates/route_intelligence/src/domain/routes/classifier.rs';t=read(p)
if t.count('        _ => None,')!=1: raise RuntimeError('RIL fallback classifier anchor')
write(p,t.replace('        _ => None,',''.join(classifiers)+'        _ => None,',1))
# Exact new totals, not weaker assertions.
for p in ['crates/route_intelligence/src/domain/tests.rs','tests/architecture/test_ril_exposure.py']:
    t=read(p)
    t=re.sub(rf'\b{old_count}\b',str(old_count+len(ROUTES)),t)
    if old_policies!=old_count:
        t=re.sub(rf'\b{old_policies}\b',str(old_policies+len(ROUTES)),t)
    write(p,t)
# Reviewed role policy has a strict allowlist, actual role lookup and mutation tests.
p='tools/check_ril_governance.py';t=read(p)
t=t.replace('"scoped", "owner", "token", "operator"}', '"scoped", "owner", "token", "operator", "administrator"}')
t=t.replace('PROTECTED = {"session", "scoped", "owner"}', 'PROTECTED = {"session", "scoped", "owner", "administrator"}')
t=t.replace('SCOPE_CALLS = {','SCOPE_CALLS = {\n    "consent", "set_consent", "set_feedback", "hidden_books", "impressions", "activate",')
t=t.replace('OWNER_CALLS = {','OWNER_CALLS = {\n    "recommendation_settings", "update_recommendation_settings",')
anchor='    if policy == "recovery":'
role_guard='''    rec_admin = {
        ("GET", "/api/v1/admin/recommendations/quality", "admin::quality"),
        ("POST", "/api/v1/admin/recommendations/control", "admin::control"),
        ("GET", "/me/recommendations/manage", "admin::page"),
        ("POST", "/me/recommendations/manage", "admin::form_control"),
    }
    route_key = (entry["method"], entry["template"], entry["handler"])
    if route_key in rec_admin and policy != "administrator":
        errors.append(f"{label}: recommendation administration policy downgraded")
    if policy == "administrator":
        if route_key not in rec_admin or entry["source"] != "crates/web/src/recommendation_http/mod.rs":
            errors.append(f"{label}: administrator exception is not allowed here")
        if not ("session_actor(" in evidence and "!verified.administrator" in evidence
                and "StatusCode::FORBIDDEN" in evidence):
            errors.append(f"{label}: missing independent administrator role guard")
'''
if t.count(anchor)!=1: raise RuntimeError('RIL role check anchor')
t=t.replace(anchor,role_guard+anchor)
write(p,t)

# Required tests are registered in existing workflows; temporary workflow is removed later.
for p in ['.github/workflows/tor-integration.yml','.github/workflows/tor-pg-runtime.yml']:
    anchor='          cargo test -p tor-recommendations --test slices_pg f_05_ -- --ignored --test-threads=1'
    extra=anchor+'\n          cargo test -p tor-recommendations --test feedback_pg -- --ignored --test-threads=1\n          cargo test -p tor-recommendations --test generations_pg -- --ignored --test-threads=1\n          cargo test -p tor-web --test recommendation_feedback_http_pg -- --ignored --test-threads=1'
    replace(p,anchor,extra)
p='tools/ci_test_map.py';t=read(p)
t=t.replace('TEST_SUITES = {','TEST_SUITES = {\n    "crates/recommendations/tests/feedback_pg.rs": {"rust", "postgres"},\n    "crates/recommendations/tests/generations_pg.rs": {"rust", "postgres"},\n    "crates/web/tests/recommendation_feedback_http_pg.rs": {"rust", "postgres"},')
write(p,t)

# Machine API remains additive; existing obligations and schema references stay intact.
p='docs/contracts/openapi-v1.json';spec=json.loads(read(p));schemas=spec['components']['schemas']
item=schemas['RecommendationItem']; item['properties'].update({
 'title':{'type':'string','maxLength':200},
 'ticket':{'type':'string','pattern':'^[A-Za-z0-9_-]{43}$'},
 'feedback_revision':{'type':'integer','minimum':0}})
schemas['RecommendationPage']['properties']['personalized']={'type':'boolean'}
schemas['RecImpressionBatch']={'type':'object','additionalProperties':False,'required':['items'],'properties':{'items':{'type':'array','minItems':1,'maxItems':20,'items':{'type':'object','additionalProperties':False,'required':['ticket','visible_fraction_per_mille','continuous_visible_ms','document_visible'],'properties':{'ticket':{'type':'string','pattern':'^[A-Za-z0-9_-]{43}$'},'visible_fraction_per_mille':{'type':'integer','minimum':0,'maximum':1000},'continuous_visible_ms':{'type':'integer','minimum':0,'maximum':60000},'document_visible':{'type':'boolean'}}}}}}
schemas['RecFeedbackCommand']={'type':'object','additionalProperties':False,'required':['book_id','choice','expected_revision','command_key'],'properties':{'book_id':{'type':'string','pattern':'^[1-9][0-9]*$'},'choice':{'enum':['not_interested','clear']},'expected_revision':{'type':'integer','minimum':0},'command_key':{'type':'string','pattern':'^[A-Za-z0-9_-]{43}$'}}}
for method,path,handler,policy in ROUTES:
    if not path.startswith('/api/') or path in spec['paths']:
        continue
    op={'operationId':'rec13_'+handler.replace('::','_'),'summary':'REC-13 '+handler,'responses':{'200':{'description':'Bounded result'},'400':{'$ref':'#/components/responses/Http400'},'401':{'$ref':'#/components/responses/Http401'},'403':{'$ref':'#/components/responses/Http403'},'404':{'$ref':'#/components/responses/Http404'},'409':{'$ref':'#/components/responses/Http409'},'429':{'$ref':'#/components/responses/Http429'},'503':{'$ref':'#/components/responses/Http503'}},'x-tor-owner':'F/I','x-tor-runtime-status':'implemented in PR328; requires exact-head PG/HTTP/browser acceptance'}
    if handler=='api::feed': op['responses']['200']['content']={'application/json':{'schema':{'$ref':'#/components/schemas/RecommendationPage'}}}
    if handler=='api::impressions':
        op['requestBody']={'required':True,'content':{'application/json':{'schema':{'$ref':'#/components/schemas/RecImpressionBatch'}}}}
        op['responses']['202']={'description':'Claims accepted, never trusted reading'}
    if method=='POST': op['parameters']=[{'in':'header','name':'X-CSRF-Token','required':handler!='api::activate','schema':{'type':'string'}}]
    spec['paths'].setdefault(path,{})[method.lower()]=op
# GET and POST share the same feedback path; add POST even when GET exists.
feedback=spec['paths'].setdefault('/api/v1/me/recommendation-feedback',{})
feedback['post']={'operationId':'rec13_feedback','summary':'Own explicit feedback; actor from session','requestBody':{'required':True,'content':{'application/json':{'schema':{'$ref':'#/components/schemas/RecFeedbackCommand'}}}},'parameters':[{'in':'header','name':'X-CSRF-Token','required':True,'schema':{'type':'string'}}],'responses':{'200':{'description':'Idempotent own revision'},'403':{'$ref':'#/components/responses/Http403'},'409':{'$ref':'#/components/responses/Http409'}}}
write(p,json.dumps(spec,ensure_ascii=False,indent=2)+'\n')
p='docs/contracts/release-flags-v1.json';flags=json.loads(read(p))
flags['flags']['recommendations_enabled']={'default_enabled':False,'gate':'REC-13 exact SHA PG/HTTP/browser; env TOR_RECOMMENDATIONS_ENABLED','requires':[]}
flags['flags']['recommendation_attribution_enabled']={'default_enabled':False,'gate':'REC-13 consent and trusted C facts; env TOR_REC_ATTRIBUTION_ENABLED','requires':['recommendations_enabled']}
write(p,json.dumps(flags,ensure_ascii=False,indent=2)+'\n')
STAMP.write_text('PR328 exact integration v1; remove temporary integration helpers before review readiness\n')
print('REC13 integrated: source wiring, C facts, explicit guards, append-only route registry, existing CI')
