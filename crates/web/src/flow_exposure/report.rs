use super::Runtime;
use crate::auth_cookies::private_response;
use axum::{
    extract::{rejection::QueryRejection, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use tor_accounts::services::security::session_token_digest;
use tor_recommendations::domain::exposure::{ExposureError, ExposureFilter, ObservationClass};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    days: Option<i32>,
    surface: Option<String>,
    source: Option<String>,
    campaign: Option<String>,
    after: Option<i64>,
    class: Option<String>,
    group: Option<String>,
    label: Option<String>,
}
impl Selection {
    fn filter(self) -> Result<ExposureFilter, ExposureError> {
        let surface = self
            .surface
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().map_err(|_| ExposureError::Invalid))
            .transpose()?;
        let editorial = match self.source.as_deref().unwrap_or("") {
            "" => None,
            "editorial" => Some(true),
            "organic" => Some(false),
            _ => return Err(ExposureError::Invalid),
        };
        let class = match self.class.as_deref().unwrap_or("") {
            "" => None,
            "unknown" => Some(ObservationClass::Unknown),
            "low" => Some(ObservationClass::LowObserved),
            "observed" => Some(ObservationClass::Observed),
            _ => return Err(ExposureError::Invalid),
        };
        let group = match self.group.as_deref().unwrap_or("") {
            "" => {
                if self.label.as_ref().is_some_and(|l| !l.is_empty()) {
                    return Err(ExposureError::Invalid);
                }
                None
            }
            "genre" | "tag" => Some((self.group.unwrap(), self.label.unwrap_or_default())),
            _ => return Err(ExposureError::Invalid),
        };
        let campaign = self
            .campaign
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().map_err(|_| ExposureError::Invalid))
            .transpose()?;
        if campaign.is_some() && editorial == Some(false) {
            return Err(ExposureError::Invalid);
        }
        let filter = ExposureFilter {
            days: self.days.unwrap_or(7),
            surface,
            after: self.after.unwrap_or(0),
            class,
            group,
            campaign,
            editorial,
        };
        filter.validate()?;
        Ok(filter)
    }
}
fn secure(value: impl IntoResponse) -> Response {
    let mut response = private_response(value, &[]);
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}
pub(super) async fn page(
    State(state): State<Runtime>,
    headers: HeaderMap,
    query: Result<Query<Selection>, QueryRejection>,
) -> Response {
    let deadline = Instant::now() + Duration::from_millis(950);
    let auth = async {
        let token = state
            .cookies
            .session_token(&headers)
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let actor = state
            .accounts
            .session_actor(&session_token_digest(&token))
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .ok_or(StatusCode::UNAUTHORIZED)?;
        if !actor.administrator {
            return Err(StatusCode::FORBIDDEN);
        }
        Ok(())
    };
    match timeout_at(deadline, auth).await {
        Ok(Ok(())) => {}
        Ok(Err(status)) => return secure(status),
        Err(_) => return secure(StatusCode::SERVICE_UNAVAILABLE),
    }
    let filter = match query
        .map_err(|_| ExposureError::Invalid)
        .and_then(|Query(q)| q.filter())
    {
        Ok(filter) => filter,
        Err(_) => return secure(StatusCode::BAD_REQUEST),
    };
    let campaign_book = if let Some(campaign) = filter.campaign {
        let Some(guard) = state.guard.as_ref() else {
            return secure((
                StatusCode::SERVICE_UNAVAILABLE,
                super::render::page(&filter, None, state.queue_snapshot()),
            ));
        };
        match timeout_at(deadline, guard.campaign_book(campaign)).await {
            Ok(Ok(Some(book))) if book > 0 => Some(book),
            Ok(Ok(None)) => return secure(StatusCode::NOT_FOUND),
            _ => {
                return secure((
                    StatusCode::SERVICE_UNAVAILABLE,
                    super::render::page(&filter, None, state.queue_snapshot()),
                ))
            }
        }
    } else {
        None
    };
    let query = async {
        match campaign_book {
            Some(book) => state.store.report_campaign_book(&filter, book).await,
            None => state.store.report(&filter).await,
        }
    };
    let report = timeout_at(deadline, query).await.ok().and_then(Result::ok);
    secure((
        if report.is_some() {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        super::render::page(&filter, report.as_ref(), state.queue_snapshot()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_rejects_unbounded_and_identity_fields() {
        for raw in [
            "days=0",
            "days=365",
            "surface=library",
            "source=paid",
            "campaign=-1",
            "class=unique",
            "group=genre&label=%0A",
            "reader=1",
            "days=7&days=1",
            "source=organic&campaign=1",
        ] {
            let uri = format!("/admin/audience/exposure?{raw}").parse().unwrap();
            assert!(
                Query::<Selection>::try_from_uri(&uri)
                    .map_err(|_| ExposureError::Invalid)
                    .and_then(|Query(q)| q.filter())
                    .is_err(),
                "{raw}"
            );
        }
    }
}
