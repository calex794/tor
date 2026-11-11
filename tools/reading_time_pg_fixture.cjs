'use strict';
// Disposable real C commands are copied exactly into an isolated read-only report source.
const {execFileSync}=require('node:child_process');
const {randomBytes}=require('node:crypto');
const assert=require('node:assert/strict');
const db=process.env.APP_URL||process.env.TEST_DATABASE_URL||'';
const owner=process.env.MIGRATOR_URL||'';
const schema=process.env.TOR_READING_TIME_SCHEMA||'';
const base=process.env.TOR_READING_TIME_BASE||'http://127.0.0.1:3120';
const writer=process.env.TOR_READING_TIME_WRITER_BASE||'http://127.0.0.1:3119';
const shots=process.env.TOR_READING_TIME_SHOTS||'/tmp/reading-time-shots';
const port=process.env.FLOW_TEST_PG_PORT||'5432';
const FALL='2026-10-25T04:37:00Z';
const SPRING='2026-03-30T00:37:00Z';
assert.match(schema,/^flow_reading_time_browser_[0-9]+_[0-9]+$/);
assert.match(port,/^[1-9][0-9]{0,4}$/);
assert.ok(Number(port)<=65535);
for(const [raw,role] of [[db,'tor_app'],[owner,'tor_migrator']]){
  const url=new URL(raw);
  assert.equal(url.protocol,'postgresql:');
  assert.equal(url.hostname,'127.0.0.1');
  assert.equal(url.port,port);
  assert.equal(url.pathname,'/tor_ci');
  assert.equal(url.username,role);
  assert.equal(url.search,'');
  assert.equal(url.hash,'');
}
for(const raw of [base,writer]){
  assert.match(raw,/^http:\/\/127\.0\.0\.1:[1-9][0-9]*$/);
  assert.ok(Number(new URL(raw).port)<=65535);
}
assert.notEqual(base,writer);
function sql(statement,url=db,searchPath='public'){
  return execFileSync('psql',['-XAtq','-v','ON_ERROR_STOP=1','--dbname',url,'-c',statement],{
    encoding:'utf8',
    env:{...process.env,PGOPTIONS:'-c statement_timeout=5000 -c lock_timeout=2000 -c search_path='+searchPath},
  }).trim();
}
function scoped(statement){return sql(statement,owner,schema+',pg_catalog,public');}
function scalar(statement,url=db){
  const value=sql(statement,url);assert.match(value,/^\d+$/);
  const number=Number(value);assert.ok(Number.isSafeInteger(number));return number;
}
function clock(at){
  assert.ok([FALL,SPRING].includes(at));
  scoped('CREATE OR REPLACE FUNCTION '+schema+'.transaction_timestamp() RETURNS timestamptz LANGUAGE SQL STABLE AS '+
    "'SELECT ''"+at+"''::timestamptz';GRANT EXECUTE ON FUNCTION "+schema+'.transaction_timestamp() TO tor_app');
}
class Fixture{
  constructor(){
    this.run=Date.now()+'-'+randomBytes(4).toString('hex');
    this.prefix='ci:reading-time:'+this.run;
    this.tag='reading-time-'+this.run;
    this.emails=['admin','reader'].map(role=>'time-'+role+'-'+this.run+'@tor-ci.example');
    this.actors=[];this.book=null;this.chapters=[];
  }
  seedBook(){
    this.book=scalar("INSERT INTO books(source_identity,target_language,title,visibility,access_level,tags) VALUES('"+
      this.prefix+"','ru','PRIVATE-TIME-CONTENT-"+this.run+"','public','basic',ARRAY['"+this.tag+"']) RETURNING id");
    for(let no=1;no<=5;no++){
      const chapter=scalar("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) VALUES("+
        this.book+','+no+",'Time fixture chapter','public','free',0) RETURNING id");
      const version=scalar("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) VALUES("+
        chapter+",1,'Audience text') RETURNING id");
      sql('INSERT INTO chapter_publications(chapter_id,version_id) VALUES('+chapter+','+version+')');
      this.chapters.push({chapter,version});
    }
  }
  copy(){
    // Copy only this fixture's immutable C proofs after actual HTTP commands.
    for(const [table,time] of [['reader_chapter_starts','started_at'],['reader_chapter_completions','completed_at']]){
      const columns='user_id,book_id,chapter_id,first_version_id,purchase_id,source_event_id,'+time;
      scoped('INSERT INTO '+schema+'.'+table+'('+columns+') SELECT '+columns+' FROM public.'+table+
        ' WHERE user_id='+this.actors[1]+' AND book_id='+this.book);
    }
    const columns='id,event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers';
    scoped('INSERT INTO '+schema+'.outbox_events('+columns+') SELECT '+columns+' FROM public.outbox_events WHERE id IN('+
      'SELECT source_event_id FROM '+schema+'.reader_chapter_starts UNION SELECT source_event_id FROM '+schema+'.reader_chapter_completions)');
    const positions=[
      ['2026-03-01T00:01:00Z','2026-03-01T00:05:00Z'],
      ['2026-10-24T23:30:00Z','2026-10-25T00:30:00Z'],
      ['2026-10-25T00:45:00Z','2026-10-25T01:30:00Z'],
      ['2026-10-24T03:30:00Z','2026-10-24T03:45:00Z'],
      ['2026-10-25T04:01:00Z','2026-10-25T04:02:00Z'],
    ];
    positions.forEach(([start,end],index)=>scoped("UPDATE reader_chapter_starts SET started_at='"+start+
      "' WHERE chapter_id="+this.chapters[index].chapter+";UPDATE reader_chapter_completions SET completed_at='"+
      end+"' WHERE chapter_id="+this.chapters[index].chapter));
    // Timestamp placement is private test history. The real public facts remain untouched.
    assert.equal(scoped("SELECT has_table_privilege('tor_app','"+schema+".reader_chapter_starts','INSERT')"),'f');
  }
  publicFingerprint(){
    return sql("SELECT md5(jsonb_build_array("+
      "(SELECT jsonb_agg(to_jsonb(t) ORDER BY chapter_id) FROM reader_chapter_starts t WHERE book_id="+this.book+"),"+
      "(SELECT jsonb_agg(to_jsonb(t) ORDER BY chapter_id) FROM reader_chapter_completions t WHERE book_id="+this.book+"),"+
      "(SELECT jsonb_agg(jsonb_build_array(id,event_type,schema_version,aggregate_kind,aggregate_id,payload,required_consumers) ORDER BY id) FROM outbox_events t WHERE id IN("+
      "SELECT source_event_id FROM reader_chapter_starts WHERE book_id="+this.book+
      " UNION SELECT source_event_id FROM reader_chapter_completions WHERE book_id="+this.book+")))::text)");
  }
  reportFingerprint(){
    return scoped("SELECT md5(jsonb_build_array("+
      "(SELECT jsonb_agg(to_jsonb(t) ORDER BY chapter_id) FROM reader_chapter_starts t),"+
      "(SELECT jsonb_agg(to_jsonb(t) ORDER BY chapter_id) FROM reader_chapter_completions t),"+
      "(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM outbox_events t))::text)");
  }
  cleanup(){
    if(this.book!==null)sql("BEGIN;"+
      "CREATE TEMP TABLE time_owned_events ON COMMIT DROP AS SELECT source_event_id AS id FROM reader_chapter_starts WHERE book_id="+this.book+
      " UNION SELECT source_event_id FROM reader_chapter_completions WHERE book_id="+this.book+";"+
      "DELETE FROM event_deliveries WHERE event_id IN(SELECT id FROM time_owned_events);"+
      "DELETE FROM outbox_events WHERE id IN(SELECT id FROM time_owned_events);"+
      "DELETE FROM reader_book_positions WHERE book_id="+this.book+";"+
      "DELETE FROM reader_chapter_starts WHERE book_id="+this.book+";DELETE FROM reader_chapter_completions WHERE book_id="+this.book+";"+
      "DELETE FROM reader_saved_free_chapters WHERE chapter_id IN(SELECT id FROM chapters WHERE book_id="+this.book+");"+
      "DELETE FROM chapter_publications WHERE chapter_id IN(SELECT id FROM chapters WHERE book_id="+this.book+");"+
      "DELETE FROM book_members WHERE book_id="+this.book+";DELETE FROM books WHERE id="+this.book+";COMMIT",owner);
    for(const actor of this.actors)sql('DELETE FROM account_roles WHERE user_id='+actor);
    sql("DELETE FROM users WHERE email_normalized IN('"+this.emails.join("','")+"')");
  }
}
function prepare(){
  let statement='CREATE SCHEMA '+schema+';GRANT USAGE ON SCHEMA '+schema+' TO tor_app;';
  for(const table of ['reader_chapter_starts','reader_chapter_completions','outbox_events']){
    statement+='CREATE TABLE '+schema+'.'+table+' (LIKE public.'+table+' INCLUDING ALL);';
  }
  statement+='REVOKE ALL ON ALL TABLES IN SCHEMA '+schema+' FROM tor_app;GRANT SELECT ON ALL TABLES IN SCHEMA '+schema+' TO tor_app;';
  sql(statement,owner);clock(FALL);
}
if(require.main===module){
  const action=process.argv[2];
  if(action==='prepare')prepare();
  else if(action==='cleanup')sql('DROP SCHEMA '+schema+' CASCADE',owner);
  else if(action==='url'){
    const url=new URL(db);url.searchParams.set('options','-c search_path='+schema+',pg_catalog,public');process.stdout.write(url.toString());
  }else throw Error('expected prepare, cleanup, or url');
}
module.exports={Fixture,sql,scoped,scalar,clock,db,owner,schema,base,writer,shots,FALL,SPRING};
