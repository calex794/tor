use aws_sdk_s3::{
    config::{
        interceptors::{BeforeTransmitInterceptorContextMut, FinalizerInterceptorContextRef},
        ConfigBag, Intercept, RuntimeComponents,
    },
    error::ProvideErrorMetadata,
    operation::get_object::GetObjectError,
};
use aws_smithy_runtime_api::client::connection::CaptureSmithyConnection;
use std::{
    cell::Cell,
    error::Error,
    net::SocketAddr,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_contracts::paid_delivery::ObjectRef;
use tor_storage::{
    domain::provider::StorageError, repositories::s3::S3PrivateStore,
    services::provider::PrivateObjectStore,
};

#[derive(Clone, Copy)]
enum GetOutcome {
    Headers(Option<u16>),
    Failed {
        status: Option<u16>,
        category: &'static str,
        service_code: &'static str,
        connector_kind: &'static str,
        io_kind: Option<std::io::ErrorKind>,
    },
}

#[derive(Clone, Copy, Default)]
pub(super) struct SocketOutcome {
    ports: Option<(u16, u16)>,
    status: Option<u16>,
    response_close: Option<bool>,
    finished_ms: u128,
}

#[derive(Clone, Copy, Default)]
struct OperationObservation {
    strict_get: bool,
    get: Option<GetOutcome>,
    socket: Option<SocketOutcome>,
}

tokio::task_local! {
    static OBSERVATION: Cell<OperationObservation>;
}

fn loopback_ports(local: SocketAddr, peer: SocketAddr) -> Option<(u16, u16)> {
    (local.ip().is_loopback() && peer.ip().is_loopback()).then_some((local.port(), peer.port()))
}

fn capture_ports(cfg: &ConfigBag) -> Option<(u16, u16)> {
    let connection = cfg.load::<CaptureSmithyConnection>()?.get()?;
    loopback_ports(connection.local_addr()?, connection.remote_addr()?)
}

fn socket_line(phase: &str, outcome: Option<SocketOutcome>) -> String {
    let observation = outcome.unwrap_or_default();
    let (local, peer) = observation.ports.unzip();
    let status = observation.status;
    let close = observation.response_close;
    let finished = outcome.map(|value| value.finished_ms);
    format!(
        "G09_SOCKET phase={phase} local_port={local:?} peer_port={peer:?} http_status={status:?} response_close={close:?} unix_ms_end={finished:?}"
    )
}

#[derive(Debug)]
pub(super) struct ObserveFirstGet;

impl Intercept for ObserveFirstGet {
    fn name(&self) -> &'static str {
        "G09FirstGetDiagnostics"
    }

    fn read_after_execution(
        &self,
        context: &FinalizerInterceptorContextRef<'_>,
        _: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // Read the SDK's existing capture; never mutate or poison a connection.
        // Background load cannot replace the serial operation's task-local evidence.
        let _ = OBSERVATION.try_with(|slot| {
            let mut observation = slot.get();
            let response = context.response();
            let response_close = response.map(|value| {
                value.headers().get("connection").is_some_and(|header| {
                    header
                        .split(',')
                        .any(|token| token.trim().eq_ignore_ascii_case("close"))
                })
            });
            let status = response.map(|value| value.status().as_u16());
            observation.socket = Some(SocketOutcome {
                ports: capture_ports(cfg),
                status,
                response_close,
                finished_ms: unix_millis(),
            });
            if !observation.strict_get {
                slot.set(observation);
                return;
            }
            observation.get = context.output_or_error().map(|outcome| match outcome {
                Ok(_) => GetOutcome::Headers(status),
                Err(error) => {
                    let sdk_error = error
                        .as_operation_error()
                        .and_then(|value| value.downcast_ref::<GetObjectError>());
                    let category = if error.is_operation_error() {
                        "service"
                    } else if error.is_timeout_error() {
                        "timeout"
                    } else if error.is_connector_error() {
                        "connector"
                    } else if error.is_response_error() {
                        "response"
                    } else {
                        "sdk"
                    };
                    let connector_kind = error.as_connector_error().map_or("none", |connector| {
                        if connector.is_io() {
                            "io"
                        } else if connector.is_timeout() {
                            "timeout"
                        } else if connector.is_user() {
                            "user"
                        } else if connector.as_other().is_some() {
                            "classified-other"
                        } else {
                            "unclassified"
                        }
                    });
                    GetOutcome::Failed {
                        status,
                        category,
                        service_code: safe_service_code(sdk_error.and_then(|value| value.code())),
                        connector_kind,
                        io_kind: io_error_kind(error),
                    }
                }
            });
            slot.set(observation);
        });
        Ok(())
    }
}

