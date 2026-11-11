//! FLOW-09 current sample: production policy queries, fenced snapshots and real HEAD.
#[path="public_pages_pg/fixtures.rs"] mod fixtures;
#[path="public_pages_pg/head.rs"] mod head;
use fixtures::*;
use tor_search::domain::public_pages::*;

const ORIGIN:&str="https://flow-pages.example";

#[tokio::test]
#[ignore="requires disposable PostgreSQL 18 and current migrations"]
async fn baseline_unknown_network_origin_and_stale_evidence_are_distinct() {
    let f=Fixture::new().await;
    let initial=f.store.report(ORIGIN).await.unwrap();
    assert_eq!((initial.expected_targets(),initial.checked_targets(),initial.lost_targets()),(4,0,0));
    f.round(ORIGIN,Some(503)).await;
    assert!(f.store.report(ORIGIN).await.unwrap().rows.iter().all(|r|r.state==PageState::Unknown));
    f.round(ORIGIN,Some(200)).await;
    assert_eq!(f.store.report(ORIGIN).await.unwrap().checked_targets(),4);
    f.round(ORIGIN,Some(503)).await;
    let lost=f.store.report(ORIGIN).await.unwrap();
    assert_eq!((lost.checked_targets(),lost.lost_targets()),(4,4));
    f.round(ORIGIN,None).await;
    assert!(f.store.report(ORIGIN).await.unwrap().rows.iter().all(|r|r.state==PageState::Unknown));
    f.round(ORIGIN,Some(200)).await;
    let restarted=tor_search::repositories::public_pages::PgPublicPages::new(f.pool.clone());
    assert_eq!(restarted.report(ORIGIN).await.unwrap().checked_targets(),4);
    sqlx::query("UPDATE flow_public_page_probe SET finished_at=now()-interval '180.5 seconds'").execute(&f.pool).await.unwrap();
    sqlx::query("UPDATE flow_public_page_results SET checked_at=now()-interval '180.5 seconds'").execute(&f.pool).await.unwrap();
    assert_eq!(f.store.report(ORIGIN).await.unwrap().checked_targets(),0);
    f.round(ORIGIN,Some(500)).await;
    assert_eq!(f.store.report(ORIGIN).await.unwrap().lost_targets(),0,"a gap discards the stale baseline");
    f.round(ORIGIN,Some(200)).await;
    sqlx::query("UPDATE flow_public_page_probe SET finished_at=now()+interval '1 hour'").execute(&f.pool).await.unwrap();
    sqlx::query("UPDATE flow_public_page_results SET checked_at=now()+interval '1 hour'").execute(&f.pool).await.unwrap();
    let future=f.store.report(ORIGIN).await.unwrap();
    assert_eq!((future.checked_targets(),future.age_seconds),(0,None));
    f.round(ORIGIN,Some(500)).await;
    assert_eq!(f.store.report(ORIGIN).await.unwrap().lost_targets(),0,"a future old check is not a usable baseline");
    f.round(ORIGIN,Some(200)).await;
    let new_origin="https://next-origin.example";
    let different=f.store.report(new_origin).await.unwrap();
    assert_eq!(different.checked_targets(),0);
    assert!(different.rows.iter().all(|row|row.last_success_at.is_none()),"old-origin success is not this source's success");
    f.round(new_origin,Some(500)).await;
    let changed=f.store.report(new_origin).await.unwrap();
    assert_eq!(changed.lost_targets(),0);
    assert!(changed.rows.iter().all(|row|row.last_success_at.is_none()));
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM flow_public_page_results").fetch_one(&f.pool).await.unwrap(),4);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM flow_public_page_probe").fetch_one(&f.pool).await.unwrap(),1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore="requires disposable PostgreSQL 18 and current migrations"]
async fn policy_and_revision_changes_never_turn_legitimate_withdrawal_into_loss() {
    let f=Fixture::new().await;
    f.round(ORIGIN,Some(200)).await;
    f.due().await;
    let lease=f.store.claim(ORIGIN).await.unwrap().unwrap();
    let mut failed=results(&lease,Some(200));
    for row in &mut failed {
        if matches!(row.target.slot,PageSlot::Book|PageSlot::FreeChapter) { row.status=Some(404); }
    }
    // Row revision changes even when the final public policy is again identical.
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=1").execute(&f.pool).await.unwrap();
    sqlx::query("UPDATE books SET admin_hold=false WHERE id=1").execute(&f.pool).await.unwrap();
    assert!(!f.store.current(&lease.targets[2]).await.unwrap());
    f.store.save(&lease,&failed).await.unwrap();
    let report=f.store.report(ORIGIN).await.unwrap();
    assert_eq!(report.lost_targets(),0);
    assert_eq!(report.rows[2].state,PageState::Unknown);
    assert_eq!(report.rows[3].state,PageState::Unknown);
    f.round(ORIGIN,Some(200)).await;
    sqlx::query("UPDATE chapters SET access_level='paid',price_amount=100 WHERE id=1").execute(&f.pool).await.unwrap();
    let paid=f.store.report(ORIGIN).await.unwrap();
    assert_eq!(paid.rows[3].state,PageState::NoTarget);assert_eq!(paid.rows[3].path,None);
    sqlx::query("UPDATE chapters SET access_level='free',price_amount=0,reader_released_at=now()+interval '1 day' WHERE id=1").execute(&f.pool).await.unwrap();
    assert_eq!(f.store.report(ORIGIN).await.unwrap().rows[3].state,PageState::NoTarget);
    sqlx::query("UPDATE chapters SET reader_released_at=NULL WHERE id=1").execute(&f.pool).await.unwrap();
    for assignment in ["visibility='draft'","visibility_status='hidden'","admin_hold=true","is_deleted=true","access_level='adult'","lifecycle_status='pending_deletion'"] {
        sqlx::query(&format!("UPDATE books SET {assignment} WHERE id=1")).execute(&f.pool).await.unwrap();
        let report=f.store.report(ORIGIN).await.unwrap();
        assert!(report.rows[2..].iter().all(|row|row.state==PageState::NoTarget && row.path.is_none()));
        assert_eq!(report.lost_targets(),0);
        sqlx::query("UPDATE books SET visibility='public',visibility_status='active',admin_hold=false,is_deleted=false,access_level='basic',lifecycle_status='in_progress' WHERE id=1")
            .execute(&f.pool).await.unwrap();
    }
    f.round(ORIGIN,Some(200)).await;
    sqlx::query("DELETE FROM chapter_publications WHERE chapter_id=1").execute(&f.pool).await.unwrap();
    assert_eq!(f.store.report(ORIGIN).await.unwrap().rows[3].state,PageState::NoTarget);
    f.cleanup().await;
}

