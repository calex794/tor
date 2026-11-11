use sqlx::{postgres::{PgConnectOptions,PgPoolOptions},PgPool};
use std::{str::FromStr,time::{SystemTime,UNIX_EPOCH}};
use tor_search::{domain::public_pages::*,repositories::public_pages::PgPublicPages};

pub struct Fixture {
    pub pool:PgPool,
    pub store:PgPublicPages,
    owner:PgPool,
    schema:String,
}
fn url(name:&str,role:&str)->String {
    let raw=std::env::var(name).expect("explicit disposable PostgreSQL URL");
    let opts=PgConnectOptions::from_str(&raw).unwrap();
    assert_eq!(opts.get_host(),"127.0.0.1");assert_eq!(opts.get_port(),5432);
    assert_eq!(opts.get_database(),Some("tor_ci"));assert_eq!(opts.get_username(),role);
    raw
}
impl Fixture {
    pub async fn new()->Self {
        let owner=PgPoolOptions::new().max_connections(2).connect(&url("MIGRATOR_URL","tor_migrator")).await.unwrap();
        assert!(sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM public._sqlx_migrations WHERE version=202611294686 AND success)")
            .fetch_one(&owner).await.unwrap());
        let schema=format!("flow_pages_{}",SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());
        sqlx::query(&format!("CREATE SCHEMA {schema}")).execute(&owner).await.unwrap();
        for table in ["books","chapters","chapter_versions","chapter_publications","flow_public_page_probe","flow_public_page_results"] {
            sqlx::query(&format!("CREATE TABLE {schema}.{table}(LIKE public.{table} INCLUDING ALL)")).execute(&owner).await.unwrap();
        }
        sqlx::query(&format!("GRANT USAGE ON SCHEMA {schema} TO tor_app")).execute(&owner).await.unwrap();
        sqlx::query(&format!("GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA {schema} TO tor_app")).execute(&owner).await.unwrap();
        sqlx::query(&format!("GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA {schema} TO tor_app")).execute(&owner).await.unwrap();
        let search_path=format!("SET search_path={schema},public");
        let pool=PgPoolOptions::new().max_connections(4).after_connect(move |c,_| {
            let sql=search_path.clone();Box::pin(async move { sqlx::query(&sql).execute(c).await?;Ok(()) })
        }).connect(&url("TEST_DATABASE_URL","tor_app")).await.unwrap();
        sqlx::query("INSERT INTO flow_public_page_probe(singleton) VALUES(true)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO books(id,source_identity,target_language,title,visibility,access_level)
            VALUES(1,'ci:flow-pages','ru','Current public book','public','basic')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO chapters(id,book_id,chapter_no,title,visibility,access_level,price_amount)
            VALUES(1,1,1,'Free chapter','public','free',0)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO chapter_versions(id,chapter_id,version_no,body_markdown)
            VALUES(1,1,1,'Disposable free text')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES(1,1)").execute(&pool).await.unwrap();
        Self {store:PgPublicPages::new(pool.clone()),pool,owner,schema}
    }
    pub async fn due(&self) {
        sqlx::query("UPDATE flow_public_page_probe SET started_at=now()-interval '61 seconds',lease_until=NULL")
            .execute(&self.pool).await.unwrap();
    }
    pub async fn round(&self,origin:&str,status:Option<u16>) {
        self.due().await;
        let lease=self.store.claim(origin).await.unwrap().unwrap();
        self.store.save(&lease,&results(&lease,status)).await.unwrap();
    }
    pub async fn readonly(&self)->PgPool {
        let search_path=format!("SET search_path={},public",self.schema);
        PgPoolOptions::new().max_connections(1).after_connect(move |c,_| {
            let sql=search_path.clone();Box::pin(async move {
                sqlx::query(&sql).execute(&mut *c).await?;
                sqlx::query("SET default_transaction_read_only=on").execute(c).await?;
                Ok(())
            })
        }).connect(&url("TEST_DATABASE_URL","tor_app")).await.unwrap()
    }
    pub async fn cleanup(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE",self.schema)).execute(&self.owner).await.unwrap();
        self.owner.close().await;
    }
}
pub fn results(lease:&PageProbeLease,status:Option<u16>)->Vec<PageProbeResult> {
    lease.targets.iter().cloned().map(|target|PageProbeResult{target,status}).collect()
}

