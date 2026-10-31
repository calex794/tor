//! Owner-only chapter TXT/Markdown/DOCX/ZIP import. Preview has no writes.
//! Confirm makes one all-or-nothing draft-book transaction via the B repository.
use super::{deny, session_actor, AuthorChapterState};
use crate::{auth_cookies::private_response, author_chapter_pages};
use axum::{
    extract::{Json, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Redirect, Response},
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use tor_accounts::services::security::generate_session_token;
use tor_books::services::{
    authoring::AuthorError,
    chapter_file_import::{fingerprint, parse_text, ImportError, ImportedChapter},
    docx_import::{extract_docx_text, valid_docx_filename, MAX_DOCX_BYTES},
    zip_import::{extract_zip_text, valid_zip_filename, MAX_ZIP_BYTES},
};

#[derive(Deserialize)]
pub(super) struct ImportInput {
    filename: String,
    text: String,
    #[serde(default)]
    file_base64: Option<String>,
    #[serde(default)]
    archive_encoding: Option<String>,
    csrf_token: String,
    #[serde(default)]
    command_key: Option<String>,
    #[serde(default)]
    fingerprint: Option<String>,
}

#[derive(Serialize)]
struct ChapterPreview {
    number: i32,
    title: String,
    characters: usize,
    excerpt: String,
}

#[derive(Serialize)]
struct PreviewResponse {
    fingerprint: String,
    count: usize,
    chapters: Vec<ChapterPreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extracted_text: Option<String>,
}

#[derive(Serialize)]
struct ConfirmResponse {
    saved: usize,
    book_url: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: &'static str,
}

/// A DOCX is decoded only on the authorized preview. Confirmation always
/// re-parses extracted UTF-8 text under the existing book-bound fingerprint.
fn preview_source(
    input: &ImportInput,
) -> Result<(Vec<ImportedChapter>, Option<String>), ImportError> {
    if let Some(encoded) = input.file_base64.as_deref() {
        let is_docx = valid_docx_filename(&input.filename);
        let is_zip = valid_zip_filename(&input.filename);
        if !input.text.is_empty() || !(is_docx || is_zip) {
            return Err(ImportError::InvalidFilename);
        }
        let max_bytes = if is_zip {
            MAX_ZIP_BYTES
        } else {
            MAX_DOCX_BYTES
        };
        if encoded.len() > max_bytes.div_ceil(3) * 4 {
            return Err(ImportError::TooLarge);
        }
        let bytes = BASE64
            .decode(encoded)
            .map_err(|_| ImportError::InvalidArchive)?;
        let extracted = if is_zip {
            extract_zip_text(&bytes, input.archive_encoding.as_deref().unwrap_or("utf-8"))?
        } else {
            extract_docx_text(&bytes)?
        };
        let chapters = parse_text("extracted.txt", &extracted)?;
        Ok((chapters, Some(extracted)))
    } else {
        if valid_docx_filename(&input.filename) || valid_zip_filename(&input.filename) {
            return Err(ImportError::InvalidArchive);
        }
        Ok((parse_text(&input.filename, &input.text)?, None))
    }
}

fn digest_hex(book_id: i64, chapters: &[ImportedChapter]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in fingerprint(book_id, chapters) {
        write!(&mut hex, "{byte:02x}").expect("write String");
    }
    hex
}

fn fail(status: StatusCode, message: &'static str) -> Response {
    private_response((status, Json(ErrorResponse { error: message })), &[])
}

fn import_error(error: ImportError) -> Response {
    let (code, message) = match error {
        ImportError::InvalidFilename => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Поддерживаются только безопасные файлы TXT, MD, DOCX и ZIP.",
        ),
        ImportError::InvalidEncoding => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Ошибка кодировки: требуется UTF-8 без повреждённых символов.",
        ),
        ImportError::TooLarge | ImportError::TooManyChapters => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "Файл, глава или количество глав превышают допустимый размер.",
        ),
        ImportError::InvalidArchive => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "DOCX или ZIP повреждён, содержит недопустимые файлы или опасные элементы.",
        ),
        ImportError::DuplicateNumber => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Повторяется номер главы. Исправьте файл и загрузите ещё раз.",
        ),
        ImportError::MissingTitle => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "У главы отсутствует название после номера.",
        ),
        ImportError::EmptyBody => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Обнаружена глава без текста.",
        ),
        ImportError::UnexpectedPreamble => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "До первой главы найден текст, который нельзя терять при импорте.",
        ),
        _ => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Не удалось найти корректные главы с номерами и названиями.",
        ),
    };
    fail(code, message)
}

