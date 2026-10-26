//! PG18 + Axum: actual SQLx query completions are counted per route, not SQL.
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::time::Duration;
use tor_books::repositories::PgCatalog;
use tor_traffic_behavior::{repositories::PgRilRollups, RouteCode};
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires isolated PG18, RIL SQLx migration and real HTTP router"]
async fn route_sqlx_completion_measurements_survive_background_pg_flush() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG test URL");
    assert!(url.ends_with("/tor_ci"), "never inspect production PG");
    assert!(tor_web::activate_ril_sqlx_observation());
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let repo = PgRilRollups::new(catalog.pool());
    let before = repo.read_sql_costs(1).await.unwrap();
    let old = |route| {
        before
            .iter()
            .find(|r| r.route == route)
            .map(|r| (r.observed_requests, r.observed_queries, r.sqlx_elapsed_us))
            .unwrap_or((0, 0, 0))
    };
    let old_catalog = old(RouteCode::ApiBooks as i16);
    let old_health = old(RouteCode::Health as i16);

    tor_web::start_ril_pg_background(&catalog);
    let app = tor_web::router_with_postgres(catalog.clone());
    for path in ["/api/v1/books", "/api/v1/books", "/healthz"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
    // Production rollup pump runs out of HTTP, every 10s, with one UUID batch.
    tokio::time::timeout(Duration::from_secs(22), async {
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let rows = repo.read_sql_costs(1).await.unwrap();
            let catalog = rows.iter().find(|r| r.route == RouteCode::ApiBooks as i16);
            let health = rows.iter().find(|r| r.route == RouteCode::Health as i16);
            if let (Some(catalog), Some(health)) = (catalog, health) {
                if catalog.observed_requests >= old_catalog.0 + 2
                    && health.observed_requests > old_health.0
                {
                    assert!(
                        catalog.observed_queries >= old_catalog.1 + 2,
                        "actual PostgreSQL queries not counted for GET /api/v1/books"
                    );
                    assert!(catalog.sqlx_elapsed_us >= old_catalog.2);
                    assert_eq!(health.observed_queries - old_health.1, 0);
                    return;
                }
            }
        }
    })
    .await
    .expect("RIL background SQLx measurement flush");
}
