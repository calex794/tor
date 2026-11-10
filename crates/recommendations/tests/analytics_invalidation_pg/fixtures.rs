use sqlx::{postgres::PgPoolOptions, PgPool};
use tor_recommendations::{
    repositories::audience::PgDiscoveryAudience,
    services::audience::{
        books::{BookDiscoveryCounts, BookDiscoveryMetrics, BookDiscoveryQuery},
        DiscoveryChannel,
    },
};

pub const BOOK_VALUES: [i64; 5] = [30, 100, 10, 15, 40];

pub struct Fixture {
    pub owner: PgPool,
    pub app: PgPool,
    pub store: PgDiscoveryAudience,
    pub base: i64,
    schema: &'static str,
}

async fn scoped_pool(url: &str, schema: &'static str, read_only: bool) -> PgPool {
    PgPoolOptions::new()
        .max_connections(3)
        .after_connect(move |connection, _| {
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={schema},public"))
                    .execute(&mut *connection)
                    .await?;
                if read_only {
                    sqlx::query("SET default_transaction_read_only=on")
                        .execute(&mut *connection)
                        .await?;
                }
                Ok(())
            })
        })
        .connect(url)
        .await
        .unwrap()
}

impl Fixture {
    pub async fn new(schema: &'static str, base: i64, count: i32) -> Self {
        assert!(schema.starts_with("analytics_invalid_"));
        assert!(schema.bytes().all(|c| c.is_ascii_lowercase() || c == b'_'));
        assert!((0..200).contains(&count));
        let app_url = super::current_analytics::url("TEST_DATABASE_URL", "tor_app");
        let owner_url = super::current_analytics::url("MIGRATOR_URL", "tor_migrator");
        let owner = PgPool::connect(&owner_url).await.unwrap();
        super::current_analytics::assert_schema(&owner).await;
        sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
            .execute(&owner)
            .await
            .unwrap();
        sqlx::query("DELETE FROM books WHERE id BETWEEN $1 AND $1+199 AND source_identity LIKE $2")
            .bind(base)
            .bind(format!("ci:{schema}:%"))
            .execute(&owner)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO books(id,source_identity,target_language,title,visibility,access_level,genres,tags)
             SELECT $1::bigint+i,'ci:'||$3::text||':'||($1::bigint+i)::text,'ru','Evidence book '||i,
                    'public','basic',ARRAY['all','g-'||lpad(i::text,3,'0')],
                    ARRAY['tag-all','t-'||lpad(i::text,3,'0')]
             FROM generate_series(0,$2::int-1) i",
        )
        .bind(base)
        .bind(count)
        .bind(schema)
        .execute(&owner)
        .await
        .unwrap();
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&owner)
            .await
            .unwrap();
        // Actual migrated anonymous shapes/checks/indexes. No production
        // trigger is disabled and no retired individual fact is fabricated.
        for table in [
            "rec_quality_daily",
            "organic_discovery_quality_daily",
            "rec_analytics_invalidations",
            "rec_dirty_buckets",
            "organic_discovery_dirty",
            "rec_worker_health",
        ] {
            sqlx::query(&format!(
                "CREATE TABLE {schema}.{table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&owner)
            .await
            .unwrap();
        }
        sqlx::query(&format!(
            "CREATE VIEW {schema}.rec_public_books WITH (security_barrier=true) AS
             SELECT * FROM public.rec_public_books WHERE id BETWEEN {base} AND {base}+199"
        ))
        .execute(&owner)
        .await
        .unwrap();
        sqlx::query(&format!("GRANT USAGE ON SCHEMA {schema} TO tor_app"))
            .execute(&owner)
            .await
            .unwrap();
        sqlx::query(&format!(
            "GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO tor_app"
        ))
        .execute(&owner)
        .await
        .unwrap();
        owner.close().await;
        let owner = scoped_pool(&owner_url, schema, false).await;
        let app = scoped_pool(&app_url, schema, true).await;
        Self {
            store: PgDiscoveryAudience::new(app.clone()),
            owner,
            app,
            base,
            schema,
        }
    }

    pub async fn seed(&self, book: i64, first_day: i32, last_day: i32) {
        sqlx::query(
            "INSERT INTO rec_quality_daily(bucket_date,book_id,surface,source,reason,position,
                 generation,algorithm_version,impressions,paired_clicks,unpaired_clicks,book_opens,starts,completions)
             SELECT (now() AT TIME ZONE 'UTC')::date-d,$1,'home','content','similar_by_content',
                    1,'retained','invalidation-fixture',100,2,3,7,11,13
             FROM generate_series($2::int,$3::int) d",
        )
        .bind(book)
        .bind(first_day)
        .bind(last_day)
        .execute(&self.owner)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO organic_discovery_quality_daily
                 (bucket_date,book_id,source,issued,clicks,book_opens,starts,completions)
             SELECT (now() AT TIME ZONE 'UTC')::date-d,$1,c.*
             FROM generate_series($2::int,$3::int) d
             CROSS JOIN (VALUES ('catalog',20,3,5,7,11),('search',10,2,3,5,7),
                                ('unattributed',0,0,0,17,19)) c(source,issued,clicks,opens,starts,completions)",
        )
        .bind(book)
        .bind(first_day)
        .bind(last_day)
        .execute(&self.owner)
        .await
        .unwrap();
    }

    pub async fn invalidate(&self, book: i64, source: &str, day: i32) {
        assert!(["recommendations", "catalog", "search", "unattributed"].contains(&source));
        let mut tx = self.owner.begin().await.unwrap();
        let statement = if source == "recommendations" {
            "DELETE FROM rec_quality_daily WHERE book_id=$1
             AND bucket_date=(now() AT TIME ZONE 'UTC')::date-$2 AND $3::text='recommendations'"
        } else {
            "DELETE FROM organic_discovery_quality_daily WHERE book_id=$1
             AND bucket_date=(now() AT TIME ZONE 'UTC')::date-$2 AND source=$3"
        };
        sqlx::query(statement)
            .bind(book)
            .bind(day)
            .bind(source)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO rec_analytics_invalidations(source,bucket_date,book_id)
             VALUES($1,(now() AT TIME ZONE 'UTC')::date-$2,$3) ON CONFLICT DO NOTHING",
        )
        .bind(source)
        .bind(day)
        .bind(book)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    /// Retirement can mark a bucket before bounded physical daily cleanup.
    pub async fn mark(&self, book: i64, source: &str, day: i32) {
        sqlx::query(
            "INSERT INTO rec_analytics_invalidations(source,bucket_date,book_id)
             VALUES($1,(now() AT TIME ZONE 'UTC')::date-$2,$3) ON CONFLICT DO NOTHING",
        )
        .bind(source)
        .bind(day)
        .bind(book)
        .execute(&self.owner)
        .await
        .unwrap();
    }

    pub async fn finish(self) {
        self.app.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.owner)
            .await
            .unwrap();
        sqlx::query("DELETE FROM books WHERE id BETWEEN $1 AND $1+199 AND source_identity LIKE $2")
            .bind(self.base)
            .bind(format!("ci:{}:%", self.schema))
            .execute(&self.owner)
            .await
            .unwrap();
        self.owner.close().await;
    }
}

pub fn query(days: i32, genre: Option<&str>, after: Option<i64>) -> BookDiscoveryQuery {
    BookDiscoveryQuery::new(days, genre.map(str::to_owned), None, after).unwrap()
}

pub fn values(metrics: &BookDiscoveryMetrics) -> [Option<i64>; 5] {
    [
        metrics.issued,
        metrics.impressions,
        metrics.clicks,
        metrics.book_opens,
        metrics.starts,
    ]
}

pub fn channel_values(channel: &DiscoveryChannel) -> [Option<i64>; 4] {
    [channel.clicks, channel.book_opens, channel.starts, channel.completions]
}

pub fn count_sum(counts: &BookDiscoveryCounts) -> i64 {
    counts.unknown_evidence
        + counts.no_observations
        + counts.observed_without_starts
        + counts.with_starts
}
