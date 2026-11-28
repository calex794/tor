//! Shared process-local authority for the admin controls and future worker.
//! Never restored from PostgreSQL; every new process starts stopped.
use std::sync::OnceLock;
use tokio::sync::{Mutex, MutexGuard};
use tor_mail::services::launch::LaunchFence;

static FENCE: OnceLock<LaunchFence> = OnceLock::new();
static CONTROL: Mutex<()> = Mutex::const_new(());

/// All runtime components must use this instance, never an independent fence.
pub fn launch_fence() -> &'static LaunchFence {
    FENCE.get_or_init(LaunchFence::new)
}

/// Serialize operator mutations with the future activation flow. Never hold
/// this lock during SMTP. Revocation precedes even waiting for the lock, so
/// an in-flight review/confirmation cannot win over an emergency stop.
/// A poisoned fence already fails closed; still persist the database stop.
pub(crate) async fn stop_for_change() -> MutexGuard<'static, ()> {
    let _ = launch_fence().stop();
    let guard = CONTROL.lock().await;
    let _ = launch_fence().stop();
    guard
}
