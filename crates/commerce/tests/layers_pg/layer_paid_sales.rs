//! H/UI-19: live PostgreSQL regression, disposable tor_ci only.
//! Package-only is a sales policy, NOT a revocation of paid chapter history.
use super::*;
use tor_commerce::{
    domain::money::{CheckoutError, CheckoutRepository, MoneyMinor, PurchaseCommand, WalletId},
    repositories::{checkout::PgCheckout, layer_package_sales::PackagePaidMode},
};
use tor_contracts::paid_delivery::ChapterVersionRef;

async fn paid(pool: &PgPool, book: i64, chapter_no: i32) -> (i64, i64) {
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,$2,'H19 paid','public','paid',75) RETURNING id",
    )
    .bind(book)
    .bind(chapter_no)
    .fetch_one(pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'H19 private') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
    (chapter, version)
}

fn direct_command(buyer: i64, chapter: i64, version: i64, key: char) -> PurchaseCommand {
    PurchaseCommand::new(
        id(buyer),
        WalletId::new(buyer).unwrap(),
        ChapterVersionRef {
            chapter_id: id(chapter),
            version_id: id(version),
        },
        MoneyMinor::new(75).unwrap(),
        tor_commerce::domain::money::IdempotencyKey::parse(&format!(
            "{}8",
            key.to_string().repeat(42)
        ))
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn h_ui_19_paid_sales_fail_closed_without_revoking_purchases() {
    let (pool, layer_repo, owner, buyer, book) = setup("h19-locked-offer").await;
    let (layer, version, offer_id) = offer(&layer_repo, owner, book, 250, true).await;
    let (old_chapter, old_version) = paid(&pool, book, 1).await;
    let (new_chapter, new_version) = paid(&pool, book, 2).await;
    let checkout = PgCheckout::new(pool.clone());
    let original = direct_command(buyer, old_chapter, old_version, 'Y');
    let receipt = checkout.checkout_once(&original).await.unwrap();
    let before: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    let page = layer_repo
        .package_sale_page(id(owner), id(layer))
        .await
        .unwrap();
    let revision = page.books[0].revision;
    assert!(!page.books[0].restricted_here);
    let updated = layer_repo
        .set_package_paid_mode(
            id(owner),
            id(layer),
            id(book),
            id(version),
            revision,
            PackagePaidMode::PackageOnly,
        )
        .await
        .unwrap();
    assert!(updated > revision);
    let exclusive_page = layer_repo
        .package_sale_page(id(owner), id(layer))
        .await
        .unwrap();
    assert!(exclusive_page.books[0].restricted_here);
    assert_eq!(exclusive_page.books[0].until_utc, None);
    assert!(checkout
        .selectable_batch_offers(buyer, book)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        checkout
            .checkout_once(&direct_command(buyer, new_chapter, new_version, 'Z'))
            .await,
        Err(CheckoutError::Inaccessible),
    );
    // Retries of already-successful commands reproduce the historical receipt.
    let replay = checkout.checkout_once(&original).await.unwrap();
    assert_eq!(replay.purchase_id, receipt.purchase_id);
    assert!(replay.replayed);
    let has_rights: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM library_purchased_chapters
         WHERE user_id=$1 AND chapter_id=$2)",
    )
    .bind(buyer)
    .bind(old_chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(has_rights);
    assert!(
        layer_repo
            .buy(&cmd(buyer, layer, offer_id, version, 250, 'H'))
            .await
            .is_ok(),
        "the designated package still sells its own book"
    );
    assert!(layer_repo.can_read_book(id(buyer), id(book)).await.unwrap());
    let after: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after,
        before - 250,
        "only the package purchase debited the wallet"
    );
    assert_eq!(
        layer_repo
            .set_package_paid_mode(
                id(owner),
                id(layer),
                id(book),
                id(version),
                revision,
                PackagePaidMode::Direct,
            )
            .await,
        Err(LayerError::StaleOffer),
        "an obsolete browser tab may not remove the restriction",
    );
    layer_repo
        .set_package_paid_mode(
            id(owner),
            id(layer),
            id(book),
            id(version),
            updated,
            PackagePaidMode::Direct,
        )
        .await
        .unwrap();
    assert!(
        checkout
            .checkout_once(&direct_command(buyer, new_chapter, new_version, 'X'))
            .await
            .is_ok(),
        "direct purchase is restored only after owner lifts restriction"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn h_ui_19_exclusivity_cannot_strand_books_or_cross_packages() {
    let (pool, repo, owner, buyer, book) = setup("h19-hidden").await;
    let (layer, version, _offer_id) = offer(&repo, owner, book, 200, false).await;
    let other = repo.create_layer(id(owner), "Other package").await.unwrap();
    let version_other = repo.add_book(id(owner), id(other), id(book)).await.unwrap();
    assert_eq!(
        repo.set_package_paid_mode(
            id(owner),
            id(layer),
            id(book),
            id(version),
            0,
            PackagePaidMode::PackageOnly,
        )
        .await,
        Err(LayerError::Invalid),
        "a book present in another package is not uniquely exclusive",
    );
    // Removing the conflicting book from the other package permits exclusivity.
    repo.remove_book(id(owner), id(other), id(book))
        .await
        .unwrap();
    assert_eq!(
        repo.set_package_paid_mode(
            id(buyer),
            id(layer),
            id(book),
            id(version),
            0,
            PackagePaidMode::PackageOnly,
        )
        .await,
        Err(LayerError::Inaccessible),
    );
    let new_revision = repo
        .set_package_paid_mode(
            id(owner),
            id(layer),
            id(book),
            id(version),
            0,
            PackagePaidMode::First7Days,
        )
        .await
        .unwrap();
    assert!(new_revision > 0);
    assert_eq!(
        repo.add_book(id(owner), id(other), id(book)).await,
        Err(LayerError::Inaccessible),
        "a second package cannot add a Book restricted to the first",
    );
    assert_eq!(
        repo.set_package_paid_mode(
            id(owner),
            id(layer),
            id(book),
            id(version_other),
            new_revision,
            PackagePaidMode::PackageOnly,
        )
        .await,
        Err(LayerError::StaleOffer),
    );
    let (_, _) = paid(&pool, book, 3).await;
    let timed = repo.package_sale_page(id(owner), id(layer)).await.unwrap();
    assert!(timed.books[0].restricted_here);
    assert!(timed.books[0].until_utc.is_some());
    // Simulate expiry without waiting a week: direct sales become available.
    sqlx::query("UPDATE books SET package_only_until=now()-interval '1 second' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        !repo
            .package_sale_page(id(owner), id(layer))
            .await
            .unwrap()
            .books[0]
            .restricted_here
    );
    repo.set_package_paid_mode(
        id(owner),
        id(layer),
        id(book),
        id(version),
        new_revision,
        PackagePaidMode::PackageOnly,
    )
    .await
    .unwrap();
    // A normal composition edit removes the book and automatically clears the
    // restriction. Historical versions and previous purchases are untouched.
    repo.remove_book(id(owner), id(layer), id(book))
        .await
        .unwrap();
    let current: Option<i64> =
        sqlx::query_scalar("SELECT package_only_layer_id FROM books WHERE id=$1")
            .bind(book)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current, None);
    assert!(repo.can_read_book(id(buyer), id(book)).await.unwrap() == false);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn h_ui_19_team_member_change_releases_sole_owner_sale_restriction() {
    let (pool, repo, owner, collaborator, book) = setup("h19-team").await;
    let (layer, version, _) = offer(&repo, owner, book, 250, true).await;
    let applied = repo
        .set_package_paid_mode(
            id(owner),
            id(layer),
            id(book),
            id(version),
            0,
            PackagePaidMode::PackageOnly,
        )
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_members(book_id,user_id,role,is_active)
         VALUES($1,$2,'translator',TRUE)",
    )
    .bind(book)
    .bind(collaborator)
    .execute(&pool)
    .await
    .unwrap();
    let (current, revision): (Option<i64>, i64) =
        sqlx::query_as("SELECT package_only_layer_id,package_only_revision FROM books WHERE id=$1")
            .bind(book)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current, None);
    assert!(revision > applied);
    assert_eq!(
        repo.set_package_paid_mode(
            id(owner),
            id(layer),
            id(book),
            id(version),
            revision,
            PackagePaidMode::PackageOnly,
        )
        .await,
        Err(LayerError::Inaccessible),
        "sole-owner restrictions must not be restored while a translator is active",
    );
    pool.close().await;
}