/// Apply only to the independent control on disposable HTTP/1.1 MinIO.
#[derive(Debug)]
pub(super) struct CloseConnection;

impl Intercept for CloseConnection {
    fn name(&self) -> &'static str {
        "G09ConnectionCloseControl"
    }

    fn modify_before_transmit(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _: &RuntimeComponents,
        _: &mut ConfigBag,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        context
            .request_mut()
            .headers_mut()
            .insert("connection", "close");
        Ok(())
    }
}

fn safe_service_code(code: Option<&str>) -> &'static str {
    match code {
        Some("AccessDenied") => "AccessDenied",
        Some("InternalError") => "InternalError",
        Some("NoSuchBucket") => "NoSuchBucket",
        Some("NoSuchKey") => "NoSuchKey",
        Some("PreconditionFailed") => "PreconditionFailed",
        Some("RequestTimeout") => "RequestTimeout",
        Some("ServiceUnavailable") => "ServiceUnavailable",
        Some("SlowDown") => "SlowDown",
        Some(_) => "other",
        None => "none",
    }
}

fn io_error_kind(mut error: &(dyn Error + 'static)) -> Option<std::io::ErrorKind> {
    loop {
        if let Some(io) = error.downcast_ref::<std::io::Error>() {
            return Some(io.kind());
        }
        error = error.source()?;
    }
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis())
}

pub(super) async fn observe_put(
    store: &S3PrivateStore,
    object: &ObjectRef,
    bytes: &[u8],
) -> (Result<(), StorageError>, Option<SocketOutcome>) {
    OBSERVATION
        .scope(Cell::new(OperationObservation::default()), async {
            let result = store.put_immutable(object, bytes).await;
            (result, OBSERVATION.with(Cell::get).socket)
        })
        .await
}

pub(super) async fn first_get(
    store: &S3PrivateStore,
    object: &ObjectRef,
    writes: [Option<SocketOutcome>; 2],
) -> (Result<Vec<u8>, StorageError>, Option<SocketOutcome>) {
    let observation = OperationObservation {
        strict_get: true,
        ..OperationObservation::default()
    };
    OBSERVATION
        .scope(Cell::new(observation), async {
            let started_ms = unix_millis();
            let result = store.get_verified(object).await;
            let observation = OBSERVATION.with(Cell::get);
            let outcome = observation.get;
            if let Err(error) = &result {
                let ended_ms = unix_millis();
                eprintln!("G09_FIRST_GET_WINDOW unix_ms_start={started_ms} unix_ms_end={ended_ms}");
                eprintln!("{}", socket_line("initial-put", writes[0]));
                eprintln!("{}", socket_line("conflicting-put", writes[1]));
                eprintln!("{}", socket_line("first-get", observation.socket));
                match outcome {
                    Some(GetOutcome::Headers(status)) => {
                        let stage = if *error == StorageError::Unavailable {
                            "body-stream"
                        } else {
                            "integrity"
                        };
                        eprintln!("G09_FIRST_GET stage={stage} http_status={status:?}");
                    }
                    Some(GetOutcome::Failed {
                        status,
                        category,
                        service_code,
                        connector_kind,
                        io_kind,
                    }) => eprintln!(
                        "G09_FIRST_GET stage=sdk-send http_status={status:?} category={category} service_code={service_code} connector_kind={connector_kind} io_kind={io_kind:?}"
                    ),
                    None => eprintln!("G09_FIRST_GET stage=unobserved"),
                }
            } else {
                assert!(
                    matches!(outcome, Some(GetOutcome::Headers(Some(200)))),
                    "first GET diagnostic must observe the successful SDK response"
                );
            }
            (result, observation.socket)
        })
        .await
}

