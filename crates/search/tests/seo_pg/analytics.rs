//! Real database, fenced replacement and public visibility. No provider credentials.
use async_trait::async_trait;
use sqlx::{postgres::PgPoolOptions, Row};
use tor_search::{
    domain::seo_analytics::*,
    repositories::seo_analytics::PgSeoAnalytics,
    services::seo_analytics::{sync_one, SearchAnalyticsProvider, SeoAnalyticsStore},
};
fn snapshot(clicks: u64) -> Snapshot {
    Snapshot {
        totals: Some(Metrics {
            clicks,
            impressions: 100,
            position: Some(3.0),
        }),
        pages: vec![PageMetrics {
            path: "/".into(),
            metrics: Metrics {
                clicks: 2,
                impressions: 20,
                position: Some(2.0),
            },
        }],
        provider_rows: 1,
        omitted_rows: 0,
        limit_reached: false,
    }
}
struct FailedProvider;
#[async_trait]
impl SearchAnalyticsProvider for FailedProvider {
    async fn collect(&self, _: &str) -> Result<Snapshot, SeoAnalyticsError> {
        Err(SeoAnalyticsError::RateLimited)
    }
}
#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL and current migrations"]
async fn flow_seo_pg_replay_leases_failure_visibility_and_independent_totals() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let options: url_guard::Safe = url_guard::check(&url);
    assert_eq!(options.database, "tor_ci");
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT current_database()")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "tor_ci"
    );
    sqlx::query("DELETE FROM flow_seo_sources WHERE origin IN ('https://flow-seo-ci.test','https://flow-seo-ci-next.test')").execute(&pool).await.unwrap();
    let store = PgSeoAnalytics::new(pool.clone());
    let source = store
        .configure(SourceKey {
            provider: Provider::Google,
            property: "https://flow-seo-ci.test/".into(),
            origin: "https://flow-seo-ci.test".into(),
        })
        .await
        .unwrap();
    assert_eq!(store.request_refresh(Provider::Google).await.unwrap(), 7);
    let (a, b) = tokio::join!(store.claim(&source), store.claim(&source));
    let mut a = a.unwrap().unwrap();
    let mut b = b.unwrap().unwrap();
    if a.day < b.day {
        std::mem::swap(&mut a, &mut b);
    }
    assert_ne!(a.id, b.id);
    store.finish(&a, &snapshot(10)).await.unwrap();
    assert_eq!(
        store.finish(&a, &snapshot(10)).await,
        Err(SeoAnalyticsError::Stale)
    );
    let report = store.report(1, Provider::Google, None).await.unwrap();
    assert_eq!(report.current.clicks, Some(10));
    assert_eq!(report.current.total_days, 1);
    assert_eq!(report.pages[0].clicks, 2);
    assert_eq!(report.previous.clicks, None);
    assert_eq!(
        store.request_refresh(Provider::Google).await.unwrap(),
        5,
        "running/done jobs coalesce"
    );
    sqlx::query("UPDATE flow_seo_jobs SET lease_until=now()-interval '1 second' WHERE id=$1")
        .bind(b.id)
        .execute(&pool)
        .await
        .unwrap();
    let reclaimed = store.claim(&source).await.unwrap().unwrap();
    assert_eq!(reclaimed.id, b.id);
    assert!(reclaimed.epoch > b.epoch);
    assert_eq!(
        store.finish(&b, &snapshot(77)).await,
        Err(SeoAnalyticsError::Stale)
    );
    assert_eq!(
        store.fail(&b, SeoAnalyticsError::Unavailable).await,
        Err(SeoAnalyticsError::Stale)
    );
    store.finish(&reclaimed, &snapshot(5)).await.unwrap();
    // Replacing the exact day never adds to its earlier successful values.
    sqlx::query("UPDATE flow_seo_jobs SET state='pending',last_attempt_at=now()-interval '1 hour',next_attempt_at=now() WHERE id=$1").bind(a.id).execute(&pool).await.unwrap();
    let replacement = store.claim(&source).await.unwrap().unwrap();
    assert_eq!(replacement.id, a.id);
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE flow_seo_pages IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    assert_eq!(
        store.finish(&replacement, &snapshot(8)).await,
        Err(SeoAnalyticsError::Unavailable)
    );
    lock.rollback().await.unwrap();
    assert_eq!(
        store
            .report(1, Provider::Google, None)
            .await
            .unwrap()
            .current
            .clicks,
        Some(10),
        "failed replacement rolls back both metrics and job state"
    );
    store.finish(&replacement, &snapshot(8)).await.unwrap();
    assert_eq!(
        store
            .report(1, Provider::Google, None)
            .await
            .unwrap()
            .current
            .clicks,
        Some(8)
    );
    sqlx::query("UPDATE flow_seo_jobs SET state='pending',next_attempt_at=now() WHERE id=$1")
        .bind(a.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        sync_one(&store, &FailedProvider, &source).await,
        Err(SeoAnalyticsError::RateLimited)
    );
    let r = store.report(1, Provider::Google, None).await.unwrap();
    assert_eq!(r.current.clicks, Some(8));
    assert_eq!(r.sources[0].last_error.as_deref(), Some("rate_limited"));
    assert!(store.report(8, Provider::Google, None).await.is_err());
    assert!(store
        .report(7, Provider::Google, Some("/admin?token=x"))
        .await
        .is_err());
    for days in [1, 7, 30, 90] {
        assert_eq!(
            store
                .report(days, Provider::Google, None)
                .await
                .unwrap()
                .days,
            days
        )
    }
    let empty = store.report(7, Provider::Yandex, None).await.unwrap();
    assert!(empty.current.clicks.is_none());
    assert!(empty.pages.is_empty());
    let book:i64=sqlx::query_scalar("INSERT INTO books(source_identity,target_language,title,visibility,access_level) VALUES('ci:flow-seo:live','ru','DO_NOT_LEAK_HIDDEN','public','basic') RETURNING id").fetch_one(&pool).await.unwrap();
    sqlx::query("UPDATE flow_seo_jobs SET state='pending',next_attempt_at=now() WHERE id=$1")
        .bind(a.id)
        .execute(&pool)
        .await
        .unwrap();
    let job = store.claim(&source).await.unwrap().unwrap();
    let mut data = snapshot(6);
    data.pages.push(PageMetrics {
        path: format!("/books/{book}"),
        metrics: Metrics {
            clicks: 4,
            impressions: 30,
            position: None,
        },
    });
    data.provider_rows = 2;
    store.finish(&job, &data).await.unwrap();
    assert_eq!(
        store
            .report(1, Provider::Google, None)
            .await
            .unwrap()
            .pages
            .len(),
        2
    );
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    let r = store.report(1, Provider::Google, None).await.unwrap();
    assert_eq!(r.pages.len(), 1);
    assert!(!format!("{r:?}").contains("DO_NOT_LEAK_HIDDEN"));
    assert_eq!(
        r.current.clicks,
        Some(6),
        "provider property totals do not become a sum of currently visible pages"
    );
    // Current source switch cannot allow a stale old-property worker to publish.
    let older = store.claim(&source).await.unwrap().unwrap();
    let next = store
        .configure(SourceKey {
            provider: Provider::Google,
            property: "https://flow-seo-ci-next.test/".into(),
            origin: "https://flow-seo-ci-next.test".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        store.finish(&older, &snapshot(11)).await,
        Err(SeoAnalyticsError::Stale)
    );
    assert!(store
        .report(1, Provider::Google, None)
        .await
        .unwrap()
        .current
        .clicks
        .is_none());
    sqlx::query("DELETE FROM flow_seo_sources WHERE id=ANY($1)")
        .bind(vec![source.id, next.id])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query("SELECT count(*) AS n FROM flow_seo_jobs WHERE source_id=ANY($1)")
            .bind(vec![source.id, next.id])
            .fetch_one(&pool)
            .await
            .unwrap()
            .get::<i64, _>("n"),
        0
    );
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL and current migrations"]
async fn flow_seo_pg_fifty_thousand_pages_keyset_and_select_only_report() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    url_guard::check(&url);
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .unwrap();
    let store = PgSeoAnalytics::new(pool.clone());
    let source = store
        .configure(SourceKey {
            provider: Provider::Google,
            property: "https://flow-seo-scale.test/".into(),
            origin: "https://flow-seo-scale.test".into(),
        })
        .await
        .unwrap();
    // Never hand-assign 50k book IDs: doing so outruns the identity sequence
    // and makes later independent fixtures fail with books_pkey collisions.
    let nonce: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    let prefix = format!("ci:seo:scale:{nonce}:");
    let ids: Vec<i64> = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         SELECT $1 || n::text,'ru','SEO scale','public','basic'
         FROM generate_series(0,49999)n RETURNING id",
    )
    .bind(&prefix)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(ids.len(), 50_000);
    store.request_refresh(Provider::Google).await.unwrap();
    let job = store.claim(&source).await.unwrap().unwrap();
    let data = Snapshot {
        totals: Some(Metrics {
            clicks: 25,
            impressions: 100,
            position: Some(2.0),
        }),
        pages: ids
            .iter()
            .map(|id| PageMetrics {
                path: format!("/books/{id}"),
                metrics: Metrics {
                    clicks: 1,
                    impressions: 10,
                    position: Some(3.0),
                },
            })
            .collect(),
        provider_rows: 50000,
        omitted_rows: 0,
        limit_reached: true,
    };
    store.finish(&job, &data).await.unwrap();
    let t = std::time::Instant::now();
    let a = store.report(90, Provider::Google, None).await.unwrap();
    assert!(t.elapsed() < std::time::Duration::from_secs(3));
    assert_eq!(a.pages.len(), 50);
    assert_eq!(a.current.clicks, Some(25));
    assert_eq!(a.current.limited_days, 1);
    let b = store
        .report(90, Provider::Google, a.next.as_deref())
        .await
        .unwrap();
    assert_eq!(b.pages.len(), 50);
    assert_eq!(b.current.clicks, Some(25));
    assert!(a
        .pages
        .iter()
        .all(|p| !b.pages.iter().any(|q| p.path == q.path)));
    let last = store
        .report(90, Provider::Google, Some("/reader/9223372036854775807"))
        .await
        .unwrap();
    assert!(last.pages.is_empty());
    assert_eq!(last.current.clicks, Some(25));
    // A SELECT-only transaction still returns the same evidence; report performs no collection.
    let reader = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|c, _| {
            Box::pin(async move {
                sqlx::query("SET default_transaction_read_only=on")
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    assert_eq!(
        PgSeoAnalytics::new(reader.clone())
            .report(1, Provider::Google, None)
            .await
            .unwrap()
            .current
            .clicks,
        Some(25)
    );
    reader.close().await;

    // Multiple full 64-path scan windows can be withdrawn. The next
    // visible page must continue past it without leaking its titles.
    let mut ordered: Vec<(String, i64)> =
        ids.iter().map(|id| (format!("/books/{id}"), *id)).collect();
    ordered.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let hidden_ids: Vec<i64> = ordered.iter().take(320).map(|(_, id)| *id).collect();
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=ANY($1)")
        .bind(&hidden_ids)
        .execute(&pool)
        .await
        .unwrap();
    let filtered = store.report(90, Provider::Google, None).await.unwrap();
    assert_eq!(
        filtered.pages.len(),
        50,
        "hidden prefix must not truncate the page"
    );
    assert!(filtered
        .pages
        .iter()
        .all(|p| !ordered[..320].iter().any(|(path, _)| path == &p.path)));

    sqlx::query("DELETE FROM flow_seo_sources WHERE id=$1")
        .bind(source.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE source_identity LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

mod url_guard {
    pub struct Safe {
        pub database: String,
    }
    pub fn check(url: &str) -> Safe {
        use std::str::FromStr;
        let options = sqlx::postgres::PgConnectOptions::from_str(url).unwrap();
        assert_eq!(options.get_host(), "127.0.0.1");
        assert_eq!(options.get_port(), 5432);
        assert_eq!(options.get_database(), Some("tor_ci"));
        assert_eq!(options.get_username(), "tor_app");
        Safe {
            database: "tor_ci".into(),
        }
    }
}

struct AvailableProvider;
#[async_trait]
impl SearchAnalyticsProvider for AvailableProvider {
    async fn collect(&self, day: &str) -> Result<Snapshot, SeoAnalyticsError> {
        assert!(valid_day(day));
        tokio::task::yield_now().await;
        Ok(snapshot(9))
    }
}
#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL and current migrations"]
async fn flow_seo_backfill_is_idempotent_and_expired_old_jobs_are_bounded() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    url_guard::check(&url);
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .unwrap();
    let store = PgSeoAnalytics::new(pool.clone());
    let origin = "https://flow-seo-backfill.test";
    sqlx::query("DELETE FROM flow_seo_sources WHERE origin=$1")
        .bind(origin)
        .execute(&pool)
        .await
        .unwrap();
    let g = store
        .configure(SourceKey {
            provider: Provider::Google,
            property: format!("{origin}/"),
            origin: origin.into(),
        })
        .await
        .unwrap();
    let y = store
        .configure(SourceKey {
            provider: Provider::Yandex,
            property: "https:flow-seo-backfill.test:443".into(),
            origin: origin.into(),
        })
        .await
        .unwrap();
    assert_eq!(store.request_backfill(&g).await.unwrap(), 180);
    assert_eq!(store.request_backfill(&g).await.unwrap(), 0);
    assert_eq!(store.request_backfill(&y).await.unwrap(), 11);
    assert_eq!(store.request_backfill(&y).await.unwrap(), 0);
    assert!(sync_one(&store, &AvailableProvider, &g).await.unwrap());
    let r = store.report(1, Provider::Google, None).await.unwrap();
    assert_eq!(r.current.clicks, Some(9));
    assert_eq!(r.pages[0].clicks, 2);
    assert_eq!(
        store.request_backfill(&g).await.unwrap(),
        0,
        "restart does not reset completed dates"
    );
    let current: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM flow_seo_jobs WHERE source_id=$1 AND state='done'",
    )
    .bind(g.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(current, 1);
    sqlx::query("INSERT INTO flow_seo_jobs(source_id,day,state,epoch,lease_until) VALUES($1,current_date-210,'running',1,now()-interval '1 second'),($1,current_date-211,'running',1,now()+interval '1 hour')")
        .bind(g.id).execute(&pool).await.unwrap();
    store.prune_one().await.unwrap();
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM flow_seo_jobs WHERE source_id=$1 AND day<current_date-200",
    )
    .bind(g.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remaining, 1, "live leases are not pruned");
    sqlx::query("UPDATE flow_seo_sources SET enabled=false WHERE id=$1")
        .bind(y.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(store.request_backfill(&y).await.unwrap(), 0);
    assert!(store.claim(&y).await.unwrap().is_none());
    sqlx::query("DELETE FROM flow_seo_sources WHERE origin=$1")
        .bind(origin)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL and current migrations"]
async fn flow_seo_claim_keeps_google_history_and_yandex_recent_boundaries() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    url_guard::check(&url);
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .unwrap();
    let store = PgSeoAnalytics::new(pool.clone());
    let origin = "https://flow-seo-claim-bounds.test";
    sqlx::query("DELETE FROM flow_seo_sources WHERE origin=$1")
        .bind(origin)
        .execute(&pool)
        .await
        .unwrap();
    let google = store
        .configure(SourceKey {
            provider: Provider::Google,
            property: format!("{origin}/"),
            origin: origin.into(),
        })
        .await
        .unwrap();
    let yandex = store
        .configure(SourceKey {
            provider: Provider::Yandex,
            property: "https:flow-seo-claim-bounds.test:443".into(),
            origin: origin.into(),
        })
        .await
        .unwrap();
    assert_eq!(store.request_backfill(&google).await.unwrap(), 180);
    assert_eq!(store.request_backfill(&yandex).await.unwrap(), 11);
    let bounds = sqlx::query_as::<_, (String, i64, i32, i32)>(
        "SELECT s.provider,count(*),
            min((now() AT TIME ZONE 'UTC')::date-j.day),
            max((now() AT TIME ZONE 'UTC')::date-j.day)
         FROM flow_seo_jobs j JOIN flow_seo_sources s ON s.id=j.source_id
         WHERE s.id=ANY($1) GROUP BY s.provider ORDER BY s.provider",
    )
    .bind(vec![google.id, yandex.id])
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        bounds,
        vec![("google".into(), 180, 3, 182), ("yandex".into(), 11, 3, 13)]
    );

    // Leave each provider's oldest installed job due. A manually queued older
    // job must stay ineligible; it must not prevent the boundary job being claimed.
    sqlx::query(
        "UPDATE flow_seo_jobs SET next_attempt_at=now()+interval '1 hour'
         WHERE source_id=ANY($1)
           AND day>(now() AT TIME ZONE 'UTC')::date-
             (CASE WHEN source_id=$2 THEN 182 ELSE 13 END)",
    )
    .bind(vec![google.id, yandex.id])
    .bind(google.id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO flow_seo_jobs(source_id,day) VALUES
         ($1,(now() AT TIME ZONE 'UTC')::date-183),
         ($2,(now() AT TIME ZONE 'UTC')::date-14)",
    )
    .bind(google.id)
    .bind(yandex.id)
    .execute(&pool)
    .await
    .unwrap();
    let expected = sqlx::query_as::<_, (i64, i64)>(
        "SELECT source_id,id FROM flow_seo_jobs WHERE source_id=ANY($1)
           AND day=(now() AT TIME ZONE 'UTC')::date-
             (CASE WHEN source_id=$2 THEN 182 ELSE 13 END)",
    )
    .bind(vec![google.id, yandex.id])
    .bind(google.id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(expected.len(), 2);
    let google_job = store.claim(&google).await.unwrap().unwrap();
    let yandex_job = store.claim(&yandex).await.unwrap().unwrap();
    assert!(expected.contains(&(google.id, google_job.id)));
    assert!(expected.contains(&(yandex.id, yandex_job.id)));
    assert!(store.claim(&google).await.unwrap().is_none());
    assert!(store.claim(&yandex).await.unwrap().is_none());
    let older_pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM flow_seo_jobs WHERE source_id=ANY($1) AND state='pending'
           AND day<(now() AT TIME ZONE 'UTC')::date-
             (CASE WHEN source_id=$2 THEN 182 ELSE 13 END)",
    )
    .bind(vec![google.id, yandex.id])
    .bind(google.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(older_pending, 2);

    // The oldest Google date contributes to the previous 90-day provider
    // window; successful restart backfill must not reset this completed job.
    store.finish(&google_job, &snapshot(9)).await.unwrap();
    let report = store.report(90, Provider::Google, None).await.unwrap();
    assert_eq!(report.current.clicks, None);
    assert_eq!(report.previous.clicks, Some(9));
    assert_eq!(report.previous.total_days, 1);
    assert_eq!(store.request_backfill(&google).await.unwrap(), 0);
    assert_eq!(store.request_backfill(&yandex).await.unwrap(), 0);
    sqlx::query("DELETE FROM flow_seo_sources WHERE origin=$1")
        .bind(origin)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}
