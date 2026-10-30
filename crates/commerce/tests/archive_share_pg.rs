//! ARCHIVE-01: isolated 3-generation sales through retired-book buyer invitations.
use sqlx::PgPool;
use tor_commerce::{
    domain::money::{CheckoutError, CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId},
    repositories::checkout::PgCheckout,
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

fn command(actor:i64, chapter:i64, version:i64, price:i64, letter:char)->PurchaseCommand {
    PurchaseCommand::new(
        PositiveId::new(actor).unwrap(),
        WalletId::new(actor).unwrap(),
        ChapterVersionRef{chapter_id:PositiveId::new(chapter).unwrap(),version_id:PositiveId::new(version).unwrap()},
        MoneyMinor::new(price).unwrap(),
        IdempotencyKey::parse(&format!("{}8",letter.to_string().repeat(42))).unwrap(),
    ).unwrap()
}

async fn buyer(pool:&PgPool, name:&str)->i64 {
    let id:i64=sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Archive buyer','$argon2id$test') RETURNING id",
    ).bind(format!("archive_{name}@example.test")).fetch_one(pool).await.unwrap();
    sqlx::query("INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) VALUES($1,1000,'RUB')")
        .bind(id).execute(pool).await.unwrap();
    id
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_archive_chain_a_b_c_d_independent_purchases() {
    let url=std::env::var("TEST_DATABASE_URL").expect("test PG");
    assert!(url.ends_with("/tor_ci"),"never operate against production DB");
    let pool=PgPool::connect(&url).await.unwrap();
    let owner=buyer(&pool,"owner").await;
    let a=buyer(&pool,"a").await;
    let b=buyer(&pool,"b").await;
    let c=buyer(&pool,"c").await;
    let d=buyer(&pool,"d").await;
    let book:i64=sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:archive-chain','ru','Retired book','public',$1) RETURNING id",
    ).bind(owner).fetch_one(&pool).await.unwrap();
    let chapter:i64=sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Archive V1','public','paid',100) RETURNING id",
    ).bind(book).fetch_one(&pool).await.unwrap();
    let version:i64=sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'History') RETURNING id",
    ).bind(chapter).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter).bind(version).execute(&pool).await.unwrap();
    let store=PgCheckout::new(pool.clone());
    let a_receipt=store.checkout_once(&command(a,chapter,version,100,'A')).await.unwrap();
    sqlx::query(
        "UPDATE books SET visibility='draft',archived_at=now(),archive_invite_sales_enabled=TRUE
         WHERE id=$1",
    ).bind(book).execute(&pool).await.unwrap();
    assert!(matches!(
        store.checkout_once(&command(b,chapter,version,100,'E')).await,
        Err(CheckoutError::Inaccessible)
    ));
    let a_offer=store.create_archive_offer(a,a_receipt.purchase_id.get(),None).await.unwrap();
    let b_cmd=command(b,chapter,version,100,'B');
    let b_receipt=store.checkout_archive_once(&a_offer,&b_cmd).await.unwrap();
    assert_eq!(store.checkout_archive_once(&a_offer,&b_cmd).await.unwrap().purchase_id,b_receipt.purchase_id);
    assert!(matches!(store.checkout_archive_once(&a_offer,&command(b,chapter,version,100,'F')).await,
        Err(CheckoutError::AlreadyOwned)));
    let b_offer=store.create_archive_offer(b,b_receipt.purchase_id.get(),None).await.unwrap();
    let c_receipt=store.checkout_archive_once(&b_offer,&command(c,chapter,version,100,'C')).await.unwrap();
    let c_offer=store.create_archive_offer(c,c_receipt.purchase_id.get(),None).await.unwrap();
    let d_receipt=store.checkout_archive_once(&c_offer,&command(d,chapter,version,100,'D')).await.unwrap();
    assert_ne!(a_receipt.purchase_id,b_receipt.purchase_id);
    assert_ne!(b_receipt.purchase_id,c_receipt.purchase_id);
    assert_ne!(c_receipt.purchase_id,d_receipt.purchase_id);
    let count:i64=sqlx::query_scalar(
        "SELECT COUNT(*) FROM chapter_purchase_facts WHERE chapter_id=$1",
    ).bind(chapter).fetch_one(&pool).await.unwrap();
    assert_eq!(count,4);
    let origins:i64=sqlx::query_scalar(
        "SELECT COUNT(*) FROM archive_purchase_sources s
         JOIN chapter_purchase_facts p ON p.id=s.purchase_id WHERE p.chapter_id=$1",
    ).bind(chapter).fetch_one(&pool).await.unwrap();
    assert_eq!(origins,3);
    // Parent's invitation can be revoked without changing a child's purchase.
    sqlx::query("UPDATE archive_share_offers SET revoked_at=now() WHERE token=$1::uuid")
        .bind(&a_offer).execute(&pool).await.unwrap();
    assert!(store.create_archive_offer(d,d_receipt.purchase_id.get(),None).await.is_ok());
    // Global disable affects NEW offers and checkouts, not the four earlier purchases.
    sqlx::query("UPDATE books SET archive_invite_sales_enabled=FALSE WHERE id=$1")
        .bind(book).execute(&pool).await.unwrap();
    assert!(matches!(store.checkout_archive_once(&c_offer,&command(owner,chapter,version,100,'G')).await,
        Err(CheckoutError::Inaccessible)));
    assert!(matches!(store.create_archive_offer(b,b_receipt.purchase_id.get(),None).await,
        Err(CheckoutError::Inaccessible)));
    let pinned:i64=sqlx::query_scalar(
        "SELECT COUNT(*) FROM library_purchased_chapters WHERE chapter_id=$1",
    ).bind(chapter).fetch_one(&pool).await.unwrap();
    assert_eq!(pinned,4);
}
