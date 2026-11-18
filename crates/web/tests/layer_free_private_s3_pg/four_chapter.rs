//! D-05/#1022: one genuine deleted Book with 2 FREE SINGLE + 2 PAID S3
//! chapters. Only compiled in the explicit combined s3-free+s3-paid gate.
use super::*;
use axum::{body::Body, http::StatusCode};
use std::{fs::{self, OpenOptions}, io::Write, path::Path, process::Command, sync::Arc};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use tor_books::repositories::paid_storage::{PaidPublication, PgPaidStorage};
use tor_contracts::paid_delivery::{
    ChapterVersionRef, ObjectId, PrivateObjectDeliveryPort,
};
use tor_reading::repositories::PgReader;
use tor_storage::{
    repositories::{
        paid_delivery::PgPrivateDelivery, registry::PgStorageRegistry, s3::S3PrivateStore,
    },
    services::{
        crypto::{seal_chapter, ContentKek},
        provider::upload_and_verify as upload_paid,
    },
};

const FREE_TWO: &str = "D05 FREE 2 frozen 中文 <script>window.__d05Xss=1</script>";
const PAID_THREE: &str = "D05 historical PAID 3 encrypted МинИО";
const PAID_FOUR: &str = "D05 historical PAID 4 encrypted 中文";

pub(super) struct Prepared {
    pub survivor: i64,
    pub editions: [(i64, i64); 4],
    paid: Arc<dyn PrivateObjectDeliveryPort>,
}

pub(super) async fn prepare(
    url: &str,
    client: &Client,
    owner: i64,
    book: i64,
    first: (i64, i64),
) -> Prepared {
    let catalog = PgCatalog::connect(url).await.unwrap();
    let pool = catalog.pool().clone();
    let second: i64 = pg(
        url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},2,'D05 archived FREE 2','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let hash: [u8; 32] = Sha256::digest(FREE_TWO.as_bytes()).into();
    let v2: i64 = pg(
        url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,content_sha256)
             VALUES({second},1,decode('{}','hex')) RETURNING id",
            hex::encode(hash)
        ),
    )
    .parse()
    .unwrap();
    let free_object = FreeChapterObject::for_markdown(second, v2, FREE_TWO).unwrap();
    let free_store = S3FreeChapterStore::new(client.clone(), BUCKET.to_owned()).unwrap();
    upload_and_verify(&free_store, &free_object, FREE_TWO)
        .await
        .unwrap();
    pg(
        url,
        &format!(
            "INSERT INTO free_chapter_storage_objects
             (chapter_id,version_id,bucket,object_key,sha256,size_bytes)
             VALUES({second},{v2},'{BUCKET}','{}',decode('{}','hex'),{})",
            free_object.key(),
            hex::encode(free_object.sha256()),
            free_object.size_bytes(),
        ),
    );
    pg(
        url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({second},{v2})"
        ),
    );
    let kek = Arc::new(ContentKek::from_bytes([0x55; 32]));
    let private = S3PrivateStore::new(client.clone(), BUCKET.to_owned()).unwrap();
    let registry = PgStorageRegistry::new(pool.clone());
    let mut paid_editions = Vec::new();
    for (chapter_no, text, object) in [
        (3, PAID_THREE, "f0c0e034-2026-4034-8034-000000000063"),
        (4, PAID_FOUR, "f0c0e034-2026-4034-8034-000000000064"),
    ] {
        let chapter: i64 = pg(
            url,
            &format!(
                "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
                 VALUES({book},{chapter_no},'D05 archived PAID {chapter_no}',
                        'public','paid',50) RETURNING id"
            ),
        )
        .parse()
        .unwrap();
        let version: i64 = pg(
            url,
            &format!(
                "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
                 VALUES({chapter},1,'{}') RETURNING id",
                text.replace('\'', "''")
            ),
        )
        .parse()
        .unwrap();
        let pinned = ChapterVersionRef {
            chapter_id: id(chapter),
            version_id: id(version),
        };
        let object_id = ObjectId::parse(object).unwrap();
        let sealed = seal_chapter(pinned, object_id.clone(), text, &kek).unwrap();
        let pending = registry.reserve_pending(&sealed, &kek, BUCKET).await.unwrap();
        let receipt = upload_paid(&private, &pending, &sealed.ciphertext)
            .await
            .unwrap();
        registry.mark_ready(&receipt).await.unwrap();
        PgPaidStorage::new(pool.clone())
            .attach_verified_and_publish(
                owner, book, pinned, &object_id,
                PaidPublication::Publish { expected_current: None },
            )
            .await
            .unwrap();
        paid_editions.push((chapter, version));
    }
    let survivor: i64 = pg(
        url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:d05-four-survivor-{book}','ru','Surviving Layer Book',
                    'public',{owner},'basic') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let survivor_chapter: i64 = pg(
        url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({survivor},1,'Remaining Layer PAID','public','paid',50)
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let survivor_version: i64 = pg(
        url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({survivor_chapter},1,'Surviving chapter') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({survivor_chapter},{survivor_version})"
        ),
    );
    let signer = PgPrivateDelivery::new(pool, private, kek, BUCKET.to_owned()).unwrap();
    Prepared {
        survivor,
        editions: [first, (second, v2), paid_editions[0], paid_editions[1]],
        paid: Arc::new(signer),
    }
}

