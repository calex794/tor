//! D-07: real PostgreSQL 18 and same finance transaction as D-02.
//! Only run with TEST_DATABASE_URL ending in disposable tor_ci.
use sqlx::{PgPool, Row};
use tor_commerce::{
    domain::money::{
        CheckoutError, CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand,
        PurchaseDiscountOffer, WalletId,
    },
    repositories::{
        checkout::{PgCheckout, SingleChapterQuote},
        discount_campaigns::{DiscountCampaignError, PgAuthorDiscounts},
    },
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

struct Fixture {
    pool: PgPool,
    reader: i64,
    owner: i64,
    book: i64,
    chapter: i64,
    version: i64,
}
async fn fixture(name: &str, balance: i64, base: i64) -> Fixture {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated DB");
    assert!(url.ends_with("/tor_ci"), "never touch a real wallet");
    let pool = PgPool::connect(&url).await.unwrap();
    let reader: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Discount reader','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("d07_{name}_reader@ci.example"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Discount owner','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("d07_{name}_owner@ci.example"))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,$2,'RUB')",
    )
    .bind(reader)
    .bind(balance)
    .execute(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','Книга со скидкой','public',$2) RETURNING id",
    )
    .bind(format!("ci:d07:{name}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Платная глава','public','paid',$2) RETURNING id",
    )
    .bind(book)
    .bind(base)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Вымышленный контент') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();
    Fixture {
        pool,
        reader,
        owner,
        book,
        chapter,
        version,
    }
}
fn command(f: &Fixture, price: i64, key: char) -> PurchaseCommand {
    PurchaseCommand::new(
        PositiveId::new(f.reader).unwrap(),
        WalletId::new(f.reader).unwrap(),
        ChapterVersionRef {
            chapter_id: PositiveId::new(f.chapter).unwrap(),
            version_id: PositiveId::new(f.version).unwrap(),
        },
        MoneyMinor::new(price).unwrap(),
        IdempotencyKey::parse(&format!("{}8", key.to_string().repeat(42))).unwrap(),
    )
    .unwrap()
}
fn command_for_quote(f: &Fixture, quote: &SingleChapterQuote, key: char) -> PurchaseCommand {
    PurchaseCommand::new(
        PositiveId::new(f.reader).unwrap(),
        WalletId::new(f.reader).unwrap(),
        ChapterVersionRef {
            chapter_id: PositiveId::new(f.chapter).unwrap(),
            version_id: PositiveId::new(quote.version_id).unwrap(),
        },
        MoneyMinor::new(quote.final_price_minor).unwrap(),
        IdempotencyKey::parse(&format!("{}8", key.to_string().repeat(42))).unwrap(),
    )
    .unwrap()
    .with_discount_offer(
        MoneyMinor::new(quote.base_price_minor).unwrap(),
        PurchaseDiscountOffer::new(
            quote.campaign_id.unwrap(),
            quote.campaign_revision.unwrap(),
            quote.rule_version.unwrap(),
            quote.follow_period_id.unwrap(),
            quote.discount_percent.unwrap(),
            quote.requires_follow_days.unwrap(),
        )
        .unwrap(),
    )
    .unwrap()
}
async fn follow(f: &Fixture, started_days_ago: i32) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO creator_follow_periods(
           follower_user_id,creator_user_id,source_book_id,started_at)
         VALUES($1,$2,$3,now()-make_interval(days=>$4))
         RETURNING id",
    )
    .bind(f.reader)
    .bind(f.owner)
    .bind(f.book)
    .bind(started_days_ago)
    .fetch_one(&f.pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_07_exact_ninety_day_discount_freezes_paid_amount_and_replay() {
    let f = fixture("ninety", 300, 100).await;
    let campaigns = PgAuthorDiscounts::new(f.pool.clone());
    let checkout = PgCheckout::new(f.pool.clone());
    assert_eq!(
        campaigns.create(f.reader, f.book, 20, 90, 30).await,
        Err(DiscountCampaignError::NotFound),
        "buyer must never create an author's promotion"
    );
    let campaign_id = campaigns.create(f.owner, f.book, 20, 90, 30).await.unwrap();
    assert_eq!(
        campaigns.create(f.owner, f.book, 20, 90, 30).await,
        Err(DiscountCampaignError::Conflict),
        "overlapping campaigns are forbidden"
    );
    assert_eq!(
        campaigns.create(f.owner, f.book, 100, 90, 30).await,
        Err(DiscountCampaignError::Invalid)
    );
    let ordinary = checkout
        .quote_single(f.reader, f.book, f.chapter)
        .await
        .unwrap();
    assert_eq!(ordinary.final_price_minor, 100);
    assert_eq!(ordinary.discount_percent, None);
    assert_eq!(
        checkout.checkout_once(&command(&f, 80, 'A')).await,
        Err(CheckoutError::StaleOffer),
        "falsely claimed discount must not reach the wallet"
    );
    let period = follow(&f, 89).await;
    let not_yet_eligible = checkout
        .quote_single(f.reader, f.book, f.chapter)
        .await
        .unwrap();
    assert_eq!(not_yet_eligible.final_price_minor, 100);
    assert_eq!(not_yet_eligible.discount_percent, None);
    sqlx::query(
        "UPDATE creator_follow_periods
         SET started_at=now()-interval '90 days' WHERE id=$1",
    )
    .bind(period)
    .execute(&f.pool)
    .await
    .unwrap();
    let offer = checkout
        .quote_single(f.reader, f.book, f.chapter)
        .await
        .unwrap();
    assert_eq!((offer.base_price_minor, offer.final_price_minor), (100, 80));
    assert_eq!(offer.discount_percent, Some(20));
    assert_eq!(offer.campaign_id, Some(campaign_id));
    assert_eq!(offer.campaign_revision, Some(1));
    assert_eq!(offer.follow_period_id, Some(period));
    assert_eq!(offer.requires_follow_days, Some(90));
    assert_eq!(
        checkout.checkout_once(&command(&f, 100, 'B')).await,
        Err(CheckoutError::StaleOffer),
        "a mature follower must not silently overpay the full price"
    );
    let buy = command_for_quote(&f, &offer, 'C');
    let receipt = checkout.checkout_once(&buy).await.unwrap();
    assert_eq!(receipt.amount.get(), 80);
    assert_eq!(receipt.balance_after.get(), 220);
    assert_eq!(receipt.pinned.version_id.get(), f.version);
    let snapshot = sqlx::query(
        "SELECT campaign_id,campaign_revision,rule_version,follow_period_id,base_price_minor,discount_minor,
                paid_minor,discount_percent,min_follow_days
         FROM chapter_purchase_discount_snapshots WHERE purchase_id=$1",
    )
    .bind(receipt.purchase_id.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(snapshot.get::<i64, _>("campaign_id"), campaign_id);
    assert_eq!(snapshot.get::<i32, _>("campaign_revision"), 1);
    assert_eq!(snapshot.get::<i16, _>("rule_version"), 1);
    assert_eq!(snapshot.get::<i64, _>("follow_period_id"), period);
    assert_eq!(snapshot.get::<i64, _>("base_price_minor"), 100);
    assert_eq!(snapshot.get::<i64, _>("discount_minor"), 20);
    assert_eq!(snapshot.get::<i64, _>("paid_minor"), 80);
    assert_eq!(snapshot.get::<i16, _>("discount_percent"), 20);
    assert_eq!(snapshot.get::<i16, _>("min_follow_days"), 90);
    let gross: i64 = sqlx::query_scalar(
        "SELECT gross_amount_minor FROM chapter_purchase_share_snapshots WHERE purchase_id=$1",
    )
    .bind(receipt.purchase_id.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        gross, 80,
        "beneficiaries receive their percentages of paid money"
    );
    let debit: i64 = sqlx::query_scalar(
        "SELECT delta_minor FROM reader_wallet_entries
         WHERE purchase_id=$1 AND kind='chapter_purchase'",
    )
    .bind(receipt.purchase_id.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(debit, -80);
    let attempts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_purchase_facts WHERE user_id=$1")
            .bind(f.reader)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(attempts, 1);

    campaigns.stop(f.owner, f.book).await.unwrap();
    assert!(campaigns.current(f.owner, f.book).await.unwrap().is_none());
    let replay = checkout.checkout_once(&buy).await.unwrap();
    assert_eq!(replay.purchase_id, receipt.purchase_id);
    assert!(replay.replayed);
    assert_eq!(replay.balance_after.get(), 220);
    let update = sqlx::query(
        "UPDATE chapter_purchase_discount_snapshots SET paid_minor=99 WHERE purchase_id=$1",
    )
    .bind(receipt.purchase_id.get())
    .execute(&f.pool)
    .await;
    assert!(
        update.is_err(),
        "frozen financial evidence may not be modified"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_07_refollow_resets_ninety_day_clock_and_campaign_stop_reprices() {
    let f = fixture("resubscribe", 300, 100).await;
    let campaigns = PgAuthorDiscounts::new(f.pool.clone());
    let checkout = PgCheckout::new(f.pool.clone());
    campaigns.create(f.owner, f.book, 20, 90, 30).await.unwrap();
    let first = follow(&f, 91).await;
    let first_offer = checkout
        .quote_single(f.reader, f.book, f.chapter)
        .await
        .unwrap();
    assert_eq!(first_offer.final_price_minor, 80);
    sqlx::query("UPDATE creator_follow_periods SET ended_at=now() WHERE id=$1")
        .bind(first)
        .execute(&f.pool)
        .await
        .unwrap();
    let second = follow(&f, 0).await;
    assert_ne!(first, second);
    let rejoined = checkout
        .quote_single(f.reader, f.book, f.chapter)
        .await
        .unwrap();
    assert_eq!(rejoined.final_price_minor, 100);
    assert_eq!(
        checkout.checkout_once(&command(&f, 80, 'D')).await,
        Err(CheckoutError::StaleOffer)
    );
    // Current subscription immediately qualifies for the alternative
    // campaign; historical periods can never accumulate their duration.
    campaigns.stop(f.owner, f.book).await.unwrap();
    campaigns.create(f.owner, f.book, 20, 0, 30).await.unwrap();
    let current_offer = checkout
        .quote_single(f.reader, f.book, f.chapter)
        .await
        .unwrap();
    assert_eq!(current_offer.final_price_minor, 80);
    assert_ne!(current_offer.campaign_id, first_offer.campaign_id);
    assert_eq!(current_offer.follow_period_id, Some(second));
    assert_eq!(
        checkout
            .checkout_once(&command_for_quote(&f, &first_offer, 'F'))
            .await,
        Err(CheckoutError::StaleOffer),
        "same final price from a different campaign and follow period is still a stale quote"
    );
    let bought = checkout
        .checkout_once(&command_for_quote(&f, &current_offer, 'E'))
        .await
        .unwrap();
    assert_eq!(bought.amount.get(), 80);
    let source: i64 = sqlx::query_scalar(
        "SELECT follow_period_id FROM chapter_purchase_discount_snapshots WHERE purchase_id=$1",
    )
    .bind(bought.purchase_id.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(source, second);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_07_expired_offer_is_not_applied_and_will_not_block_replacement() {
    let f = fixture("expiry", 200, 100).await;
    let campaigns = PgAuthorDiscounts::new(f.pool.clone());
    let checkout = PgCheckout::new(f.pool.clone());
    let id = campaigns.create(f.owner, f.book, 25, 0, 5).await.unwrap();
    follow(&f, 1).await;
    assert_eq!(
        checkout
            .quote_single(f.reader, f.book, f.chapter)
            .await
            .unwrap()
            .final_price_minor,
        75
    );
    // Bump only the disposable campaign's end time to simulate expiry.
    sqlx::query(
        "UPDATE author_discount_campaigns
         SET starts_at=now()-interval '3 days',ends_at=now()-interval '1 second'
         WHERE id=$1",
    )
    .bind(id)
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        checkout
            .quote_single(f.reader, f.book, f.chapter)
            .await
            .unwrap()
            .final_price_minor,
        100
    );
    assert_eq!(
        checkout.checkout_once(&command(&f, 75, 'F')).await,
        Err(CheckoutError::StaleOffer)
    );
    let replacement = campaigns.create(f.owner, f.book, 30, 0, 4).await.unwrap();
    assert_ne!(replacement, id);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_07_same_final_amount_with_changed_base_price_is_stale() {
    // 20% of 104 and 105 kopeks both round down to a 20/21-kopek discount
    // whose final amount is 84 kopeks. The original list price must still bind.
    let f = fixture("base_plateau", 200, 104).await;
    let campaigns = PgAuthorDiscounts::new(f.pool.clone());
    let checkout = PgCheckout::new(f.pool.clone());
    campaigns.create(f.owner, f.book, 20, 0, 30).await.unwrap();
    follow(&f, 1).await;
    let quote = checkout
        .quote_single(f.reader, f.book, f.chapter)
        .await
        .unwrap();
    assert_eq!((quote.base_price_minor, quote.final_price_minor), (104, 84));
    sqlx::query("UPDATE chapters SET price_amount=105 WHERE id=$1")
        .bind(f.chapter)
        .execute(&f.pool)
        .await
        .unwrap();
    let changed = checkout
        .quote_single(f.reader, f.book, f.chapter)
        .await
        .unwrap();
    assert_eq!(changed.final_price_minor, quote.final_price_minor);
    assert_ne!(changed.base_price_minor, quote.base_price_minor);
    assert_eq!(
        checkout
            .checkout_once(&command_for_quote(&f, &quote, 'G'))
            .await,
        Err(CheckoutError::StaleOffer),
        "same final kopecks do not make a changed list price the same quote"
    );
    let balance: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(f.reader)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(balance, 200);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_07_same_amount_with_new_campaign_revision_is_stale() {
    let f = fixture("revision", 200, 100).await;
    let campaigns = PgAuthorDiscounts::new(f.pool.clone());
    let checkout = PgCheckout::new(f.pool.clone());
    let campaign = campaigns.create(f.owner, f.book, 20, 90, 30).await.unwrap();
    follow(&f, 91).await;
    let quote = checkout
        .quote_single(f.reader, f.book, f.chapter)
        .await
        .unwrap();
    assert_eq!(quote.final_price_minor, 80);
    assert_eq!(quote.campaign_revision, Some(1));
    sqlx::query(
        "UPDATE author_discount_campaigns
         SET ends_at=ends_at+interval '1 day' WHERE id=$1",
    )
    .bind(campaign)
    .execute(&f.pool)
    .await
    .unwrap();
    let revised = checkout
        .quote_single(f.reader, f.book, f.chapter)
        .await
        .unwrap();
    assert_eq!(revised.final_price_minor, quote.final_price_minor);
    assert_eq!(revised.campaign_revision, Some(2));
    assert_eq!(
        checkout
            .checkout_once(&command_for_quote(&f, &quote, 'H'))
            .await,
        Err(CheckoutError::StaleOffer),
        "campaign revision is part of the quote even when the final price is unchanged"
    );
    let receipt = checkout
        .checkout_once(&command_for_quote(&f, &revised, 'I'))
        .await
        .unwrap();
    let revision: i32 = sqlx::query_scalar(
        "SELECT campaign_revision FROM chapter_purchase_discount_snapshots WHERE purchase_id=$1",
    )
    .bind(receipt.purchase_id.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(revision, 2);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_07_thirty_day_tier_uses_the_same_continuous_period_authority() {
    let f = fixture("thirty_day", 200, 100).await;
    let campaigns = PgAuthorDiscounts::new(f.pool.clone());
    let checkout = PgCheckout::new(f.pool.clone());
    campaigns.create(f.owner, f.book, 10, 30, 30).await.unwrap();
    let period = follow(&f, 29).await;
    let early = checkout
        .quote_single(f.reader, f.book, f.chapter)
        .await
        .unwrap();
    assert_eq!(early.final_price_minor, 100);
    sqlx::query(
        "UPDATE creator_follow_periods
         SET started_at=now()-interval '30 days' WHERE id=$1",
    )
    .bind(period)
    .execute(&f.pool)
    .await
    .unwrap();
    let mature = checkout
        .quote_single(f.reader, f.book, f.chapter)
        .await
        .unwrap();
    assert_eq!(mature.final_price_minor, 90);
    assert_eq!(mature.requires_follow_days, Some(30));
    let receipt = checkout
        .checkout_once(&command_for_quote(&f, &mature, 'J'))
        .await
        .unwrap();
    let snapshot: (i16, i16) = sqlx::query_as(
        "SELECT rule_version,min_follow_days
         FROM chapter_purchase_discount_snapshots WHERE purchase_id=$1",
    )
    .bind(receipt.purchase_id.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(snapshot, (1, 30));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_07_180_day_tier_is_supported_and_snapshotted() {
    let f = fixture("one_eighty_day", 200, 100).await;
    let campaigns = PgAuthorDiscounts::new(f.pool.clone());
    let checkout = PgCheckout::new(f.pool.clone());
    campaigns
        .create(f.owner, f.book, 20, 180, 30)
        .await
        .unwrap();
    let period = follow(&f, 179).await;
    let early = checkout
        .quote_single(f.reader, f.book, f.chapter)
        .await
        .unwrap();
    assert_eq!(early.final_price_minor, 100);
    sqlx::query(
        "UPDATE creator_follow_periods
         SET started_at=now()-interval '180 days' WHERE id=$1",
    )
    .bind(period)
    .execute(&f.pool)
    .await
    .unwrap();
    let mature = checkout
        .quote_single(f.reader, f.book, f.chapter)
        .await
        .unwrap();
    assert_eq!(mature.final_price_minor, 80);
    assert_eq!(mature.requires_follow_days, Some(180));
    let receipt = checkout
        .checkout_once(&command_for_quote(&f, &mature, 'K'))
        .await
        .unwrap();
    let days: i16 = sqlx::query_scalar(
        "SELECT min_follow_days FROM chapter_purchase_discount_snapshots WHERE purchase_id=$1",
    )
    .bind(receipt.purchase_id.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(days, 180);
}
