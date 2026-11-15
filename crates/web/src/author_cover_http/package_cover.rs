//! Owner-only package-cover upload. Reuses the trusted cover pipeline.
use super::*;
use tor_commerce::repositories::layer_covers::{PackageCoverUpload, PgLayerCovers};
use tor_contracts::paid_delivery::PositiveId;
use tor_ingest::services::image_pipeline::{process_cover, ImageVariant};

fn image_variant<'a>(variants: &'a [ImageVariant], name: &str) -> Option<&'a ImageVariant> {
    variants.iter().find(|variant| variant.name == name)
}

pub(super) async fn upload(
    State(state): State<CoverUploadState>,
    Path(raw_layer): Path<i64>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if !state.cookies.same_origin(&headers) {
        return deny(StatusCode::FORBIDDEN);
    }
    let Ok(layer) = PositiveId::new(raw_layer) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let raw_actor = match state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
    {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let Ok(actor) = PositiveId::new(raw_actor) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let repo = PgLayerCovers::new(state.accounts.pool().clone());
    // Ownership is checked BEFORE allocating or decoding any submitted image.
    match repo.owner_can_upload(actor, layer).await {
        Ok(true) => {}
        Ok(false) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let Ok(_permit) = state.pending.clone().try_acquire_owned() else {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    };
    let mut csrf = false;
    let mut image = None;
    let mut field_count = 0;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => return deny(StatusCode::BAD_REQUEST),
        };
        field_count += 1;
        if field_count > 2 {
            return deny(StatusCode::BAD_REQUEST);
        }
        match field.name() {
            Some("csrf_token") if !csrf => {
                let token = match field.text().await {
                    Ok(value) if value.len() <= 128 => value,
                    _ => return deny(StatusCode::BAD_REQUEST),
                };
                if !state.cookies.valid_csrf(&headers, &token) {
                    return deny(StatusCode::FORBIDDEN);
                }
                csrf = true;
            }
            Some("cover") if csrf && image.is_none() => {
                let kind = match field.content_type() {
                    Some("image/png") => "image/png",
                    Some("image/jpeg") => "image/jpeg",
                    Some("image/webp") => "image/webp",
                    _ => return deny(StatusCode::UNSUPPORTED_MEDIA_TYPE),
                };
                let bytes = match field.bytes().await {
                    Ok(bytes) if !bytes.is_empty() && bytes.len() <= MAX_SOURCE_BYTES => bytes,
                    _ => return deny(StatusCode::PAYLOAD_TOO_LARGE),
                };
                image = Some((kind, bytes));
            }
            _ => return deny(StatusCode::BAD_REQUEST),
        }
    }
    if !csrf {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some((kind, bytes)) = image else {
        return deny(StatusCode::BAD_REQUEST);
    };
    // CPU-heavy decoding, validation and image transformation stay off Tokio.
    // A source cannot choose dimensions, content type, S3 paths or variant name.
    let generated = match tokio::time::timeout(
        Duration::from_secs(15),
        tokio::task::spawn_blocking(move || process_cover(&bytes, kind)),
    )
    .await
    {
        Ok(Ok(Ok(variants))) => variants,
        _ => return deny(StatusCode::UNSUPPORTED_MEDIA_TYPE),
    };
    let (Some(card), Some(main)) = (
        image_variant(&generated, "card"),
        image_variant(&generated, "main"),
    ) else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    // Private S3 key is server-generated and cannot be submitted in a form.
    let opaque = generate_session_token();
    let base = format!("staging/covers/variants/packages/{raw_layer}/{opaque}");
    let card_key = format!("{base}/card.webp");
    let main_key = format!("{base}/main.webp");
    let card_digest: [u8; 32] = Sha256::digest(&card.bytes).into();
    let main_digest: [u8; 32] = Sha256::digest(&main.bytes).into();
    let upload = PackageCoverUpload {
        card_key: &card_key,
        main_key: &main_key,
        card_sha256: card_digest,
        main_sha256: main_digest,
        card_bytes: card.bytes.len() as i64,
        main_bytes: main.bytes.len() as i64,
    };
    let saved = match repo.reserve(actor, layer, &upload).await {
        Ok(id) => id,
        Err(tor_commerce::domain::layers::LayerError::StaleOffer) => {
            return deny(StatusCode::CONFLICT);
        }
        Err(tor_commerce::domain::layers::LayerError::Inaccessible) => {
            return deny(StatusCode::NOT_FOUND);
        }
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        state.storage.put_verified(&card_key, &card.bytes).await?;
        state.storage.put_verified(&main_key, &main.bytes).await
    })
    .await;
    if !matches!(result, Ok(Ok(()))) {
        if let Ok(candidate) = PositiveId::new(saved) {
            let _ = repo.mark_failed(actor, candidate).await;
        }
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    }
    let Ok(candidate) = PositiveId::new(saved) else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    match repo.mark_pending(actor, candidate).await {
        Ok(()) => private_response(
            Redirect::to(&format!("/me/layers/{raw_layer}/books?cover=queued")),
            &[],
        ),
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_cover_requires_card_and_main_variants() {
        assert!(image_variant(&[], "card").is_none());
        assert!(image_variant(&[], "main").is_none());
        assert_eq!(MAX_SOURCE_BYTES, 8 * 1024 * 1024);
    }
}
