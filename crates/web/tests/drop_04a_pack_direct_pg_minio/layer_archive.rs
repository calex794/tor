//! D-05: real active PACK + paid historical Layer + actual owner deletion.
//! This fixture builds on the genuine DROP-04b MinIO pack finalizer and
//! STOPs before touching any production source or migration.
use super::{pg, BUCKET};
use aws_sdk_s3::{
    config::{
        interceptors::FinalizerInterceptorContextRef, ConfigBag, Intercept, RuntimeComponents,
    },
    Client,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    num::NonZeroU32,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
};
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

#[path = "layer_archive/live_pack_chromium.rs"]
mod live_pack_chromium;

const RANGE_IDLE: u8 = 0;
const RANGE_ARMED: u8 = 1;
const RANGE_OBSERVED: u8 = 2;

/// Revoke only when this fixture's genuine MinIO Range has returned headers
/// successfully, before the streaming SDK result reaches the gateway.
#[derive(Debug)]
struct RevokeArchivedTermOnRange {
    state: Arc<AtomicU8>,
    term_id: String,
}

impl Intercept for RevokeArchivedTermOnRange {
    fn name(&self) -> &'static str {
        "D001RevokeArchivedTermOnRange"
    }

    fn read_after_execution(
        &self,
        context: &FinalizerInterceptorContextRef<'_>,
        _: &RuntimeComponents,
        _: &mut ConfigBag,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        if context.response().is_some_and(|r| r.status().as_u16() == 206)
            && matches!(context.output_or_error(), Some(Ok(_)))
            && self
                .state
                .compare_exchange(
                    RANGE_ARMED,
                    RANGE_OBSERVED,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
        {
            let url = std::env::var("TEST_DATABASE_URL").expect("disposable fixture database");
            assert_eq!(
                pg(
                    &url,
                    &format!(
                        "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
                         VALUES({0},'legal_block','ci:d05-pack-revoke:{0}') RETURNING term_id",
                        self.term_id
                    ),
                ),
                self.term_id
            );
        }
        Ok(())
    }
}

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

    // Earlier transport and author-edit checks published SINGLE V2 for both
    // chapters. Restore both verified PACK V1 publications in this disposable
    // fixture before the owner takes the archive snapshot.
    assert_eq!(receipt.frames.len(), 2);
    for restored in &receipt.frames {
        // Use the existing disposable PostgreSQL fixture helper; tor-web
        // intentionally has no direct sqlx dependency.
        pg(
            url,
            &format!(
                "UPDATE chapter_publications SET version_id={} WHERE chapter_id={}",
                restored.version_id, restored.chapter_id
            ),
        );
        assert_eq!(
            pg(
                url,
                &format!(
                    "SELECT version_id FROM chapter_publications WHERE chapter_id={}",
                    restored.chapter_id
                ),
            ),
            restored.version_id.to_string(),
            "each FREE chapter must publish its original immutable PACK frame"
        );
    }
    let frame = &receipt.frames[1];
    let expected = chapters[1].body.as_bytes();
    assert_eq!(
        <[u8; 32]>::from(Sha256::digest(expected)),
        frame.content_sha256
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

    // The same verified two-frame PACK must keep BOTH pre-deletion FREE
    // publications, with independent version/hash identities.
    let first = &receipt.frames[0];
    let first_expected = chapters[0].body.as_bytes();
    assert_eq!(
        <[u8; 32]>::from(Sha256::digest(first_expected)),
        first.content_sha256
    );
    let first_pack = tor_reading::repositories::free_s3_ref::PgFreeS3Ref::new(pool.clone())
        .archived_free_active_pack_metadata_only(first.chapter_id, first.version_id)
        .await
        .unwrap()
        .expect("the first FREE V1 is an independently verified active PACK frame");
    assert_eq!(first_pack.sha256, first.content_sha256);
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM free_chapter_storage_objects
                 WHERE chapter_id={} AND version_id={}",
                first.chapter_id, first.version_id
            ),
        ),
        "0",
        "the first FREE V1 must also be PACK-only, without a SINGLE receipt"
    );

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

    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT version_id FROM book_layer_free_archive_versions
                 WHERE book_id={book} AND chapter_id={}",
                first.chapter_id
            ),
        ),
        first.version_id.to_string(),
        "the approved archive must retain both FREE V1 publications"
    );

    // Actor-scoped HTTP route, exact FRAME only; all transport keys and
    // Range offsets stay inside the service.
    let range_state = Arc::new(AtomicU8::new(RANGE_IDLE));
    let pack_client = Client::from_conf(
        client
            .config()
            .to_builder()
            .interceptor(RevokeArchivedTermOnRange {
                state: Arc::clone(&range_state),
                term_id: term.term_id.to_string(),
            })
            .build(),
    );
    std::env::set_var("TOR_LAYER_FREE_ARCHIVE_ENABLED", "1");
    let pack_app = tor_web::free_pack_http::router(
        accounts.clone(),
        false,
        "http://127.0.0.1:3098".into(),
        pack_client,
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

    let first_path = format!("/api/v1/reader/layer/free/{}/pack", first.chapter_id);
    let first_response = post(
        &app,
        &first_path,
        Some(&buyer_session),
        Some(&csrf),
        Some("http://127.0.0.1:3098"),
        false,
    )
    .await;
    assert_eq!(first_response.status(), StatusCode::OK);
    assert_eq!(
        first_response.headers()["x-tor-free-version-id"],
        first.version_id.to_string()
    );
    assert_eq!(
        first_response.headers()["x-tor-free-sha256"],
        hex::encode(first.content_sha256)
    );
    assert_eq!(first_response.headers()["cache-control"], "no-store");
    assert!(!first_response.headers().contains_key(header::LOCATION));
    let first_bytes = to_bytes(first_response.into_body(), 200_001).await.unwrap();
    assert_eq!(
        first_bytes.as_ref(),
        first_expected,
        "the first FREE frame cannot expose the neighboring FREE frame"
    );
    assert_ne!(
        first_bytes.as_ref(),
        expected,
        "distinct FREE frame contents"
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

    // Start with a live purchase. Only the successful MinIO response for this
    // request triggers the PG revocation, not any preceding request or timer.
    range_state.store(RANGE_ARMED, Ordering::SeqCst);
    let interrupted = post(
        &app,
        &path,
        Some(&buyer_session),
        Some(&csrf),
        Some("http://127.0.0.1:3098"),
        false,
    )
    .await;
    assert_eq!(range_state.load(Ordering::SeqCst), RANGE_OBSERVED);
    assert_eq!(interrupted.status(), StatusCode::NOT_FOUND);
    assert!(!interrupted.headers().contains_key("x-tor-free-version-id"));
    let denied = to_bytes(interrupted.into_body(), 200_001).await.unwrap();
    assert!(
        denied.is_empty(),
        "an in-flight revoked Layer request must not release plaintext"
    );
    println!("D001_ARCHIVED_PACK_RANGE_REVOKED_NO_PLAINTEXT_OK");

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
    assert_eq!(
        post(
            &app,
            &first_path,
            Some(&buyer_session),
            Some(&csrf),
            Some("http://127.0.0.1:3098"),
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "revocation must also close the other chapter in the same PACK"
    );
    super::retained_layer_pack::verify(url, client, book, receipt).await;
    pool.close().await;
}