pub(super) async fn lifetime_buyer(
    accounts: &PgAccounts,
    owner: i64,
    book: i64,
    survivor: i64,
) -> (i64, String) {
    let (buyer, session, _) = user(accounts, "lifetime").await;
    pg(
        &env::var("TEST_DATABASE_URL").unwrap(),
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES({buyer},300,'RUB')"
        ),
    );
    let layers = PgLayers::new(accounts.pool().clone());
    let layer = layers.create_layer(id(owner), "D05 lifetime archive").await.unwrap();
    layers.add_book(id(owner), id(layer), id(book)).await.unwrap();
    let version = layers.add_book(id(owner), id(layer), id(survivor)).await.unwrap();
    let offer = layers.publish_offer(id(owner), id(layer), 90, true).await.unwrap();
    layers
        .buy(&LayerCheckoutCommand {
            actor: id(buyer), wallet: WalletId::new(buyer).unwrap(),
            layer: id(layer), offer: id(offer), version: id(version),
            expected_price: MoneyMinor::new(90).unwrap(),
            key: IdempotencyKey::parse(&format!("{}8", "R".repeat(42))).unwrap(),
        })
        .await
        .unwrap();
    (buyer, session)
}

pub(super) async fn verify(
    free_app: Router,
    url: &str,
    accounts: &PgAccounts,
    prepared: Prepared,
    old: (i64, &str, i64, i64, i64),
    lifetime: (i64, &str),
    book: i64,
    owner: i64,
) {
    let (old_user, old_session, old_term, layer, offer) = old;
    let layers = PgLayers::new(accounts.pool().clone());
    let clean = layers.remove_book(id(owner), id(layer), id(book)).await.unwrap();
    let renewed = layers
        .buy(&LayerCheckoutCommand {
            actor: id(old_user), wallet: WalletId::new(old_user).unwrap(),
            layer: id(layer), offer: id(offer), version: id(clean),
            expected_price: MoneyMinor::new(90).unwrap(),
            key: IdempotencyKey::parse(&format!("{}8", "S".repeat(42))).unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT next.started_at=prior.expires_at AND
                        next.access_origin_version_id=prior.access_origin_version_id
                 FROM commerce_layer_terms prior
                 JOIN commerce_layer_terms next ON next.id={}
                 WHERE prior.id={old_term}",
                renewed.term_id
            ),
        ),
        "t",
        "renewal cannot drop original deleted Book or steal prepaid days"
    );
    let (late, late_session, _) = user(accounts, "late").await;
    pg(
        url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES({late},300,'RUB')"
        ),
    );
    layers
        .buy(&LayerCheckoutCommand {
            actor: id(late), wallet: WalletId::new(late).unwrap(),
            layer: id(layer), offer: id(offer), version: id(clean),
            expected_price: MoneyMinor::new(90).unwrap(),
            key: IdempotencyKey::parse(&format!("{}8", "T".repeat(42))).unwrap(),
        })
        .await
        .unwrap();
    assert!(layers.can_read_book(id(late), id(prepared.survivor)).await.unwrap());
    assert!(!layers.can_read_book(id(late), id(book)).await.unwrap());
    assert!(layers.can_read_book(id(old_user), id(book)).await.unwrap());
    assert!(layers.can_read_book(id(lifetime.0), id(book)).await.unwrap());

    std::env::set_var("TOR_LAYER_READER_ENABLED", "1");
    std::env::set_var("TOR_PAID_CHECKOUT_ENABLED", "1");
    let paid_catalog = PgCatalog::connect(url).await.unwrap();
    let paid_router = tor_web::router_with_postgres_reading_auth_paid_grants_browser(
        paid_catalog, PgReader::new(accounts.pool().clone()),
        accounts.clone(), false, ORIGIN.to_owned(),
        (prepared.paid, ENDPOINT.to_owned()), None,
    )
    .unwrap();
    std::env::remove_var("TOR_LAYER_READER_ENABLED");
    std::env::remove_var("TOR_PAID_CHECKOUT_ENABLED");
    let app = free_app.merge(paid_router);

    for chapter in prepared.editions.iter().take(2).map(|(c, _)| *c) {
        assert_eq!(
            call(&app, Method::GET, &format!("/read/layer/free/{chapter}"),
                 Some(old_session), None, None).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            call(&app, Method::GET, &format!("/read/layer/free/{chapter}"),
                 Some(&late_session), None, None).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    for chapter in prepared.editions.iter().skip(2).map(|(c, _)| *c) {
        assert_eq!(
            call(&app, Method::GET, &format!("/read/layer/{chapter}"),
                 Some(old_session), None, None).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            call(&app, Method::GET, &format!("/read/layer/{chapter}"),
                 Some(&late_session), None, None).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    browser(&app, &prepared.editions, old_session, lifetime.1, &late_session).await;
}

async fn browser(
    app: &Router,
    editions: &[(i64, i64); 4],
    old: &str,
    lifetime: &str,
    late: &str,
) {
    let temp = env::var("RUNNER_TEMP").expect("disposable CI temp required");
    let fixture_path = Path::new(&temp).join("d05-four-chapter-live-chromium.json");
    let payload = serde_json::json!({
        "old": old, "lifetime": lifetime, "late": late,
        "free": [
            {"id": editions[0].0, "version": editions[0].1, "text": TEXT},
            {"id": editions[1].0, "version": editions[1].1, "text": FREE_TWO},
        ],
        "paid": [
            {"id": editions[2].0, "version": editions[2].1, "text": PAID_THREE},
            {"id": editions[3].0, "version": editions[3].1, "text": PAID_FOUR},
        ]
    });
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&fixture_path).unwrap();
    file.write_all(payload.to_string().as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3098")
        .await
        .expect("reserved loopback CI port");
    let app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("real D05 four-chapter Rust router");
    });
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/d05_four_chapter_live_chromium.cjs");
    let output = tokio::task::spawn_blocking(move || {
        Command::new("timeout")
            .args(["-k", "5s", "210s", "node"])
            .arg(script)
            .env("TOR_D05_FOUR_CHAPTER_FIXTURE", fixture_path)
            .output()
    })
    .await
    .unwrap();
    server.abort();
    let _ = server.await;
    fs::remove_file(Path::new(&temp).join("d05-four-chapter-live-chromium.json")).unwrap();
    let output = output.expect("Node/Playwright required");
    assert!(
        output.status.success(),
        "D05 four-chapter real Chromium failed (exit {:?}); secrets withheld",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("D05 FOUR Chromium PASS: desktop")
            && stdout.contains("D05 FOUR Chromium PASS: mobile")
            && stdout.contains("D05 FOUR Chromium PASS: lifetime")
            && stdout.contains("D05 FOUR Chromium PASS: denied"),
        "old, lifetime and late actors must complete all Chromium gates"
    );
}
