//! Script-free selection actions. Session, Origin and CSRF are verified in parent route.
use super::{csv_ids, deny, encoded_search_suffix, grid, staged_changes, status, AddBookForm};
use crate::auth_cookies::private_response;
use axum::{http::StatusCode, response::{Redirect, Response}};
use tor_commerce::{repositories::layer_drafts::LayerStagePage, services::layers::LayerService};
use tor_contracts::paid_delivery::PositiveId;

fn selection_url(layer: i64, search: &str, after: Option<i64>) -> String {
    let mut url = format!("/me/layers/{layer}/books");
    if let Some(after) = after {
        url.push_str(&format!("?after={after}"));
    }
    let suffix = encoded_search_suffix(search).replace("&amp;", "&");
    if !suffix.is_empty() {
        if after.is_none() {
            url.push_str("?q=");
            url.push_str(suffix.trim_start_matches("&q="));
        } else {
            url.push_str(&suffix);
        }
    }
    url
}


pub(super) async fn handle_selection(
    service: &LayerService,
    actor: PositiveId,
    layer: PositiveId,
    raw_layer: i64,
    form: &AddBookForm,
) -> Response {
    if form.operation.as_deref() == Some("clear") {
        let (Some(Ok(version)), Some(revision)) = (
            form.expected_version_id.map(PositiveId::new), form.draft_revision,
        ) else {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        };
        return match service.clear_selection_draft(actor, layer, version, revision).await {
            Ok(()) => private_response(
                Redirect::to(&format!("/me/layers/{raw_layer}/books")), &[],
            ),
            Err(error) => deny(status(error)),
        };
    }
    if matches!(
        form.operation.as_deref(),
        Some("stage" | "search" | "next" | "reset" | "preview")
    ) {
        let (Some(Ok(expected)), Some(revision), Some(visible), Some(included)) = (
            form.expected_version_id.map(PositiveId::new),
            form.draft_revision,
            csv_ids(form.visible_csv.as_deref()),
            csv_ids(form.included_csv.as_deref()),
        ) else {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        };
        if staged_changes(&form).is_none() {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        }
        let page_after = match form.page_after.as_deref().filter(|value| !value.is_empty())
            .map(|value| value.parse::<i64>().map_err(|_| ()))
            .transpose().and_then(|number| number.map(PositiveId::new).transpose().map_err(|_| ()))
        {
            Ok(value) => value,
            Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
        };
        let mut selected=Vec::new();
        for key in form.choices.keys() {
            let Some(id)=key.strip_prefix("chosen_")
                .and_then(|raw| raw.parse::<i64>().ok())
            else {
                return deny(StatusCode::UNPROCESSABLE_ENTITY);
            };
            selected.push(id);
        }
        selected.sort_unstable();
        let original_query=form.page_query.as_deref().unwrap_or("").trim();
        let next_query=form.next_search.as_deref().unwrap_or("").trim();
        if original_query.chars().count()>100 || next_query.chars().count()>100
            || included.len()>visible.len()
        {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        }
        let staged=service.stage_selection_page(
            actor, layer, &LayerStagePage {
                version: expected, revision, after:page_after,
                query:original_query.to_owned(), visible, selected,
            },
        ).await;
        let (draft, page)=match staged {
            Ok(result)=>result,
            Err(error)=>return deny(status(error)),
        };
        if form.operation.as_deref()==Some("preview") {
            return match service
                .preview_composition(
                    actor, layer, expected, &draft.additions, &draft.removals,
                )
                .await
            {
                Ok(preview)=>private_response(
                    grid::review(
                        layer.get(), &preview,
                        form.csrf_token.as_deref().unwrap_or(""), draft.revision,
                    ), &[],
                ),
                Err(error)=>deny(status(error)),
            };
        }
        let destination=match form.operation.as_deref() {
            Some("search")=>selection_url(raw_layer,next_query,None),
            Some("reset")=>selection_url(raw_layer,"",None),
            Some("next")=>selection_url(raw_layer,original_query,page.next_after),
            _=>selection_url(raw_layer,original_query,page_after.map(PositiveId::get)),
        };
        return private_response(Redirect::to(&destination), &[]);
    }
    if form.operation.as_deref()==Some("apply") {
        let (Some(Ok(expected)), Some(revision))=(
            form.expected_version_id.map(PositiveId::new), form.draft_revision,
        ) else {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        };
        let (Some(adds),Some(removes),Some(active),Some(lifetime))=(
            csv_ids(form.add_csv.as_deref()), csv_ids(form.remove_csv.as_deref()),
            form.active_buyers, form.lifetime_buyers,
        ) else {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        };
        if form.confirm_composition.as_deref()!=Some("yes")
            || !form.choices.is_empty() || revision<=0
        {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        }
        return match service
            .apply_staged_composition(
                actor,layer,expected,&adds,&removes,((active,lifetime),revision),
            ).await
        {
            Ok(_)=>private_response(
                Redirect::to(&format!("/me/layers/{raw_layer}/books?updated=1")),&[],
            ),
            Err(error)=>deny(status(error)),
        };
    }
    super::deny(StatusCode::UNPROCESSABLE_ENTITY)
}
