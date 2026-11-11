use super::{administrator, render, secure, status, Runtime, PATH};
use axum::{
    extract::{
        rejection::{FormRejection, QueryRejection},
        Form, Query, State,
    },
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::time::Duration;
use tor_ads::{
    domain::editorial::{Action, CampaignSpec, Command},
    services::editorial::Editorial,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PageQuery {
    after: Option<String>,
    changed: Option<String>,
}
fn positive(value: &str) -> Result<i64, StatusCode> {
    if value.is_empty() || value.starts_with('0') || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or(StatusCode::BAD_REQUEST)
}
pub(super) async fn page(
    State(state): State<Runtime>,
    headers: HeaderMap,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Response {
    let operation = async {
        administrator(&state, &headers).await?;
        let Query(query) = query.map_err(|_| StatusCode::BAD_REQUEST)?;
        let after = query.after.as_deref().map(positive).transpose()?;
        let changed = query.changed.as_deref().map(positive).transpose()?;
        let report = Editorial::new(&state.store)
            .campaigns(after)
            .await
            .map_err(status)?;
        administrator(&state, &headers).await?;
        let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
        Ok::<_, StatusCode>(secure(
            render::page(&report, &csrf, state.enabled, changed),
            &cookies,
        ))
    };
    match tokio::time::timeout(Duration::from_millis(950), operation).await {
        Ok(Ok(response)) => response,
        Ok(Err(code)) => secure(code, &[]),
        Err(_) => secure(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CommandForm {
    csrf_token: String,
    command_key: String,
    action: Action,
    campaign_id: Option<String>,
    expected_revision: i64,
    book_id: Option<String>,
    slot: Option<String>,
    starts_at: Option<String>,
    ends_at: Option<String>,
    priority: Option<i16>,
}
impl CommandForm {
    fn command(&self) -> Result<Command, StatusCode> {
        let details = if matches!(self.action, Action::Create | Action::Update) {
            Some(CampaignSpec {
                book_id: positive(
                    self.book_id
                        .as_deref()
                        .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?,
                )?,
                slot: self
                    .slot
                    .as_deref()
                    .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?
                    .parse()
                    .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?,
                starts_at: self
                    .starts_at
                    .clone()
                    .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?,
                ends_at: self
                    .ends_at
                    .clone()
                    .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?,
                priority: self.priority.ok_or(StatusCode::UNPROCESSABLE_ENTITY)?,
            })
        } else {
            if self.book_id.is_some()
                || self.slot.is_some()
                || self.starts_at.is_some()
                || self.ends_at.is_some()
                || self.priority.is_some()
            {
                return Err(StatusCode::UNPROCESSABLE_ENTITY);
            }
            None
        };
        Ok(Command {
            action: self.action,
            campaign_id: self.campaign_id.as_deref().map(positive).transpose()?,
            expected_revision: self.expected_revision,
            details,
        })
    }
}
pub(super) async fn submit(
    State(state): State<Runtime>,
    headers: HeaderMap,
    form: Result<Form<CommandForm>, FormRejection>,
) -> Response {
    let operation = async {
        let actor = administrator(&state, &headers).await?;
        let Form(form) = form.map_err(|error| error.status())?;
        if !state.cookies.same_origin(&headers)
            || !state.cookies.valid_csrf(&headers, &form.csrf_token)
        {
            return Err(StatusCode::FORBIDDEN);
        }
        let command = form.command()?;
        let receipt = Editorial::new(&state.store)
            .execute(&actor, &form.command_key, &command)
            .await
            .map_err(status)?;
        Ok::<_, StatusCode>(secure(
            Redirect::to(&format!("{PATH}?changed={}", receipt.campaign_id)),
            &[],
        ))
    };
    match tokio::time::timeout(Duration::from_millis(950), operation).await {
        Ok(Ok(response)) => response,
        Ok(Err(code)) => secure(code, &[]),
        Err(_) => secure(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn administrative_ids_are_bounded_and_canonical() {
        for bad in ["", "0", "01", "-1", "1e3", "9223372036854775808"] {
            assert!(positive(bad).is_err());
        }
        assert_eq!(positive("42"), Ok(42));
    }
}
