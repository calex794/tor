//! HTTP owns the stop witness: collectors retire only after the listener and requests drain.
pub use crate::auth::reset_queue::start_handoff as start_password_reset_handoff;
use axum::Router;
use std::{net::SocketAddr, time::Duration};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};
use tor_books::repositories::PgCatalog;
use tor_traffic_behavior::{
    domain::html_health::{HtmlHealthError, HtmlIngressStopped},
    repositories::PgRilRollups,
};

pub(crate) fn utc(epoch: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(epoch)
        .ok()
        .and_then(|at| {
            at.format(time::macros::format_description!(
                "[year]-[month]-[day] [hour]:[minute] UTC"
            ))
            .ok()
        })
        .unwrap_or_else(|| "время неизвестно".into())
}

pub struct HtmlCollector {
    stop: Option<oneshot::Sender<HtmlIngressStopped>>,
    task: Option<JoinHandle<Result<(), HtmlHealthError>>>,
}
impl HtmlCollector {
    pub fn start(catalog: &PgCatalog) -> Self {
        let (stop, receiver) = oneshot::channel();
        let pump = crate::ril::pg_pump(PgRilRollups::new(catalog.pool()));
        Self {
            stop: Some(stop),
            task: Some(pump.spawn_with_shutdown(receiver)),
        }
    }
    async fn shutdown_after_http_drained(mut self) -> bool {
        let Some(stop) = self.stop.take() else {
            return false;
        };
        if stop
            .send(HtmlIngressStopped::after_http_stopped_and_drained())
            .is_err()
        {
            return false;
        }
        let Some(mut task) = self.task.take() else {
            return false;
        };
        match tokio::time::timeout(Duration::from_secs(15), &mut task).await {
            Ok(Ok(Ok(()))) => true,
            _ => {
                task.abort();
                let _ = task.await;
                false
            }
        }
    }
}
impl Drop for HtmlCollector {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub async fn serve(
    listener: TcpListener,
    app: Router,
    html: Option<HtmlCollector>,
    observations: Option<crate::flow_exposure::ObservationCollector>,
    backgrounds: Vec<JoinHandle<()>>,
) -> std::io::Result<()> {
    let result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await;
    // Axum returns only after accepting stopped and all in-flight requests completed.
    // A listener error, task abort or dropped control is never a terminal acknowledgement.
    if result.is_ok() {
        if let Some(html) = html {
            if !html.shutdown_after_http_drained().await {
                tracing::warn!("FLOW HTML collector stopped without a durable acknowledgement");
            }
        }
        if let Some(observations) = observations {
            let probe = observations.probe();
            if !observations.shutdown_after_http_drained().await {
                tracing::warn!("FLOW card collector stopped without a durable acknowledgement");
            } else {
                // Durable retirement means the tail is accounted for, not that
                // all proof registrations were persisted. Lost/failed hours
                // remain unmeasurable and must be visible to operators.
                let counters = probe.snapshot();
                if counters.dropped > 0 || counters.failed > 0 {
                    tracing::warn!(
                        queued = counters.queued,
                        persisted = counters.persisted,
                        dropped = counters.dropped,
                        failed = counters.failed,
                        "FLOW card collector durably retired with degraded observation health"
                    );
                }
            }
        }
    }
    for task in backgrounds {
        task.abort();
        let _ = task.await;
    }
    result
}

async fn shutdown_signal() {
    let interrupt = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { () = interrupt => {}, () = terminate => {} }
}