pub(super) fn report_healthy_witness(connection_close: bool, phases: [Option<SocketOutcome>; 3]) {
    use std::io::Write as _;

    let [initial, rejected, get] = [
        phases[0].expect("initial PUT must have live SDK evidence"),
        phases[1].expect("rejected PUT must have live SDK evidence"),
        phases[2].expect("first GET must have live SDK evidence"),
    ];
    assert_eq!(initial.status, Some(200));
    assert_eq!(rejected.status, Some(412));
    assert_eq!(get.status, Some(200));
    let initial_ports = initial.ports.expect("initial PUT must use loopback");
    let rejected_ports = rejected.ports.expect("rejected PUT must use loopback");
    let get_ports = get.ports.expect("first GET must use loopback");
    assert_eq!(initial_ports.1, rejected_ports.1);
    assert_eq!(rejected_ports.1, get_ports.1);
    let rejected_response_close = rejected.response_close.expect("412 response was observed");
    // Write only boolean comparisons; avoid both raw socket addresses and
    // libtest output capture for this single, explicit live witness.
    writeln!(
        std::io::stderr().lock(),
        "G09_SOCKET_WITNESS connection_close={connection_close} initial_put=200 conflicting_put=412 first_get=200 put_reused={} get_reused={} rejected_response_close={rejected_response_close}",
        initial_ports == rejected_ports,
        rejected_ports == get_ports,
    )
    .expect("write sanitized G-09 witness");
}

#[test]
#[ignore = "exercised by s3-provider with the existing --ignored suite"]
fn g09_reports_only_loopback_socket_metadata_and_distinguishes_absence() {
    use aws_smithy_runtime_api::client::connection::ConnectionMetadata;

    let local = "127.0.0.1:55664".parse().unwrap();
    let peer = "127.0.0.1:9000".parse().unwrap();
    let external = "192.0.2.1:9000".parse().unwrap();
    assert_eq!(loopback_ports(local, peer), Some((55664, 9000)));
    assert_eq!(loopback_ports(local, external), None);
    assert_eq!(loopback_ports(external, peer), None);
    let mut cfg = ConfigBag::base();
    assert_eq!(capture_ports(&cfg), None);
    let capture = CaptureSmithyConnection::new();
    capture.set_connection_retriever(move || {
        Some(
            ConnectionMetadata::builder()
                .proxied(false)
                .local_addr(local)
                .remote_addr(peer)
                .poison_fn(|| panic!("observation must not poison a socket"))
                .build(),
        )
    });
    cfg.interceptor_state().store_put(capture);
    assert_eq!(capture_ports(&cfg), Some((55664, 9000)));
    let captured = SocketOutcome {
        ports: capture_ports(&cfg),
        status: Some(412),
        response_close: Some(false),
        finished_ms: 42,
    };
    let observed = socket_line("conflicting-put", Some(captured));
    assert!(observed.contains("local_port=Some(55664) peer_port=Some(9000)"));
    assert!(observed.contains("http_status=Some(412) response_close=Some(false)"));
    assert!(observed.ends_with("unix_ms_end=Some(42)"));
    assert!(!observed.contains("127.0.0.1"));
    assert_eq!(
        socket_line("first-get", None),
        concat!(
            "G09_SOCKET phase=first-get local_port=None peer_port=None ",
            "http_status=None response_close=None unix_ms_end=None"
        )
    );
}