#[tokio::test]
#[ignore="requires disposable PostgreSQL 18 and current migrations"]
async fn stale_epoch_cannot_commit_and_real_read_is_read_only_and_bounded() {
    let f=Fixture::new().await;
    let old=f.store.claim(ORIGIN).await.unwrap().unwrap();
    assert!(f.store.claim(ORIGIN).await.unwrap().is_none());
    let mut malformed=old.clone();malformed.targets.swap(0,1);
    assert_eq!(f.store.save(&malformed,&results(&malformed,Some(200))).await,Err(PageProbeError::Invalid));
    sqlx::query("UPDATE flow_public_page_probe SET lease_until=now()-interval '1 second'").execute(&f.pool).await.unwrap();
    assert_eq!(f.store.save(&old,&results(&old,Some(200))).await,Err(PageProbeError::Stale));
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM flow_public_page_results").fetch_one(&f.pool).await.unwrap(),0);
    f.due().await;
    let fresh=f.store.claim(ORIGIN).await.unwrap().unwrap();
    assert!(fresh.epoch>old.epoch);
    assert_eq!(f.store.save(&old,&results(&old,Some(200))).await,Err(PageProbeError::Stale));
    f.store.save(&fresh,&results(&fresh,Some(200))).await.unwrap();
    assert_eq!(f.store.save(&fresh,&results(&fresh,Some(200))).await,Err(PageProbeError::Stale));
    assert_eq!(f.store.report(ORIGIN).await.unwrap().checked_targets(),4);
    let readonly=f.readonly().await;
    let ro=tor_search::repositories::public_pages::PgPublicPages::new(readonly.clone());
    assert_eq!(ro.report(ORIGIN).await.unwrap().checked_targets(),4);
    readonly.close().await;
    let mut locked=f.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE flow_public_page_results IN ACCESS EXCLUSIVE MODE").execute(&mut *locked).await.unwrap();
    let started=std::time::Instant::now();
    assert!(matches!(f.store.report(ORIGIN).await,Err(PageProbeError::Unavailable)));
    assert!(started.elapsed()<std::time::Duration::from_secs(1));
    locked.rollback().await.unwrap();
    assert_eq!(f.store.report(ORIGIN).await.unwrap().checked_targets(),4);
    f.cleanup().await;
}
