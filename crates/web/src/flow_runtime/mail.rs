//! One explicitly configured server-owned mail runtime. A new process is
//! always stopped, including when a restored database says enabled.
use crate::auth::reset_queue::{build_password_reset_dispatch, configured_signer};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle};
use tor_accounts::{
    domain::email::disposable::DisposableMode,
    repositories::PgAccounts,
    services::{recipient_dns::MailDnsResolver, recipient_preflight::RecipientPreflight},
};
use tor_mail::{
    domain::postbox_profile::ProfileMasterKey,
    services::{
        launch::{LaunchError, LaunchFence},
        postbox::ResetOrigin,
    },
};
use zeroize::Zeroizing;

// DispatchWorker bounds an entire attempt at 90 seconds. Do not equate
// timeout or forced abortion with a durable send acknowledgement.
const DRAIN_LIMIT: Duration = Duration::from_secs(95);

#[derive(Clone)]
pub(crate) struct MailControl {
    fence: Arc<LaunchFence>,
}

impl MailControl {
    pub(crate) fn fence(&self) -> &LaunchFence {
        &self.fence
    }

    pub(crate) fn stop(&self) -> Result<(), LaunchError> {
        self.fence.stop()
    }
}

#[derive(Clone)]
pub(super) struct StopHandle {
    control: MailControl,
    signal: watch::Sender<bool>,
}

impl StopHandle {
    pub(super) fn request_stop(&self) {
        if self.control.fence.retire().is_err() {
            tracing::warn!("System mail launch authority unavailable at shutdown");
        }
        self.signal.send_replace(true);
    }
}

pub(super) struct ManagedMail {
    stop: StopHandle,
    task: Option<JoinHandle<()>>,
}

impl ManagedMail {
    pub(super) fn from_env(accounts: Option<&PgAccounts>) -> Result<Option<Self>, &'static str> {
        let selected = match std::env::var("TOR_MAIL_DISPATCH_WORKER_ENABLED") {
            Ok(value) if value == "1" => true,
            Ok(value) if value == "0" => false,
            Err(std::env::VarError::NotPresent) => false,
            _ => return Err("invalid TOR_MAIL_DISPATCH_WORKER_ENABLED"),
        };
        if !selected {
            return Ok(None);
        }
        if std::env::var("TOR_AUTH_ENABLED").as_deref() != Ok("1") {
            return Err("mail worker requires authenticated PostgreSQL runtime");
        }
        let accounts = accounts.ok_or("mail worker requires PostgreSQL")?;
        let signer = configured_signer().ok_or("mail worker reset configuration unavailable")?;
        let origin = std::env::var("TOR_PUBLIC_ORIGIN").map_err(|_| "mail origin unavailable")?;
        let origin = ResetOrigin::new(&origin).ok_or("mail worker requires HTTPS origin")?;
        let master = Arc::new(profile_master_key()?);
        let preflight = Arc::new(RecipientPreflight::new(
            system_dns()?,
            DisposableMode::Observe,
        ));
        let fence = Arc::new(LaunchFence::new());
        let worker = build_password_reset_dispatch(
            accounts.clone(),
            signer,
            preflight,
            fence.clone(),
            master,
            origin,
        );
        // No DB observation, queue claim, DNS lookup or connection until
        // the same fresh process fence has been explicitly armed.
        Ok(Some(Self::spawn(fence, move |shutdown| async move {
            worker.run(shutdown).await;
        })))
    }

    fn spawn<F, Fut>(fence: Arc<LaunchFence>, run: F) -> Self
    where
        F: FnOnce(watch::Receiver<bool>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (signal, receiver) = watch::channel(false);
        Self {
            stop: StopHandle {
                control: MailControl { fence },
                signal,
            },
            task: Some(tokio::spawn(run(receiver))),
        }
    }

    pub(super) fn control(&self) -> MailControl {
        self.stop.control.clone()
    }

    pub(super) fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }

    pub(super) async fn drain(mut self) -> bool {
        self.stop.request_stop();
        let Some(mut task) = self.task.take() else {
            return false;
        };
        match tokio::time::timeout(DRAIN_LIMIT, &mut task).await {
            Ok(Ok(())) => true,
            _ => {
                task.abort();
                let _ = task.await;
                false
            }
        }
    }
}

impl Drop for ManagedMail {
    fn drop(&mut self) {
        self.stop.request_stop();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub(crate) fn profile_master_key() -> Result<ProfileMasterKey, &'static str> {
    let unavailable = "Postbox profile master key unavailable";
    let version = std::env::var("TOR_POSTBOX_PROFILE_KEY_VERSION")
        .map_err(|_| unavailable)?
        .parse::<i32>()
        .map_err(|_| unavailable)?;
    let encoded =
        Zeroizing::new(std::env::var("TOR_POSTBOX_PROFILE_MASTER_KEY").map_err(|_| unavailable)?);
    ProfileMasterKey::from_base64(version, &encoded).map_err(|_| unavailable)
}

#[cfg(any(unix, target_os = "windows"))]
fn system_dns() -> Result<Arc<dyn MailDnsResolver>, &'static str> {
    use tor_accounts::repositories::recipient_dns::SystemMailDns;
    Ok(Arc::new(SystemMailDns::from_system()?))
}

#[cfg(not(any(unix, target_os = "windows")))]
fn system_dns() -> Result<Arc<dyn MailDnsResolver>, &'static str> {
    Err("system mail DNS unsupported")
}

#[cfg(test)]
mod tests;
