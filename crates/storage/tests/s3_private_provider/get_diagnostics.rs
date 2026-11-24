use aws_sdk_s3::{
    config::{
        interceptors::{BeforeTransmitInterceptorContextMut, FinalizerInterceptorContextRef},
        ConfigBag, Intercept, RuntimeComponents,
    },
    error::ProvideErrorMetadata,
    operation::get_object::GetObjectError,
};
use std::{
    cell::Cell,
    error::Error,
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

tokio::task_local! {
    static FIRST_GET_OUTCOME: Cell<Option<GetOutcome>>;
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
        _: &mut ConfigBag,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // Only the strict GET runs in this scope. Background load uses the same
        // client from other tasks and cannot replace its evidence.
        let _ = FIRST_GET_OUTCOME.try_with(|slot| {
            let status = context
                .response()
                .map(|response| response.status().as_u16());
            slot.set(context.output_or_error().map(|outcome| match outcome {
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
            }));
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

pub(super) async fn first_get(
    store: &S3PrivateStore,
    object: &ObjectRef,
) -> Result<Vec<u8>, StorageError> {
    FIRST_GET_OUTCOME
        .scope(Cell::new(None), async {
            let started_ms = unix_millis();
            let result = store.get_verified(object).await;
            let outcome = FIRST_GET_OUTCOME.with(Cell::get);
            if let Err(error) = &result {
                let ended_ms = unix_millis();
                eprintln!("G09_FIRST_GET_WINDOW unix_ms_start={started_ms} unix_ms_end={ended_ms}");
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
            result
        })
        .await
}
