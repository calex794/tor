use super::*;
use tor_commerce::repositories::{
    direct_swaps::{PgDirectSwaps, SwapError},
    swap_intent::DirectSwapIntent,
};

async fn swap_fixture(tag: &str, target_price: i64)
    -> (PgPool, DirectSwapIntent, i64, i64)
{
    let (pool,buyer,source_book,chapter,version) = fixture(tag,1000,300).await;
    let owner: i64 = sqlx::query_scalar(
        "SELECT owner_user_id FROM books WHERE id=$1",
    ).bind(source_book).fetch_one(&pool).await.unwrap();
    let legal = format!("ci:swap:legal:{buyer}");
    let terms = format!("ci:swap:terms:{buyer}");
    sqlx::query(
        "INSERT INTO finance_direct_refund_policy_catalog
          (version,approval_reference,approved_by_user_id,
           approved_at,effective_at,window_seconds)
         VALUES($1,$2,$3,now()-INTERVAL '2 days',
                now()-INTERVAL '1 day',86400)",
    ).bind(&legal).bind(format!("ci:swap:approval:{buyer}")).bind(owner)
     .execute(&pool).await.unwrap();
    let target_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','Вторая книга того же автора','public',$2) RETURNING id",
    ).bind(format!("ci:swap:target:{tag}")).bind(owner)
     .fetch_one(&pool).await.unwrap();
    for book in [source_book,target_book] {
        sqlx::query(
            "INSERT INTO finance_revenue_terms
             (book_id,policy_version,platform_fee_bps,
              refund_policy_version,refund_window_seconds)
             VALUES($1,$2,2000,$3,86400)",
        ).bind(book).bind(&terms).bind(&legal).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO finance_revenue_term_states(book_id,policy_version)
             VALUES($1,$2)",
        ).bind(book).bind(&terms).execute(&pool).await.unwrap();
    }
    let source_purchase = PgCheckout::new(pool.clone())
        .with_creator_posting(true)
        .checkout_once(&command(buyer,chapter,version,300,'A'))
        .await.unwrap().purchase_id.get();
    let (target,target_version) = chapter_with_offer(&pool,target_book,1,target_price).await;
    let intent = DirectSwapIntent {
        buyer_id:buyer,source_purchase_id:source_purchase,
        target_chapter_id:target,target_version_id:target_version,
        source_price_minor:300,target_price_minor:target_price,
    };
    (pool,intent,chapter,version)
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d06_swap_atomically_reverses_source_and_posts_target() {
    let (pool,i,source_chapter,source_version) =
        swap_fixture("d06_swap_higher",450).await;
    let key=IdempotencyKey::parse(&format!("{}8","W".repeat(42))).unwrap();
    let swaps=PgDirectSwaps::new(pool.clone());
    let first=swaps.swap_once(i,&key).await.unwrap();
    assert!(!first.replayed);
    assert_eq!(first.delta_minor,150);
    assert_eq!(first.balance_after,550);
    let replay=swaps.swap_once(i,&key).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.new_purchase,first.new_purchase);
    let wallet: i64=sqlx::query_scalar(
        "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1",
    ).bind(i.buyer_id).fetch_one(&pool).await.unwrap();
    assert_eq!(wallet,550);
    let (source_revoked,new_owned,old_credits,new_debits,platform_reverse,creator_reverse):
        (bool,bool,i64,i64,i64,i64)=sqlx::query_as(
        "SELECT
           EXISTS(SELECT 1 FROM chapter_purchase_revocations
                  WHERE purchase_id=$1 AND reason='purchase_swap'),
           EXISTS(SELECT 1 FROM library_purchased_chapters
                  WHERE user_id=$2 AND chapter_id=$3 AND purchase_id=$4),
           (SELECT count(*) FROM reader_wallet_entries
            WHERE purchase_id=$1 AND kind='chapter_swap_credit'),
           (SELECT count(*) FROM reader_wallet_entries
            WHERE purchase_id=$4 AND kind='chapter_purchase'),
           (SELECT platform_reversal_minor FROM finance_direct_swap_postings
            WHERE source_purchase_id=$1),
           (SELECT COALESCE(sum(amount_minor),0)::BIGINT
            FROM finance_direct_swap_creator_reversals
            WHERE source_purchase_id=$1)",
    ).bind(i.source_purchase_id).bind(i.buyer_id).bind(i.target_chapter_id)
     .bind(first.new_purchase).fetch_one(&pool).await.unwrap();
    assert!(source_revoked && new_owned);
    assert_eq!((old_credits,new_debits,platform_reverse,creator_reverse),(1,1,60,240));
    let old_readable:bool=sqlx::query_scalar(
        "SELECT tor_purchase_version_readable($1,$2,$3,$4)",
    ).bind(i.buyer_id).bind(i.source_purchase_id).bind(source_chapter)
     .bind(source_version).fetch_one(&pool).await.unwrap();
    let new_readable:bool=sqlx::query_scalar(
        "SELECT tor_purchase_version_readable($1,$2,$3,$4)",
    ).bind(i.buyer_id).bind(first.new_purchase)
     .bind(i.target_chapter_id).bind(i.target_version_id)
     .fetch_one(&pool).await.unwrap();
    assert!(!old_readable && new_readable);
    assert_eq!(
        swaps.swap_once(DirectSwapIntent{target_price_minor:451,..i},&key).await,
        Err(SwapError::Conflict)
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d06_swap_short_wallet_rolls_back_all_postings_and_access() {
    let (pool,i,_,_)=swap_fixture("d06_swap_no_balance",1100).await;
    let key=IdempotencyKey::parse(&format!("{}8","X".repeat(42))).unwrap();
    assert_eq!(PgDirectSwaps::new(pool.clone()).swap_once(i,&key).await,
               Err(SwapError::Insufficient));
    let (wallet,swap_count,credits,revocations,target_purchases):
        (i64,i64,i64,i64,i64)=sqlx::query_as(
        "SELECT balance_minor,
           (SELECT count(*) FROM finance_direct_swap_postings
            WHERE source_purchase_id=$2),
           (SELECT count(*) FROM reader_wallet_entries
            WHERE purchase_id=$2 AND kind='chapter_swap_credit'),
           (SELECT count(*) FROM chapter_purchase_revocations WHERE purchase_id=$2),
           (SELECT count(*) FROM chapter_purchase_facts
            WHERE chapter_id=$3 AND user_id=$1)
         FROM reader_wallet_accounts WHERE user_id=$1",
    ).bind(i.buyer_id).bind(i.source_purchase_id).bind(i.target_chapter_id)
     .fetch_one(&pool).await.unwrap();
    assert_eq!((wallet,swap_count,credits,revocations,target_purchases),(700,0,0,0,0));
    pool.close().await;
}
