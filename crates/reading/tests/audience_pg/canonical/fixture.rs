use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool,
};
use std::str::FromStr;

pub struct Fixture {
    pub owner: PgPool,
    pub app: PgPool,
    pub schema: String,
}
impl Fixture {
    pub async fn new() -> Self {
        let owner_url = std::env::var("MIGRATOR_URL").unwrap();
        let app_url = std::env::var("TEST_DATABASE_URL").unwrap();
        for (url, user) in [(&owner_url, "tor_migrator"), (&app_url, "tor_app")] {
            let o = PgConnectOptions::from_str(url).unwrap();
            assert_eq!(o.get_host(), "127.0.0.1");
            let port = std::env::var("FLOW_TEST_PG_PORT")
                .unwrap_or_else(|_| "5432".into())
                .parse::<u16>()
                .expect("explicit disposable PostgreSQL port");
            assert!(port > 0);
            assert_eq!(o.get_port(), port);
            assert_eq!(o.get_database(), Some("tor_ci"));
            assert_eq!(o.get_username(), user);
        }
        let schema = format!(
            "flow_canonical_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let owner = PgPool::connect(&owner_url).await.unwrap();
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&owner)
            .await
            .unwrap();
        for table in [
            "reader_return_public_books",
            "reader_chapter_completions",
            "outbox_events",
            "reader_library_items",
            "reader_book_follows",
        ] {
            sqlx::query(&format!(
                "CREATE TABLE {schema}.{table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&owner)
            .await
            .unwrap();
        }
        for command in [
            format!("ALTER TABLE {schema}.reader_return_public_books ADD PRIMARY KEY(id)"),
            format!("GRANT USAGE ON SCHEMA {schema} TO tor_app"),
            format!("REVOKE ALL ON ALL TABLES IN SCHEMA {schema} FROM tor_app"),
            format!("GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO tor_app"),
        ] {
            sqlx::query(&command).execute(&owner).await.unwrap();
        }
        let namespace = schema.clone();
        let app = PgPoolOptions::new()
            .max_connections(3)
            .after_connect(move |conn, _| {
                let schema = namespace.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={schema},public"))
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("SET TIME ZONE 'Asia/Kathmandu'")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("SET default_transaction_read_only=on")
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&app_url)
            .await
            .unwrap();
        Self { owner, app, schema }
    }
    pub async fn sql(&self, sql: &str) {
        let mut tx = self.owner.begin().await.unwrap();
        sqlx::query(&format!("SET LOCAL search_path={},public", self.schema))
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::raw_sql(sql).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    pub async fn books(&self, count: i32) {
        self.sql(&format!("INSERT INTO reader_return_public_books SELECT n,'Book '||n,CASE WHEN n=3 THEN ARRAY[]::text[] WHEN n=2 THEN ARRAY['a','b'] ELSE ARRAY['a','a'] END,CASE WHEN n=3 THEN ARRAY[]::text[] ELSE ARRAY['x'] END FROM generate_series(1,{count})n")).await;
    }
    // This fixtures the actual migrated tables; HTTP/browser separately exercise real C writers.
    pub async fn completion(&self, user: i64, book: i64, chapter: i64, at: &str, mode: &str) {
        assert!(["free", "paid", "invalid", "mismatch", "missing"].contains(&mode));
        let purchase = if mode == "paid" || mode == "mismatch" {
            "123::bigint"
        } else {
            "NULL::bigint"
        };
        let authority = if mode == "paid" || mode == "mismatch" {
            "C-verified-purchased-position"
        } else if mode == "invalid" {
            "untrusted"
        } else {
            "C-verified-free-position"
        };
        let extra = if mode == "paid" {
            "||jsonb_build_object('purchase_id',123)"
        } else if mode == "mismatch" {
            "||jsonb_build_object('purchase_id',999)"
        } else {
            ""
        };
        self.sql(&format!("WITH facts AS (INSERT INTO reader_chapter_completions(user_id,book_id,chapter_id,first_version_id,completed_at,source_event_id,purchase_id) VALUES({user},{book},{chapter},{chapter},{at},gen_random_uuid(),{purchase}) RETURNING *) INSERT INTO outbox_events(id,event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers) SELECT source_event_id,'ChapterCompletedV1',1,'chapter',chapter_id,source_event_id::text,jsonb_build_object('user_id',user_id,'book_id',book_id,'chapter_id',chapter_id,'version_id',first_version_id,'authority','{authority}'){extra},ARRAY['fixture-only'] FROM facts WHERE {has_event}",has_event=if mode=="missing"{"false"}else{"true"})).await;
    }
    pub async fn finish(self) {
        self.app.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.owner)
            .await
            .unwrap();
        self.owner.close().await;
    }
}
