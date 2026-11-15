//! Script-free package draft navigation and confirmation.
use super::*;

pub(super) fn stage_choices(form: &AddBookForm) -> Option<(Vec<i64>, Vec<i64>)> {
    let visible = super::csv_ids(form.visible_csv.as_deref())?;
    if visible.len() > usize::from(super::GRID_PAGE_SIZE) || form.choices.len() > visible.len() {
        return None;
    }
    let shown: BTreeSet<_> = visible.iter().copied().collect();
    let mut selected = BTreeSet::new();
    for (key, value) in &form.choices {
        let id = key.strip_prefix("chosen_")?.parse::<i64>().ok()?;
        if value != "1" || !shown.contains(&id) {
            return None;
        }
        selected.insert(id);
    }
    Some((visible, selected.into_iter().collect()))
}

/// Bounded navigation stack (a cursor per 50-book page). Zero represents first.
pub(super) fn history_ids(raw: Option<&str>) -> Option<Vec<i64>> {
    let raw = raw.unwrap_or("");
    if raw.len() > 1500 {
        return None;
    }
    if raw.is_empty() {
        return Some(Vec::new());
    }
    let cursors = raw
        .split(',')
        .map(|id| id.parse::<i64>().ok().filter(|id| *id >= 0))
        .collect::<Option<Vec<_>>>()?;
    (cursors.len() <= 100).then_some(cursors)
}

pub(super) fn valid_filter(filter: &str) -> bool {
    matches!(filter, "all" | "included" | "available")
}

fn draft_redirect(
    layer: i64,
    search: &str,
    filter: &str,
    after: Option<i64>,
    history: &[i64],
) -> Redirect {
    let search_suffix = super::encoded_search_suffix(search).replace("&amp;", "&");
    let cursor_suffix = after.map_or_else(String::new, |id| format!("&after={id}"));
    let history_suffix = if history.is_empty() {
        String::new()
    } else {
        format!(
            "&history={}",
            history
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    Redirect::to(&format!(
        "/me/layers/{layer}/books?filter={filter}{cursor_suffix}{history_suffix}{search_suffix}"
    ))
}

pub(super) async fn handle(
    service: &LayerService,
    actor: PositiveId,
    layer: PositiveId,
    raw_layer: i64,
    form: &AddBookForm,
) -> Option<Response> {
    if !matches!(
        form.operation.as_deref(),
        Some("discard_stale" | "stage" | "apply_draft")
    ) {
        return None;
    }
    Some(handle_draft(service, actor, layer, raw_layer, form).await)
}

async fn handle_draft(
    service: &LayerService,
    actor: PositiveId,
    layer: PositiveId,
    raw_layer: i64,
    form: &AddBookForm,
) -> Response {
    if form.operation.as_deref() == Some("discard_stale") {
        return match service.discard_stale_composition_draft(actor, layer).await {
            Ok(()) => private_response(Redirect::to(&format!("/me/layers/{raw_layer}/books")), &[]),
            Err(error) => deny(status(error)),
        };
    }
    if form.operation.as_deref() == Some("stage") {
        let Some(Ok(expected)) = form.expected_version_id.map(PositiveId::new) else {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        };
        let Some(revision) = form.draft_revision else {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        };
        let nav = form.nav.as_deref().unwrap_or("");
        let filter = form.filter.as_deref().unwrap_or("all");
        if !valid_filter(filter) {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        }
        if nav == "discard" {
            return match service
                .discard_composition_draft(actor, layer, revision)
                .await
            {
                Ok(()) => {
                    private_response(Redirect::to(&format!("/me/layers/{raw_layer}/books")), &[])
                }
                Err(error) => deny(status(error)),
            };
        }
        let Some((visible, selected)) = stage_choices(form) else {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        };
        let draft = match service
            .stage_composition_page(actor, layer, expected, revision, &visible, &selected)
            .await
        {
            Ok(draft) => draft,
            Err(error) => return deny(status(error)),
        };
        if nav == "review" {
            return match service
                .preview_composition(actor, layer, expected, &draft.additions, &draft.removals)
                .await
            {
                Ok(preview) => private_response(
                    grid::review(
                        layer.get(),
                        &preview,
                        form.csrf_token.as_deref().unwrap_or(""),
                        draft.revision,
                    ),
                    &[],
                ),
                Err(error) => deny(status(error)),
            };
        }
        let mut search = form.q.as_deref().unwrap_or("").trim();
        if search.chars().count() > 100 {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        }
        let mut target_filter = filter;
        let mut after = form.after.filter(|id| *id > 0);
        let Some(mut history) = history_ids(form.history_csv.as_deref()) else {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        };
        match nav {
            "save" => {}
            "search" => {
                after = None;
                history.clear();
            }
            "clear_search" => {
                search = "";
                after = None;
                history.clear();
            }
            "filter_all" => {
                target_filter = "all";
                after = None;
                history.clear();
            }
            "filter_included" => {
                target_filter = "included";
                after = None;
                history.clear();
            }
            "filter_available" => {
                target_filter = "available";
                after = None;
                history.clear();
            }
            "next" => {
                let Some(next) = form.next_after.filter(|id| *id > 0) else {
                    return deny(StatusCode::UNPROCESSABLE_ENTITY);
                };
                if history.len() >= 100 || after.is_some_and(|id| next <= id) {
                    return deny(StatusCode::UNPROCESSABLE_ENTITY);
                }
                history.push(after.unwrap_or(0));
                after = Some(next);
            }
            "previous" => {
                let Some(previous) = history.pop() else {
                    return deny(StatusCode::UNPROCESSABLE_ENTITY);
                };
                after = (previous > 0).then_some(previous);
            }
            _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
        }
        return private_response(
            draft_redirect(raw_layer, search, target_filter, after, &history),
            &[],
        );
    }
    if form.operation.as_deref() == Some("apply_draft") {
        let (Some(Ok(version)), Some(revision), Some(active), Some(lifetime)) = (
            form.expected_version_id.map(PositiveId::new),
            form.draft_revision,
            form.active_buyers,
            form.lifetime_buyers,
        ) else {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        };
        if form.confirm_composition.as_deref() != Some("yes") || !form.choices.is_empty() {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        }
        return match service
            .apply_composition_draft(actor, layer, version, revision, (active, lifetime))
            .await
        {
            Ok(_) => private_response(
                Redirect::to(&format!("/me/layers/{raw_layer}/books?updated=1")),
                &[],
            ),
            Err(error) => deny(status(error)),
        };
    }

    deny(StatusCode::UNPROCESSABLE_ENTITY)
}
