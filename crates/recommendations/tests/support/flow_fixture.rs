use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool,
};
use std::{
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct Fixture {
    pub pool: PgPool,
    pub admin: PgPool,
    pub schema: String,
    #[allow(dead_code)] // Shared by independently compiled integration targets.
    pub app_url: String,
}
impl Fixture {
    pub async fn new(tables: &[&str]) -> Self {
        let app_url =
            std::env::var("TEST_DATABASE_URL").expect("explicit disposable application URL");
        let migrator = std::env::var("MIGRATOR_URL").expect("explicit disposable migration URL");
        let port = std::env::var("FLOW_TEST_PG_PORT")
            .ok()
            .map(|s| s.parse::<u16>().unwrap())
            .unwrap_or(5432);
        for (url, role) in [(&app_url, "tor_app"), (&migrator, "tor_migrator")] {
            let o = PgConnectOptions::from_str(url).unwrap();
            assert_eq!(o.get_host(), "127.0.0.1");
            assert_eq!(o.get_port(), port);
            assert_eq!(o.get_database(), Some("tor_ci"));
            assert_eq!(o.get_username(), role);
        }
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&migrator)
            .await
            .unwrap();
        let installed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM public._sqlx_migrations WHERE version=202611294684 AND success) AND EXISTS(SELECT 1 FROM public._sqlx_migrations WHERE version=202611294683 AND success)"
        ).fetch_one(&admin).await.unwrap();
        assert!(installed, "fresh real migrations are mandatory");
        let schema = format!(
            "flow_case_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let mut tx = admin.begin().await.unwrap();
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&mut *tx)
            .await
            .unwrap();
        for table in tables {
            assert!(table.bytes().all(|c| c.is_ascii_lowercase() || c == b'_'));
            sqlx::query(&format!(
                "CREATE TABLE {schema}.{table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        sqlx::query(&format!("GRANT USAGE ON SCHEMA {schema} TO tor_app"))
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(&format!(
            "GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA {schema} TO tor_app"
        ))
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(&format!(
            "GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA {schema} TO tor_app"
        ))
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let options = PgConnectOptions::from_str(&app_url)
            .unwrap()
            .options([("search_path", format!("{schema},pg_catalog,public"))]);
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .unwrap();
        Self {
            pool,
            admin,
            schema,
            app_url,
        }
    }
    pub async fn cleanup(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}
