//! D-05: real active PACK + paid historical Layer + actual owner deletion.
//! This fixture builds on the genuine DROP-04b MinIO pack finalizer and
//! STOPs before touching any production source or migration.
use super::{pg, BUCKET};
use aws_sdk_s3::Client;
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
};
use sha2::{Digest, Sha256};
use std::num::NonZeroU32;
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::{postgres::PgAuthoring, PgCatalog};
use tor_commerce::{
    domain::{
        layers::LayerCheckoutCommand,
        money::{IdempotencyKey, MoneyMinor, WalletId},
    },
    repositories::layers::PgLayers,
};
use tor_contracts::paid_delivery::PositiveId;
use tower::ServiceExt;

mod live_pack_chromium;

fn id(raw: i64) -> PositiveId {
    PositiveId::new(raw).unwrap()
}

async fn user(accounts: &PgAccounts, label: &str, book: i64) -> (i64, String) {
    let session = generate_session_token();
    let password_hash = hash_password("D05 archived PACK actor test password").unwrap();
    let actor = accounts
        .register_with_session(
            &format!("d05-layer-pack-{label}-{book}@example.invalid"),
            "D05 historical PACK actor",
            &password_hash,
            &session_token_digest(&session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    (actor, session)
}

async fn post(
    app: &axum::Router,
    path: &str,
    session: Option<&str>,
    csrf: Option<&str>,
    origin: Option<&str>,
    extra_range: bool,
) -> axum::response::Response {
    let mut builder = Request::builder().method(Method::POST).uri(path);
    if let Some(session) = session {
        let cookie = match csrf {
            Some(csrf) => format!("tor-dev-session={session}; tor-dev-csrf={csrf}"),
            None => format!("tor-dev-session={session}"),
        };
        builder = builder.header(header::COOKIE, cookie);
    }
    if let Some(csrf) = csrf {
        builder = builder.header("x-csrf-token", csrf);
    }
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    if extra_range {
        builder = builder.header(header::RANGE, "bytes=0-1000000");
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

pub(super) async fn verify(
    url: &str,
    client: &Client,
    owner: i64,
    book: i64,
    receipt: &tor_contracts::drop_pack::PackPlacementReceipt,
    chapters: &[tor_books::services::chapter_file_import::ImportedChapter],
) {
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
    let catalog = PgCatalog::connect(url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());

    // The preceding DROP-04c test edited chapter two from PACK V1 into
    // SINGLE V2. Re-publish the still-verified immutable PACK V1 as the
    // deliberate current publication for this *isolated* archive fixture.
    // The archive must freeze the currently published edition, not select an
    // old V1 merely because an S3 object happens to remain.
    let frame = &receipt.frames[1];
    let expected = chapters[1].body.as_bytes();
    assert_eq!(
        <[u8; 32]>::from(Sha256::digest(expected)),
        frame.content_sha256
    );
    pg(
        url,
        &format!(
            "UPDATE chapter_publications SET version_id={} WHERE chapter_id={}",
            frame.version_id, frame.chapter_id,
        ),
    );
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM free_chapter_storage_objects
                 WHERE chapter_id={} AND version_id={}",
                frame.chapter_id, frame.version_id
            ),
        ),
        "0",
        "PACK-only V1 must not have any SINGLE receipt"
    );
    let pack = tor_reading::repositories::free_s3_ref::PgFreeS3Ref::new(pool.clone())
        .archived_free_active_pack_metadata_only(frame.chapter_id, frame.version_id)
        .await
        .unwrap()
        .expect("real active PACK physical generation is present");
    assert_eq!(pack.sha256, frame.content_sha256);

    let owner_hash = hash_password("D05 real owner deletion password").unwrap();
    // The DROP import fixture created an account without a usable password;
    // upgrade only this disposable CI actor to run the real author contract.
    pg(
        url,
        &format!("UPDATE users SET password_hash='{owner_hash}' WHERE id={owner}"),
    );
    let owner_session = generate_session_token();
    let owner_token_hash = session_token_digest(&owner_session);
    pg(
        url,
        &format!(
            "INSERT INTO sessions(user_id,token_hash,expires_at)
             VALUES({owner},decode('{}','hex'),now()+interval '1 hour')",
            hex::encode(owner_token_hash)
        ),
    );
    let (buyer, buyer_session) = user(&accounts, "buyer", book).await;
    let (_, unrelated_session) = user(&accounts, "unrelated", book).await;
    let (moderator, moderator_session) = user(&accounts, "moderator", book).await;
    pg(
        url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({moderator},'moderator',true)"
        ),
    );
    let layers = PgLayers::new(pool.clone());
    let layer = layers
        .create_layer(id(owner), "Historical PACK snapshot")
        .await
        .unwrap();
    let version = layers
        .add_book(id(owner), id(layer), id(book))
        .await
        .unwrap();
    let offer = layers
        .publish_offer(id(owner), id(layer), 90, true)
        .await
        .unwrap();
    pg(
        url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES({buyer},500,'RUB')"
        ),
    );
    let term = layers
        .buy(&LayerCheckoutCommand {
            actor: id(buyer),
            wallet: WalletId::new(buyer).unwrap(),
            layer: id(layer),
            offer: id(offer),
            version: id(version),
            expected_price: MoneyMinor::new(90).unwrap(),
            key: IdempotencyKey::parse(&format!("{}8", "V".repeat(42))).unwrap(),
        })
        .await
        .unwrap();

    let author = PgAuthoring::new(pool.clone());
    let rev = author
        .ordinary_deletion_view(owner, book)
        .await
        .unwrap()
        .unwrap()
        .revision;
    let pending = author
        .advance_ordinary_deletion(owner, book, rev, "start", &owner_token_hash, &owner_hash)
        .await
        .unwrap();
    accounts
        .moderate_ordinary_deletion(
            &session_token_digest(&moderator_session),
            book,
            pending,
            "approve",
            "",
        )
        .await
        .unwrap();
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT version_id FROM book_layer_free_archive_versions
                 WHERE book_id={book} AND chapter_id={}",
                frame.chapter_id
            ),
        ),
        frame.version_id.to_string()
    );

    // Actor-scoped HTTP route, exact FRAME only; all transport keys and
    // Range offsets stay inside the service.
    std::env::set_var("TOR_LAYER_FREE_ARCHIVE_ENABLED", "1");
    let pack_app = tor_web::free_pack_http::router(
        accounts.clone(),
        false,
        "http://127.0.0.1:3098".into(),
        client.clone(),
        BUCKET.into(),
    )
    .unwrap();
    let free_app = tor_web::free_reader_http::router(
        accounts.clone(),
        false,
        "http://127.0.0.1:3098".into(),
        tor_storage::repositories::free_chapter_s3::S3FreeChapterStore::new(
            client.clone(),
            BUCKET.into(),
        )
        .unwrap(),
        BUCKET.into(),
        super::ENDPOINT.into(),
    )
    .unwrap();
    let app = pack_app.merge(free_app);
    std::env::remove_var("TOR_LAYER_FREE_ARCHIVE_ENABLED");
    let path = format!("/api/v1/reader/layer/free/{}/pack", frame.chapter_id);
    let csrf = generate_session_token();
    assert_eq!(
        post(&app, &path, None, None, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(&app, &path, Some(&buyer_session), Some(&csrf), None, false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(
            &app,
            &path,
            Some(&unrelated_session),
            Some(&csrf),
            Some("http://127.0.0.1:3098"),
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post(
            &app,
            &path,
            Some(&buyer_session),
            Some(&csrf),
            Some("http://127.0.0.1:3098"),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let response = post(
        &app,
        &path,
        Some(&buyer_session),
        Some(&csrf),
        Some("http://127.0.0.1:3098"),
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        response.headers()["x-tor-free-version-id"],
        frame.version_id.to_string()
    );
    assert_eq!(
        response.headers()["x-tor-free-sha256"],
        hex::encode(frame.content_sha256)
    );
    assert!(!response.headers().contains_key(header::LOCATION));
    let bytes = to_bytes(response.into_body(), 200_001).await.unwrap();
    assert_eq!(
        bytes.as_ref(),
        expected,
        "one archived PACK frame, no neighbor"
    );

    // The browser must exercise the real D-checked Rust shell and discover
    // the PACK only via a denied SINGLE grant (404), then a single same-origin
    // /pack POST. It never downloads a complete PACK or sees object keys.
    live_pack_chromium::verify(
        &app,
        frame.chapter_id,
        frame.version_id,
        &buyer_session,
        &unrelated_session,
        std::str::from_utf8(expected).unwrap(),
    )
    .await;

    pg(
        url,
        &format!(
            "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
             VALUES({},'legal_block','ci:d05-pack-revoke:{}')",
            term.term_id, term.term_id
        ),
    );
    assert_eq!(
        post(
            &app,
            &path,
            Some(&buyer_session),
            Some(&csrf),
            Some("http://127.0.0.1:3098"),
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "revoked paid Layer term cannot read any archived PACK frame"
    );
    super::retained_layer_pack::verify(url, client, book, receipt).await;
    pool.close().await;
}
