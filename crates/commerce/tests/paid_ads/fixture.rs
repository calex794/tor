use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::time::{SystemTime, UNIX_EPOCH};
use tor_commerce::repositories::ad_posting::PgAdPosting;
use tor_contracts::ads::*;

pub struct Fixture {
    pub pool: PgPool,
    pub posting: PgAdPosting,
    pub buyer: AdActor,
    pub publisher: AdActor,
    pub viewer: AdActor,
    pub book: i64,
    pub host: i64,
    pub chapter: i64,
    pub package: i64,
    pub offer: i64,
}
pub fn key(value: u8) -> String {
    format!("{}8", char::from(value).to_string().repeat(42))
}
pub async fn user(pool: &PgPool, label: &str) -> AdActor {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
   VALUES($1,$2,'$argon2id$ci') RETURNING id",
    )
    .bind(format!("ads-{label}-{stamp}@example.invalid"))
    .bind(label)
    .fetch_one(pool)
    .await
    .unwrap();
    let digest: [u8; 32] = Sha256::digest(format!("ads-fixture:{id}:{stamp}").as_bytes()).into();
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at) VALUES($1,$2,now()+interval '1 hour')",
    )
    .bind(id)
    .bind(digest.as_slice())
    .execute(pool)
    .await
    .unwrap();
    AdActor {
        user_id: id,
        session_digest: digest,
    }
}
pub async fn setup(price: i64, target: i64, hold: i32, cap: i32) -> Fixture {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable tor_ci required");
    assert!(url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let buyer = user(&pool, "advertiser").await;
    let publisher = user(&pool, "publisher").await;
    let viewer = user(&pool, "viewer").await;
    let promoted = book(&pool, buyer.user_id).await;
    let host = book(&pool, publisher.user_id).await;
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
  VALUES($1,1,'Free host','public','free') RETURNING id",
    )
    .bind(host)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
  VALUES($1,1,'Fixture free body') RETURNING id",
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
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) VALUES($1,10000,'RUB')",
    )
    .bind(buyer.user_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO ad_publisher_settings(user_id,enabled) VALUES($1,true)")
        .bind(publisher.user_id)
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let package:i64=sqlx::query_scalar("INSERT INTO ad_packages(current_revision,enabled,created_by) VALUES(1,true,$1) RETURNING id")
  .bind(buyer.user_id).fetch_one(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO ad_package_revisions(package_id,revision,price_minor,price_micros_at_offer,target_impressions,
  publisher_share_bps,pricing_algorithm,hold_seconds,daily_cap,hourly_cap,session_cap,terms_version,published_by)
  VALUES($1,1,$2,$2::NUMERIC*10000,$3,7000,'cumulative_minor_v1',$4,$5,$5,$5,'ci-fixture-v1',$6)")
  .bind(package).bind(price).bind(target).bind(hold).bind(cap).bind(buyer.user_id).execute(&mut *tx).await.unwrap();
    let offer:i64=sqlx::query_scalar("INSERT INTO ad_daily_offers(current_revision,enabled,created_by) VALUES(1,true,$1) RETURNING id")
  .bind(buyer.user_id).fetch_one(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO ad_daily_offer_revisions(offer_id,revision,slot,rate_minor,min_days,max_days,cancellation_policy,terms_version,published_by,daily_cap,hourly_cap,session_cap,min_interval_seconds)
  VALUES($1,1,'home_featured',100,1,30,'unused_full_utc_days','ci-days-v1',$2,100,100,100,0)")
  .bind(offer).bind(buyer.user_id).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    Fixture {
        posting: PgAdPosting::new(pool.clone()),
        pool,
        buyer,
        publisher,
        viewer,
        book: promoted,
        host,
        chapter,
        package,
        offer,
    }
}
pub async fn book(pool: &PgPool, owner: i64) -> i64 {
    sqlx::query_scalar("INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id)
  VALUES($1,'ru','Paid ads fixture','public','basic',$2) RETURNING id")
  .bind(format!("ads:book:{owner}")).bind(owner).fetch_one(pool).await.unwrap()
}
impl Fixture {
    pub fn command(&self, k: u8) -> AdPurchaseCommand {
        AdPurchaseCommand {
            actor: self.buyer.clone(),
            command_key: key(k),
            purchase: AdPurchase::Chapter(ChapterPurchase {
                book_id: self.book,
                package_id: self.package,
                expected_package_revision: 1,
            }),
        }
    }
    pub async fn daily(&self, k: u8) -> AdPurchaseCommand {
        let row=sqlx::query("SELECT to_char(date_trunc('day',now() AT TIME ZONE 'UTC'),'YYYY-MM-DD')||'T00:00:00Z' AS start,
   to_char(date_trunc('day',now() AT TIME ZONE 'UTC')+interval '3 days','YYYY-MM-DD')||'T00:00:00Z' AS finish")
   .fetch_one(&self.pool).await.unwrap();
        AdPurchaseCommand {
            actor: self.buyer.clone(),
            command_key: key(k),
            purchase: AdPurchase::Daily(DailyPurchase {
                book_id: self.book,
                offer_id: self.offer,
                offer_revision: 1,
                placement: tor_contracts::exposure::PromotionSlot::HomeFeatured,
                starts_at: row.get("start"),
                ends_at: row.get("finish"),
                total_days: 3,
                prepaid_days: 3,
            }),
        }
    }
    pub async fn assign(&self, campaign: AdCampaignId) -> i64 {
        let mut tx = self.pool.begin().await.unwrap();
        let generation:i64=sqlx::query_scalar("INSERT INTO ad_delivery_generations(status,host_ceiling) VALUES('active',$1) RETURNING id")
   .bind(self.host).fetch_one(&mut *tx).await.unwrap();
        sqlx::query("UPDATE ad_delivery_generations SET status='retired' WHERE id=(SELECT generation_id FROM ad_delivery_active)")
   .execute(&mut *tx).await.unwrap();
        sqlx::query(
            "INSERT INTO ad_delivery_active(singleton,generation_id) VALUES(true,$1)
   ON CONFLICT(singleton) DO UPDATE SET generation_id=$1",
        )
        .bind(generation)
        .execute(&mut *tx)
        .await
        .unwrap();
        let assignment=sqlx::query_scalar("INSERT INTO ad_delivery_assignments(generation_id,host_book_id,publisher_id,campaign_id,rank)
   VALUES($1,$2,$3,$4,1) RETURNING id").bind(generation).bind(self.host).bind(self.publisher.user_id)
   .bind(campaign.get()).fetch_one(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        assignment
    }
    pub async fn opportunity(&self, assignment: i64, n: i64) -> AdImpressionClaim {
        // Canonical 32-byte base64url token; fixtures bypass only the one-second
        // wait by issuing in the past, never the server validation or money path.
        let mut bytes = [0_u8; 32];
        bytes[..8].copy_from_slice(&n.to_be_bytes());
        bytes[8..16].copy_from_slice(&self.viewer.user_id.to_be_bytes());
        let token = base64url(&bytes);
        let digest = Sha256::digest(token.as_bytes());
        sqlx::query("INSERT INTO ad_viewability_opportunities(token_hash,assignment_id,viewer_id,session_hash,host_book_id,host_chapter_id,issued_at,expires_at)
   VALUES($1,$2,$3,$4,$5,$6,clock_timestamp()-interval '2 seconds',clock_timestamp()+interval '4 minutes')")
   .bind(digest.as_slice()).bind(assignment).bind(self.viewer.user_id).bind(self.viewer.session_digest.as_slice())
   .bind(self.host).bind(self.chapter).execute(&self.pool).await.unwrap();
        let context = sqlx::query(
            "SELECT campaign_id,generation_id FROM ad_delivery_assignments WHERE id=$1",
        )
        .bind(assignment)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        AdImpressionClaim {
            actor: self.viewer.clone(),
            context: AdDeliveryContext {
                campaign_id: AdCampaignId::new(context.get("campaign_id")).unwrap(),
                generation_id: AdGenerationId::new(context.get("generation_id")).unwrap(),
                assignment_id: AdAssignmentId::new(assignment).unwrap(),
            },
            viewability_token: token,
            visible_percent: 60,
            duration_ms: 1000,
        }
    }
    pub async fn balance(&self, owner: i64) -> i64 {
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(owner)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut value = 0_u32;
    let mut bits = 0;
    for byte in bytes {
        value = (value << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            out.push(char::from(ALPHABET[((value >> bits) & 63) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(ALPHABET[((value << (6 - bits)) & 63) as usize]));
    }
    out
}
