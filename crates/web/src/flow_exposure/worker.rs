use super::*;
use std::time::Duration;

pub struct Worker {
    pub(super) store: PgExposure,
    pub(super) receiver: mpsc::Receiver<IssuedObservation>,
    pub(super) counters: Arc<Counters>,
}
impl Worker {
    pub async fn run(mut self) {
        let mut cleanup = tokio::time::interval(Duration::from_secs(5));
        cleanup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _=cleanup.tick()=>{
                    if !matches!(tokio::time::timeout(Duration::from_secs(2),self.store.prune()).await,Ok(Ok(_))){
                        self.counters.failed.fetch_add(1,Ordering::Relaxed);
                    }
                }
                first=self.receiver.recv()=>{
                    let Some(first)=first else {break};let mut batch=vec![first];
                    while batch.len()<64 {match self.receiver.try_recv(){Ok(next)=>batch.push(next),Err(_)=>break}}
                    match tokio::time::timeout(Duration::from_secs(2),self.store.register(&batch)).await {
                        Ok(Ok(count))=>{
                            self.counters.persisted.fetch_add(count as u64,Ordering::Relaxed);
                            self.counters.failed.fetch_add(batch.len() as u64-count as u64,Ordering::Relaxed);
                        }
                        _=>{self.counters.failed.fetch_add(batch.len() as u64,Ordering::Relaxed);}
                    }
                }
            }
        }
    }
}
