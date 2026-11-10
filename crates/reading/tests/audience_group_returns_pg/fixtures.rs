//! Isolated observations over the real migrated public-book visibility port.
use sqlx::{postgres::PgPoolOptions, PgPool};

const SCHEMA: &str = "ci_flow_group_book_returns";

pub(super) struct Fixture {
    pub(super) owner: PgPool,
    pub(super) app: PgPool,
    pub(super) books: Vec<i64>,
}

async fn scoped_pool(url: &str) -> PgPool {
    PgPoolOptions::new()
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={SCHEMA},public"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .unwrap()
}

impl Fixture {
    pub(super) async fn new() -> Self {
        let app_url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL URL");
        let owner_url = std::env::var("MIGRATOR_URL").expect("disposable CI schema owner");
        for url in [&app_url, &owner_url] {
            assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:"));
        }
        let admin = PgPool::connect(&owner_url).await.unwrap();
        assert_eq!(
            sqlx::query_as::<_, (String, String)>("SELECT current_database(),current_user")
                .fetch_one(&admin)
                .await
                .unwrap(),
            ("tor_ci".into(), "tor_migrator".into())
        );
        sqlx::query(&format!("DROP SCHEMA IF EXISTS {SCHEMA} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query(&format!("CREATE SCHEMA {SCHEMA}"))
            .execute(&admin)
            .await
            .unwrap();
        for table in [
            "reader_book_return_coverage",
            "reader_book_return_subjects",
            "reader_book_return_daily",
        ] {
            sqlx::query(&format!(
                "CREATE TABLE {SCHEMA}.{table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&admin)
            .await
            .unwrap();
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string();
        let books: Vec<i64> = sqlx::query_scalar(
            "INSERT INTO public.books(source_identity,target_language,title,visibility,access_level,genres,tags)
             SELECT 'ci:group-book-returns:'||$1::text||':'||n,'ru','Книга '||n,
                    'public','basic',ARRAY['all'],ARRAY['tag-all']
             FROM generate_series(0,28) n RETURNING id",
        ).bind(unique).fetch_all(&admin).await.unwrap();
        let ids = books
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        // Keep the production policy and new projection; limit only fixture IDs.
        sqlx::query(&format!(
            "CREATE VIEW {SCHEMA}.reader_return_public_books WITH (security_barrier=true) AS
             SELECT * FROM public.reader_return_public_books WHERE id IN ({ids})"
        ))
        .execute(&admin)
        .await
        .unwrap();
        sqlx::query(&format!("GRANT USAGE ON SCHEMA {SCHEMA} TO tor_app"))
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query(&format!(
            "REVOKE ALL ON ALL TABLES IN SCHEMA {SCHEMA} FROM tor_app"
        ))
        .execute(&admin)
        .await
        .unwrap();
        sqlx::query(&format!(
            "GRANT SELECT ON ALL TABLES IN SCHEMA {SCHEMA} TO tor_app"
        ))
        .execute(&admin)
        .await
        .unwrap();
        admin.close().await;
        Self {
            owner: scoped_pool(&owner_url).await,
            app: scoped_pool(&app_url).await,
            books,
        }
    }

    pub(super) async fn metadata(&self, index: usize, genres: &[&str], tags: &[&str]) {
        sqlx::query("UPDATE public.books SET genres=$2,tags=$3 WHERE id=$1")
            .bind(self.books[index])
            .bind(genres)
            .bind(tags)
            .execute(&self.owner)
            .await
            .unwrap();
    }

    pub(super) async fn healthy_coverage(&self) {
        sqlx::query("DELETE FROM reader_book_return_coverage")
            .execute(&self.owner)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO reader_book_return_coverage(started_at,last_heartbeat_at)
             VALUES(((now() AT TIME ZONE 'UTC')::date-181)::timestamp AT TIME ZONE 'UTC',now())",
        )
        .execute(&self.owner)
        .await
        .unwrap();
    }

    pub(super) async fn daily(&self, index: usize, age: i32, values: [i64; 4]) {
        sqlx::query(
            "INSERT INTO reader_book_return_daily(book_id,cohort_day,day,enrolled,eligible,returned,unknown)
             SELECT $1,(now() AT TIME ZONE 'UTC')::date-$2::int,day,$3,$4,$5,$6
             FROM (VALUES(7),(30)) horizon(day)",
        )
        .bind(self.books[index])
        .bind(age)
        .bind(values[0])
        .bind(values[1])
        .bind(values[2])
        .bind(values[3])
        .execute(&self.owner)
        .await
        .unwrap();
    }

    pub(super) async fn cohort(&self, index: usize, age: i32, subject: &str, flags: [bool; 2]) {
        sqlx::query(
            "INSERT INTO reader_book_return_subjects(subject_id,book_id,cohort_day,returned_d7,returned_d30)
             VALUES($1::uuid,$2,(now() AT TIME ZONE 'UTC')::date-$3::int,$4,$5)",
        ).bind(subject).bind(self.books[index]).bind(age).bind(flags[0]).bind(flags[1])
            .execute(&self.owner).await.unwrap();
    }

    pub(super) async fn fingerprint(&self) -> (String, String, String) {
        sqlx::query_as(
            "SELECT
             (SELECT md5(COALESCE(string_agg(row_to_json(t)::text,'' ORDER BY id),'')) FROM reader_book_return_coverage t),
             (SELECT md5(COALESCE(string_agg(row_to_json(t)::text,'' ORDER BY subject_id,book_id,cohort_day),'')) FROM reader_book_return_subjects t),
             (SELECT md5(COALESCE(string_agg(row_to_json(t)::text,'' ORDER BY book_id,cohort_day,day),'')) FROM reader_book_return_daily t)",
        ).fetch_one(&self.owner).await.unwrap()
    }

    pub(super) async fn finish(self) {
        self.app.close().await;
        sqlx::query(&format!("DROP SCHEMA {SCHEMA} CASCADE"))
            .execute(&self.owner)
            .await
            .unwrap();
        sqlx::query("DELETE FROM public.books WHERE id=ANY($1)")
            .bind(&self.books)
            .execute(&self.owner)
            .await
            .unwrap();
        self.owner.close().await;
    }
}
