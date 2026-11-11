//! Short nonblocking producer/barrier fence. No database work or waiting occurs
//! while held; a paused producer cannot enqueue behind an acknowledged barrier.
use crate::domain::html_health::HTML_CLOCK_MARGIN_MS;
use std::{
    sync::{atomic::{AtomicBool,Ordering},Mutex,MutexGuard},
    time::{Duration,Instant,SystemTime,UNIX_EPOCH},
};

#[derive(Default)]
pub(super) struct FlowGate {
    managed:AtomicBool,
    ready:AtomicBool,
    state:Mutex<Option<ClockLease>>,
}
struct ClockLease {
    started:Instant,
    db_ms:i64,
}
impl FlowGate {
    pub(super) fn manage(&self) {self.managed.store(true,Ordering::Release);}
    pub(super) fn lock(&self)->Option<FlowGuard<'_>>{
        let state=self.state.try_lock().ok()?;
        Some(FlowGuard{state,managed:self.managed.load(Ordering::Acquire),ready:&self.ready})
    }
    pub(super) fn activate(&self,db_ms:i64,started:Instant)->bool{
        let Ok(mut state)=self.state.try_lock() else {return false;};
        if started.elapsed()>=Duration::from_secs(25) {return false;}
        *state=Some(ClockLease{started,db_ms});
        self.ready.store(true,Ordering::Release);
        true
    }
    pub(super) fn close(&self){
        self.ready.store(false,Ordering::Release);
        if let Ok(mut state)=self.state.try_lock(){*state=None;}
    }
}
pub(super) struct FlowGuard<'a>{
    state:MutexGuard<'a,Option<ClockLease>>,
    managed:bool,
    ready:&'a AtomicBool,
}
impl FlowGuard<'_>{
    pub(super) fn observation_time(&self)->Option<SystemTime>{
        let now=SystemTime::now();
        if !self.managed {return Some(now);}
        if !self.ready.load(Ordering::Acquire) {return None;}
        let lease=self.state.as_ref()?;
        let elapsed=lease.started.elapsed();
        let actual=i64::try_from(now.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()?;
        let expected=lease.db_ms.saturating_add(elapsed.as_millis().min(i64::MAX as u128) as i64);
        (elapsed<Duration::from_secs(25) && actual.abs_diff(expected)<=HTML_CLOCK_MARGIN_MS as u64).then_some(now)
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn lease_and_clock_are_checked_even_when_a_producer_keeps_the_guard(){
        let gate=FlowGate::default();
        gate.manage();
        assert!(gate.lock().unwrap().observation_time().is_none());
        let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
        assert!(gate.activate(now,Instant::now()));
        let guard=gate.lock().unwrap();
        assert!(guard.observation_time().is_some());
        assert!(gate.lock().is_none());
        drop(guard);
        assert!(gate.activate(now-10_000,Instant::now()));
        assert!(gate.lock().unwrap().observation_time().is_none());
        assert!(!gate.activate(now,Instant::now()-Duration::from_secs(25)));
        gate.close();
        assert!(gate.lock().unwrap().observation_time().is_none());
    }
}