pub(super) async fn page(
    State(state): State<AuthorChapterState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    // The handler explicitly repeats both guards for static RIL governance.
    let actor = match session_actor(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    match state.authoring.owned_book(actor, id).await {
        Ok(Some(_)) => (),
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let published = match state.authoring.owned_book_is_public(actor, id).await {
        Ok(value) => value,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if published {
        return private_response(
            (
                StatusCode::CONFLICT,
                "Импорт пока доступен только для черновика книги.",
            ),
            &[],
        );
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let key = generate_session_token();
    let mut response = private_response(
        author_chapter_pages::import_chapters(id, &csrf, &key),
        &cookies,
    );
    // Script is an external, static, same-origin file; no inline JavaScript.
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'self'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    response
}

pub(super) async fn preview(
    State(state): State<AuthorChapterState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(input): Json<ImportInput>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state.cookies.valid_csrf(&headers, &input.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    // The ownership proof is actor-bound and rechecked on each request.
    let actor = match session_actor(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match state.authoring.owned_book(actor, id).await {
        Ok(Some(_)) => (),
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    match state.authoring.owned_book_is_public(actor, id).await {
        Ok(false) => (),
        Ok(true) => {
            return fail(
                StatusCode::CONFLICT,
                "Пакетный импорт пока доступен только для неопубликованной книги.",
            );
        }
        Err(AuthorError::Missing) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let (chapters, extracted_text) = match preview_source(&input) {
        Ok(chapters) => chapters,
        Err(error) => return import_error(error),
    };
    let preview = chapters
        .iter()
        .map(|chapter| ChapterPreview {
            number: chapter.number,
            title: chapter.title.clone(),
            characters: chapter.body.chars().count(),
            excerpt: chapter.body.chars().take(160).collect(),
        })
        .collect();
    private_response(
        Json(PreviewResponse {
            fingerprint: digest_hex(id, &chapters),
            count: chapters.len(),
            chapters: preview,
            extracted_text,
        }),
        &[],
    )
}

pub(super) async fn confirm(
    State(state): State<AuthorChapterState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(input): Json<ImportInput>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state.cookies.valid_csrf(&headers, &input.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    // A confirmation cannot be forged for another book or another user.
    let actor = match session_actor(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match state.authoring.owned_book(actor, id).await {
        Ok(Some(_)) => (),
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    match state.authoring.owned_book_is_public(actor, id).await {
        Ok(false) => (),
        Ok(true) => {
            return fail(
                StatusCode::CONFLICT,
                "Пакетный импорт пока доступен только для неопубликованной книги.",
            );
        }
        Err(AuthorError::Missing) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    if input.file_base64.is_some() {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Сначала проверьте и подтвердите извлечённый текст DOCX или ZIP.",
        );
    }
    let chapters: Vec<ImportedChapter> = match parse_text(&input.filename, &input.text) {
        Ok(chapters) => chapters,
        Err(error) => return import_error(error),
    };
    let actual = digest_hex(id, &chapters);
    if input.fingerprint.as_deref() != Some(actual.as_str()) {
        return fail(
            StatusCode::CONFLICT,
            "Файл изменился после предпросмотра. Проверьте главы заново.",
        );
    }
    let Some(key) = input.command_key.as_deref() else {
        return fail(StatusCode::BAD_REQUEST, "Отсутствует ключ подтверждения.");
    };
    match state
        .authoring
        .import_draft_chapter_batch(actor, id, key, &chapters)
        .await
    {
        Ok(saved) => private_response(
            Json(ConfirmResponse {
                saved,
                book_url: format!("/me/books/{id}"),
            }),
            &[],
        ),
        Err(AuthorError::Conflict) => fail(
            StatusCode::CONFLICT,
            "Номер главы уже занят, книга опубликована или файл изменён. Ничего не сохранено.",
        ),
        Err(AuthorError::Invalid) => fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Файл содержит недопустимые номера, названия или текст.",
        ),
        Err(AuthorError::Missing) | Err(AuthorError::Forbidden) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Unavailable) => fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "База данных недоступна. Повторите с тем же файлом без изменения ключа.",
        ),
    }
}
