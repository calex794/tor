//! G/I supervised bounded feedback work. Never execute a graph rebuild in HTTP.
use std::time::Duration;
use tokio::{task::JoinHandle, time::sleep};
use tor_recommendations::repositories::PgRecommendations;

pub fn spawn(store: PgRecommendations) -> Option<JoinHandle<()>> {
    if std::env::var("TOR_RECOMMENDATIONS_ENABLED").as_deref() != Ok("1") {
        return None;
    }
    Some(tokio::spawn(async move {
        let mut loops = 0_u64;
        loop {
            let collect = std::env::var("TOR_REC_ATTRIBUTION_ENABLED").as_deref() == Ok("1");
            // REC and organic are separate collection boundaries. Disabling
            // organic must not write unattributed C reads or resolve any
            // previously issued organic tickets into new organic conversions.
            let collect_organic =
                collect && std::env::var("TOR_ORGANIC_ATTRIBUTION_ENABLED").as_deref() == Ok("1");
            let result = store.tick_feedback(collect, collect_organic).await;
            if result.is_err() {
                eprintln!("rec_feedback_worker: batch failed; retry deferred");
            }
            if loops.is_multiple_of(120) {
                let _ = store.schedule_due_refresh().await;
            }
            if store.advance_generation().await.is_err() {
                eprintln!("rec_generation_worker: build failed; previous active preserved");
            }
            loops = loops.wrapping_add(1);
            if loops.is_multiple_of(120) && store.prune().await.is_err() {
                eprintln!("rec_feedback_worker: retention batch failed");
            }
            sleep(Duration::from_secs(5)).await;
        }
    }))
}
