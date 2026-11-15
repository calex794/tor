//! Cross-lane HTTP/PG acceptance; SQL fixtures use psql, never a web SQLx dep.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use std::{
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};
use tor_ads::repositories::paid::PgPaidAds;
use tor_books::repositories::PgCatalog;
use tor_contracts::ads::AdActor;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;
const ORIGIN: &str = "http://127.0.0.1:3116";
fn pg(sql: &str) -> String {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"));
    let out = Command::new("psql")
        .args([
            "-XAtq",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            &url,
            "-c",
            sql,
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
struct Fixture {
    app: Router,
    ads: PgPaidAds,
    actor: AdActor,
    cookies: String,
    other: String,
    other_actor: AdActor,
    csrf: String,
    book: i64,
    package: i64,
}
async fn setup() -> Fixture {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let ads = PgPaidAds::new(pool.clone());
    let accounts = PgAccounts::from_pool(pool.clone());
    let reader = PgReader::new(pool.clone());
    let csrf = generate_session_token();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user = |label: &str| {
        let token = generate_session_token();
        let digest = session_token_digest(&token);
        let id:i64=pg(&format!("INSERT INTO users(email_normalized,display_name,password_hash) VALUES('ads-http-{label}-{stamp}@example.invalid','Ads HTTP','$argon2id$ci') RETURNING id")).parse().unwrap();
        pg(&format!("INSERT INTO sessions(user_id,token_hash,expires_at) VALUES({id},decode('{}','hex'),now()+interval '1 hour')",hex::encode(digest)));
        (
            AdActor {
                user_id: id,
                session_digest: digest,
            },
            format!("tor-dev-session={token}; tor-dev-csrf={csrf}"),
        )
    };
    let (actor, cookies) = user("owner");
    let (other_actor, other) = user("other");
    let owner = actor.user_id;
    let book=pg(&format!("INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id) VALUES('ci:ads-http:{stamp}','ru','Ads <safe>','public','basic',{owner}) RETURNING id")).parse().unwrap();
    let package=pg(&format!("WITH new_package AS (INSERT INTO ad_packages(current_revision,enabled,created_by) VALUES(1,true,{owner}) RETURNING id) INSERT INTO ad_package_revisions(package_id,revision,price_minor,price_micros_at_offer,target_impressions,publisher_share_bps,pricing_algorithm,hold_seconds,daily_cap,hourly_cap,session_cap,terms_version,published_by) SELECT id,1,101,1010000,4,7000,'cumulative_minor_v1',0,100,100,100,'ci-http-1',{owner} FROM new_package RETURNING package_id")).parse().unwrap();
    pg(&format!("INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) VALUES({owner},10000,'RUB')"));
    let app =
        tor_web::router_with_postgres_reading_auth(catalog, reader, accounts, false, ORIGIN.into());
    Fixture {
        app,
        ads,
        actor,
        cookies,
        other,
        other_actor,
        csrf,
        book,
        package,
    }
}
async fn send(
    f: &Fixture,
    method: &str,
    path: &str,
    body: serde_json::Value,
    cookie: &str,
    origin: &str,
    key: &str,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", cookie)
        .header("origin", origin)
        .header("content-type", "application/json")
        .header("x-csrf-token", &f.csrf)
        .header("idempotency-key", key)
        .body(if method == "GET" {
            Body::empty()
        } else {
            Body::from(body.to_string())
        })
        .unwrap();
    let response = f.app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}
#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL and explicit purchase flag"]
async fn paid_ads_http_enabled_error_contract() {
    let f = setup().await;
    let body = serde_json::json!({"promoted_book_id":f.book.to_string(),"package_id":f.package.to_string(),"expected_package_revision":1});
    let key = format!("{}8", "B".repeat(42));
    let mut unknown = body.clone();
    unknown["command_key"] = serde_json::json!(key);
    for (payload, header, expected_status, expected_code) in [
        (
            body.to_string(),
            Some("bad"),
            StatusCode::UNPROCESSABLE_ENTITY,
            "INVALID_IDEMPOTENCY_KEY",
        ),
        (
            body.to_string(),
            None,
            StatusCode::UNPROCESSABLE_ENTITY,
            "IDEMPOTENCY_KEY_REQUIRED",
        ),
        (
            unknown.to_string(),
            Some(key.as_str()),
            StatusCode::UNPROCESSABLE_ENTITY,
            "UNKNOWN_FIELD",
        ),
        (
            "{".into(),
            Some(key.as_str()),
            StatusCode::BAD_REQUEST,
            "MALFORMED_REQUEST",
        ),
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v1/ads/chapter/campaigns")
            .header("cookie", &f.cookies)
            .header("origin", ORIGIN)
            .header("content-type", "application/json")
            .header("x-csrf-token", &f.csrf);
        if let Some(value) = header {
            request = request.header("idempotency-key", value);
        }
        let response = f
            .app
            .clone()
            .oneshot(request.body(Body::from(payload)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected_status);
        let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
        let error: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(error["error"]["code"], expected_code);
        assert!(!error["error"]["request_id"].as_str().unwrap().is_empty());
    }
    assert_eq!(
        pg(&format!(
            "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={}",
            f.actor.user_id
        )),
        "10000"
    );
}
#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL; flags absent"]
async fn paid_ads_http_purchase_off_by_default() {
    assert!(std::env::var("TOR_CHAPTER_ADS_PURCHASE_ENABLED").as_deref() != Ok("1"));
    let f = setup().await;
    let body = serde_json::json!({"promoted_book_id":f.book.to_string(),"package_id":f.package.to_string(),"expected_package_revision":1});
    assert_eq!(
        send(
            &f,
            "POST",
            "/api/v1/ads/chapter/campaigns",
            body,
            &f.cookies,
            ORIGIN,
            &format!("{}8", "A".repeat(42))
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        pg(&format!(
            "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={}",
            f.actor.user_id
        )),
        "10000"
    );
}
#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL and explicit purchase flag"]
async fn paid_ads_http_enabled_roles_origin_keys_snapshot_and_bounded_dashboard() {
    assert_eq!(
        std::env::var("TOR_CHAPTER_ADS_PURCHASE_ENABLED").as_deref(),
        Ok("1")
    );
    let f = setup().await;
    let key = format!("{}8", "B".repeat(42));
    let body = serde_json::json!({"promoted_book_id":f.book.to_string(),"package_id":f.package.to_string(),"expected_package_revision":1});
    assert_eq!(
        send(
            &f,
            "POST",
            "/api/v1/ads/chapter/campaigns",
            body.clone(),
            "",
            ORIGIN,
            &key
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &f,
            "POST",
            "/api/v1/ads/chapter/campaigns",
            body.clone(),
            &f.cookies,
            "https://other.invalid",
            &key
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, receipt) = send(
        &f,
        "POST",
        "/api/v1/ads/chapter/campaigns",
        body.clone(),
        &f.cookies,
        ORIGIN,
        &key,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{receipt}");
    let receipt: serde_json::Value = serde_json::from_str(&receipt).unwrap();
    let campaign = receipt["campaign_id"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let (code, reply) = send(
        &f,
        "POST",
        "/api/v1/ads/chapter/campaigns",
        body.clone(),
        &f.cookies,
        ORIGIN,
        &key,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert!(reply.contains("\"replayed\":true"));
    let changed = serde_json::json!({"promoted_book_id":f.book.to_string(),"package_id":f.package.to_string(),"expected_package_revision":2});
    assert_eq!(
        send(
            &f,
            "POST",
            "/api/v1/ads/chapter/campaigns",
            changed,
            &f.cookies,
            ORIGIN,
            &key
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &f,
            "GET",
            &format!("/api/v1/ads/chapter/campaigns/{campaign}"),
            serde_json::Value::Null,
            &f.other,
            ORIGIN,
            &key
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    pg(&format!("INSERT INTO ad_delivery_hourly_stats(campaign_id,hour_start,accepted) SELECT {campaign},'2000-01-01'::timestamptz+n*interval '1 hour',1 FROM generate_series(1,100000) n"));
    let (code, report) = send(
        &f,
        "GET",
        "/api/v1/me/ads/chapter-campaigns",
        serde_json::Value::Null,
        &f.cookies,
        ORIGIN,
        &key,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    let report: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(report["campaigns"].as_array().unwrap().len(), 1);
    let (code, page) = send(
        &f,
        "GET",
        "/me/ads",
        serde_json::Value::Null,
        &f.cookies,
        ORIGIN,
        &key,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert!(page.contains("Ads &lt;safe&gt;"));
    assert!(!page.contains("100000</"));
    let (code, _) = send(
        &f,
        "GET",
        "/admin/promotion/paid",
        serde_json::Value::Null,
        &f.cookies,
        ORIGIN,
        &key,
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    // An incomplete generation cannot replace the previous active one.
    loop {
        let (_, complete) = f.ads.plan_batch(10).await.unwrap();
        if complete {
            break;
        }
    }
    let active = pg("SELECT generation_id FROM ad_delivery_active");
    pg("INSERT INTO ad_delivery_generations(host_ceiling,campaign_ceiling) VALUES(999999999,999999999)");
    assert_eq!(pg("SELECT generation_id FROM ad_delivery_active"), active);
    assert_eq!(
        pg(&format!(
            "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={}",
            f.actor.user_id
        )),
        "9899"
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL and explicit purchase flag"]
async fn paid_ads_http_enabled_outcomes_require_committed_c_and_d_facts() {
    use tor_commerce::{
        domain::money::{
            CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId,
        },
        repositories::checkout::PgCheckout,
    };
    use tor_contracts::{
        ads::*,
        paid_delivery::{ChapterVersionRef, PositiveId},
    };
    let f = setup().await;
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let funding = tor_commerce::repositories::ad_posting::PgAdPosting::new(pool.clone());
    let receipt = funding
        .purchase(&AdPurchaseCommand {
            actor: f.actor.clone(),
            command_key: generate_session_token(),
            purchase: AdPurchase::Chapter(ChapterPurchase {
                book_id: f.book,
                package_id: f.package,
                expected_package_revision: 1,
            }),
        })
        .await
        .unwrap();
    let publisher:i64=pg(&format!("INSERT INTO users(email_normalized,display_name,password_hash) VALUES('ads-outcome-publisher-{}@example.invalid','Publisher','$argon2id$ci') RETURNING id",f.actor.user_id)).parse().unwrap();
    let host:i64=pg(&format!("INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id) VALUES('ci:ads-outcome-host:{publisher}','ru','Host','public','basic',{publisher}) RETURNING id")).parse().unwrap();
    pg(&format!(
        "INSERT INTO ad_publisher_settings(user_id,enabled) VALUES({publisher},true)"
    ));
    let chapter:i64=pg(&format!("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level) VALUES({host},1,'Host free','public','free') RETURNING id")).parse().unwrap();
    let version:i64=pg(&format!("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) VALUES({chapter},1,'Ready') RETURNING id")).parse().unwrap();
    pg(&format!(
        "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
    ));
    let generation:i64=pg(&format!("INSERT INTO ad_delivery_generations(status,host_ceiling) VALUES('active',{host}) RETURNING id")).parse().unwrap();
    pg(&format!("INSERT INTO ad_delivery_active(singleton,generation_id) VALUES(true,{generation}) ON CONFLICT(singleton) DO UPDATE SET generation_id={generation}"));
    let assignment:i64=pg(&format!("INSERT INTO ad_delivery_assignments(generation_id,host_book_id,publisher_id,campaign_id,rank) VALUES({generation},{host},{publisher},{},1) RETURNING id",receipt.campaign_id.get())).parse().unwrap();
    let token = generate_session_token();
    f.ads
        .issue_opportunity(&f.other_actor, assignment, chapter, &token)
        .await
        .unwrap();
    f.ads.click(&f.other_actor, &token).await.unwrap();
    f.ads.open(&f.other_actor, &token, f.book).await.unwrap();
    f.ads.reconcile_outcomes(100).await.unwrap();
    let campaign = receipt.campaign_id.get();
    assert_eq!(pg(&format!("SELECT clicks||'|'||opens||'|'||reads||'|'||library_adds||'|'||purchases FROM ad_campaign_stats WHERE campaign_id={campaign}")),"1|1|0|0|0");
    let reader = f.other_actor.user_id;
    let free:i64=pg(&format!("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level) VALUES({},1,'Free','public','free') RETURNING id",f.book)).parse().unwrap();
    let free_version:i64=pg(&format!("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) VALUES({free},1,'Ready') RETURNING id")).parse().unwrap();
    pg(&format!(
        "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({free},{free_version})"
    ));
    tor_reading::repositories::library::PgLibrary::new(pool.clone())
        .add_book(reader, f.book)
        .await
        .unwrap();
    tor_reading::repositories::sync::PgReadingSync::new(pool.clone())
        .save_free(
            reader,
            free,
            free_version,
            tor_reading::services::progress::ReadingPlace {
                block_index: 1,
                char_offset: 5,
            },
            0,
        )
        .await
        .unwrap();
    let paid:i64=pg(&format!("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) VALUES({},2,'Paid','public','paid',50) RETURNING id",f.book)).parse().unwrap();
    let paid_version:i64=pg(&format!("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) VALUES({paid},1,'Private paid fixture') RETURNING id")).parse().unwrap();
    pg(&format!("INSERT INTO chapter_publications(chapter_id,version_id) VALUES({paid},{paid_version}); INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) VALUES({reader},1000,'RUB')"));
    let checkout = PurchaseCommand::new(
        PositiveId::new(reader).unwrap(),
        WalletId::new(reader).unwrap(),
        ChapterVersionRef {
            chapter_id: PositiveId::new(paid).unwrap(),
            version_id: PositiveId::new(paid_version).unwrap(),
        },
        MoneyMinor::new(50).unwrap(),
        IdempotencyKey::parse(&generate_session_token()).unwrap(),
    )
    .unwrap();
    PgCheckout::new(pool.clone())
        .checkout_once(&checkout)
        .await
        .unwrap();
    pg("UPDATE ad_attribution_pending SET next_check_at=clock_timestamp()");
    f.ads.reconcile_outcomes(100).await.unwrap();
    assert_eq!(pg(&format!("SELECT reads||'|'||library_adds||'|'||purchases FROM ad_campaign_stats WHERE campaign_id={campaign}")),"1|1|1");
    pg("UPDATE ad_attribution_pending SET next_check_at=clock_timestamp()");
    f.ads.reconcile_outcomes(100).await.unwrap();
    assert_eq!(pg(&format!("SELECT reads||'|'||library_adds||'|'||purchases FROM ad_campaign_stats WHERE campaign_id={campaign}")),"1|1|1");
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL and explicit purchase flag"]
async fn paid_ads_http_enabled_daily_frequency_is_durable_and_editorial_priority_does_not_consume_it(
) {
    use tor_contracts::{ads::*, exposure::PromotionSlot};
    let f = setup().await;
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let funding = tor_commerce::repositories::ad_posting::PgAdPosting::new(catalog.pool().clone());
    let owner = f.actor.user_id;
    let offer:i64=pg(&format!("WITH created AS (INSERT INTO ad_daily_offers(current_revision,enabled,created_by) VALUES(1,true,{owner}) RETURNING id) INSERT INTO ad_daily_offer_revisions(offer_id,revision,slot,rate_minor,min_days,max_days,cancellation_policy,terms_version,published_by,daily_cap,hourly_cap,session_cap,min_interval_seconds) SELECT id,1,'home_featured',100,1,30,'unused_full_utc_days','ci-frequency-v1',{owner},2,2,2,3600 FROM created RETURNING offer_id")).parse().unwrap();
    let start = pg("SELECT to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD')||'T00:00:00Z'");
    let end =
        pg("SELECT to_char(now() AT TIME ZONE 'UTC'+interval '1 day','YYYY-MM-DD')||'T00:00:00Z'");
    funding
        .purchase(&AdPurchaseCommand {
            actor: f.actor.clone(),
            command_key: generate_session_token(),
            purchase: AdPurchase::Daily(DailyPurchase {
                book_id: f.book,
                offer_id: offer,
                offer_revision: 1,
                placement: PromotionSlot::HomeFeatured,
                starts_at: start,
                ends_at: end,
                total_days: 1,
                prepaid_days: 1,
            }),
        })
        .await
        .unwrap();
    pg(&format!("INSERT INTO ad_slot_policies(slot,paid_first,daily_enabled,chapter_enabled,max_cards,revision,updated_by) VALUES('home_featured',false,true,false,4,1,{owner}) ON CONFLICT(slot) DO UPDATE SET paid_first=false,daily_enabled=true"));
    let flags = tor_ads::domain::paid::AdFlags {
        daily_serving: true,
        ..Default::default()
    };
    assert!(f
        .ads
        .cards(
            Some(&f.actor),
            PromotionSlot::HomeFeatured,
            None,
            flags,
            true
        )
        .await
        .unwrap()
        .1
        .is_empty());
    assert_eq!(
        f.ads
            .cards(
                Some(&f.actor),
                PromotionSlot::HomeFeatured,
                None,
                flags,
                false
            )
            .await
            .unwrap()
            .1
            .len(),
        1
    );
    let restarted = PgPaidAds::new(catalog.pool().clone());
    assert!(restarted
        .cards(
            Some(&f.actor),
            PromotionSlot::HomeFeatured,
            None,
            flags,
            false
        )
        .await
        .unwrap()
        .1
        .is_empty());
    assert_eq!(
        pg(&format!(
            "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={owner}"
        )),
        "9900"
    );
}
