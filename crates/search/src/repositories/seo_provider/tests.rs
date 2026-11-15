use super::*;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    routing::any,
    Router,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
#[derive(Clone)]
struct Fixture {
    responses: Arc<Mutex<VecDeque<(StatusCode, Value)>>>,
    requests: Arc<Mutex<Vec<(String, String, Value)>>>,
}
async fn wire(
    State(f): State<Fixture>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, axum::Json<Value>) {
    let auth = headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    f.requests
        .lock()
        .unwrap()
        .push((uri.to_string(), auth, value));
    let (status, value) = f
        .responses
        .lock()
        .unwrap()
        .pop_front()
        .expect("bounded expected requests");
    (status, axum::Json(value))
}
async fn server(
    provider: Provider,
    responses: Vec<(StatusCode, Value)>,
) -> (ProviderClient, Fixture, tokio::task::JoinHandle<()>) {
    let f = Fixture {
        responses: Arc::new(Mutex::new(responses.into())),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new().fallback(any(wire)).with_state(f.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let key = SourceKey {
        provider,
        origin: "https://example.com".into(),
        property: if provider == Provider::Google {
            "https://example.com/"
        } else {
            "https:example.com:443"
        }
        .into(),
    };
    let mut client = ProviderClient::new(key, "test-secret-never-stored", Some(123)).unwrap();
    client.base = format!("http://{address}");
    (client, f, task)
}
fn google_total() -> Value {
    json!({"responseAggregationType":"byProperty","rows":[{"clicks":10.0,"impressions":100.0,"position":4.0}]})
}
#[test]
fn accepts_only_exact_public_paths_dates_and_configured_properties() {
    for path in ["/", "/catalog", "/books/7", "/reader/52"] {
        assert_eq!(
            canonical_path("https://example.com", &format!("https://example.com{path}")),
            Some(path.into())
        );
    }
    for url in [
        "https://example.com.evil/books/7",
        "http://example.com/books/7",
        "https://u@example.com/books/7",
        "https://example.com/books/7?token=secret",
        "https://example.com/books/7#x",
        "https://example.com/books/007",
        "https://example.com/books/9223372036854775808",
        "https://example.com/reader/0",
        "https://example.com/books/%37",
        "https://example.com/admin",
        "https://example.com/books/1/../7",
    ] {
        assert_eq!(canonical_path("https://example.com", url), None, "{url}");
    }
    for date in ["2024-02-29", "2026-11-11", "2000-02-29"] {
        assert!(valid_day(date));
    }
    for date in [
        "2026-02-29",
        "2100-02-29",
        "2026-1-01",
        "2026-11-31",
        "2026-00-01",
        "2026-12-00",
        "2026-11-11' OR true",
    ] {
        assert!(!valid_day(date));
    }
    assert!(ProviderClient::new(
        SourceKey {
            provider: Provider::Google,
            property: "sc-domain:evil.com".into(),
            origin: "https://example.com".into()
        },
        "test-token",
        None
    )
    .is_err());
    assert_eq!(parse::count(&json!(1.5)), Err(E::InvalidResponse));
    assert_eq!(parse::count(&json!(-1)), Err(E::InvalidResponse));
    assert_eq!(parse::count(&json!(MAX_COUNT + 1)), Err(E::InvalidResponse));
}
#[tokio::test]
async fn google_wire_separates_property_totals_discards_noncanonical_and_uses_final_pages() {
    let (client,f,task)=server(Provider::Google,vec![(StatusCode::OK,google_total()),(StatusCode::OK,json!({"responseAggregationType":"byPage","rows":[
        {"keys":["https://example.com/books/7"],"clicks":3,"impressions":20,"position":5},
        {"keys":["https://example.com/books/7?user=DO_NOT_STORE"],"clicks":1,"impressions":2},
        {"keys":["https://evil.com/books/7"],"clicks":1,"impressions":2}
    ]}))]).await;
    let result = client.collect("2026-11-08").await.unwrap();
    assert_eq!(result.totals.as_ref().unwrap().clicks, 10);
    assert_eq!(result.pages.len(), 1);
    assert_eq!(result.pages[0].metrics.clicks, 3);
    assert_eq!(result.omitted_rows, 2);
    assert!(!format!("{result:?}").contains("DO_NOT_STORE"));
    let req = f.requests.lock().unwrap();
    assert_eq!(req.len(), 2);
    assert!(req[0].0.contains("https%3A%2F%2Fexample.com%2F"));
    assert_eq!(req[0].1, "Bearer test-secret-never-stored");
    assert!(req[0].2.get("dimensions").is_none());
    assert_eq!(req[1].2["dimensions"], json!(["page"]));
    assert_eq!(req[1].2["dataState"], "final");
    assert_eq!(req[1].2["aggregationType"], "auto");
    assert_eq!(req[1].2["type"], "web");
    task.abort();
}
#[tokio::test]
async fn google_paginates_without_adding_totals_twice() {
    let rows:Vec<_>=(1..=25000).map(|id|json!({"keys":[format!("https://example.com/books/{id}")],"clicks":0,"impressions":1})).collect();
    let (client, f, task) = server(
        Provider::Google,
        vec![
            (StatusCode::OK, google_total()),
            (StatusCode::OK, json!({"rows":rows})),
            (StatusCode::OK, json!({})),
        ],
    )
    .await;
    let result = client.collect("2026-11-08").await.unwrap();
    assert_eq!(result.pages.len(), 25000);
    assert!(!result.limit_reached);
    assert_eq!(result.totals.unwrap().impressions, 100);
    assert_eq!(f.requests.lock().unwrap()[2].2["startRow"], 25000);
    task.abort();
}
#[tokio::test]
async fn missing_and_rejected_sources_are_not_zero_or_success() {
    let (client, _, task) = server(
        Provider::Google,
        vec![(StatusCode::OK, json!({})), (StatusCode::OK, json!({}))],
    )
    .await;
    let result = client.collect("2026-11-08").await.unwrap();
    assert!(!result.available());
    assert!(result.totals.is_none());
    task.abort();
    for (status, error) in [
        (StatusCode::UNAUTHORIZED, E::Unauthorized),
        (StatusCode::FORBIDDEN, E::Unauthorized),
        (StatusCode::NOT_FOUND, E::Unauthorized),
        (StatusCode::TOO_MANY_REQUESTS, E::RateLimited),
        (StatusCode::FOUND, E::Unavailable),
        (StatusCode::BAD_GATEWAY, E::Unavailable),
    ] {
        let (client, _, task) = server(
            Provider::Google,
            vec![(status, json!({"error":"DO_NOT_EXPOSE_SECRET"}))],
        )
        .await;
        assert_eq!(client.collect("2026-11-08").await.unwrap_err(), error);
        task.abort();
    }
}
#[tokio::test]
async fn yandex_keeps_provider_date_uses_url_web_scope_and_ignores_queries() {
    let response = json!({"count":2,"text_indicator_to_statistics":[
      {"text_indicator":{"type":"URL","value":"https://example.com/books/7"},"popular_complementary_indicator":{"type":"QUERY","value":"DO_NOT_STORE_QUERY"},"statistics":[
        {"date":"2026-11-08","field":"CLICKS","value":2.0},{"date":"2026-11-08","field":"IMPRESSIONS","value":10.0},{"date":"2026-11-08","field":"POSITION","value":3.0},
        {"date":"2026-11-07","field":"CLICKS","value":9.0}]},
      {"text_indicator":{"type":"URL","value":"https://example.com/catalog"},"statistics":[{"date":"2026-11-07","field":"CLICKS","value":5.0}]}
    ]});
    let total = json!({"indicators":{"TOTAL_CLICKS":[{"date":"2026-11-08T00:00:00.000+03:00","value":20.0}],"TOTAL_SHOWS":[{"date":"2026-11-08T00:00:00.000+03:00","value":100.0}]}});
    let (client, f, task) = server(
        Provider::Yandex,
        vec![(StatusCode::OK, total), (StatusCode::OK, response)],
    )
    .await;
    let result = client.collect("2026-11-08").await.unwrap();
    assert_eq!(result.pages[0].metrics.clicks, 2);
    assert_eq!(result.pages.len(), 1);
    assert_eq!(result.omitted_rows, 1);
    assert_eq!(result.totals.unwrap().clicks, 20);
    assert!(!format!("{:?}", result.pages).contains("QUERY"));
    let req = f.requests.lock().unwrap();
    assert_eq!(req[0].1, "OAuth test-secret-never-stored");
    assert_eq!(req[1].2["text_indicator"], "URL");
    assert_eq!(req[1].2["search_location"], "WEB_LOCATION");
    assert_eq!(req[1].2["limit"], 500);
    task.abort();
}
#[test]
fn conflicting_duplicate_pages_and_invalid_counts_are_rejected() {
    let mut pages = BTreeMap::new();
    let m = Metrics {
        clicks: 2,
        impressions: 10,
        position: None,
    };
    assert!(parse::insert(
        &mut pages,
        "https://example.com",
        "https://example.com/books/7",
        m.clone()
    )
    .unwrap());
    assert!(parse::insert(
        &mut pages,
        "https://example.com",
        "https://example.com/books/7",
        m
    )
    .unwrap());
    assert_eq!(pages.len(), 1);
    assert_eq!(
        parse::insert(
            &mut pages,
            "https://example.com",
            "https://example.com/books/7",
            Metrics {
                clicks: 3,
                impressions: 10,
                position: None
            }
        ),
        Err(E::InvalidResponse)
    );
    assert_eq!(
        parse::google_metrics(&json!({"clicks":2,"impressions":1})),
        Err(E::InvalidResponse)
    );
}
