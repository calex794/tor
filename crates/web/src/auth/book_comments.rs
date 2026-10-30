//! E-02/H: server-rendered comments for visible public books.
//! Chapter/paid-edition comments remain disabled until C/D grants are checked.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write;
use tor_accounts::services::security::generate_session_token;
use tor_discussions::{
    domain::{
        comments::{CommentBody, CommentIntentKey},
        spoilers::{mask_mentions, parse, Segment},
    },
    repositories::public_book_comments::{CreateResult, PgPublicBookComments, PublicComment},
};

pub(super) async fn mention_script() -> Response {
    private_response(
        (
            [(
                axum::http::header::CONTENT_TYPE,
                "text/javascript; charset=utf-8",
            )],
            include_str!("../../assets/mentions.js"),
        )
            .into_response(),
        &[],
    )
}

#[derive(Deserialize)]
pub(super) struct MentionsQuery {
    book_id: i64,
    q: String,
}

#[derive(Serialize)]
struct MentionOption {
    user_id: i64,
    label: String,
}

#[derive(Serialize)]
struct MentionOptions {
    items: Vec<MentionOption>,
}

pub(super) async fn discover_mentions(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<MentionsQuery>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let repo = PgPublicBookComments::new(state.accounts.pool().clone());
    match repo
        .mention_candidates(actor, query.book_id, &query.q)
        .await
    {
        Ok(items) => private_response(
            Json(MentionOptions {
                items: items
                    .into_iter()
                    .map(|item| MentionOption {
                        user_id: item.user_id,
                        label: item.display_name,
                    })
                    .collect(),
            })
            .into_response(),
            &[],
        ),
        Err(tor_discussions::repositories::public_book_comments::ListCommentsError::Invalid) => {
            deny(StatusCode::BAD_REQUEST)
        }
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[derive(Default, Deserialize)]
pub(super) struct CommentsQuery {
    after: Option<i64>,
}

#[derive(Deserialize)]
pub(super) struct CommentForm {
    body: String,
    command_key: String,
    parent_id: Option<i64>,
    csrf_token: Option<String>,
}

// Only our fixed markup is trusted; every user-controlled segment is escaped.
fn render_comment(body: &str) -> String {
    let mut html = String::new();
    for segment in parse(body) {
        match segment {
            Segment::Text(text) => html.push_str(&pages::escape_html(&mask_mentions(text))),
            Segment::Spoiler(text) => {
                html.push_str("<details class=\"comment-spoiler\"><summary class=\"comment-spoiler__toggle\">Спойлер</summary><span class=\"comment-spoiler__content\">");
                html.push_str(&pages::escape_html(&mask_mentions(text)));
                html.push_str("</span></details>");
            }
        }
    }
    html
}

fn comments_list(book_id: i64, comments: &[PublicComment], csrf: Option<&str>) -> String {
    if comments.is_empty() {
        return "<p class=\"comments-block__empty\">Комментариев пока нет.</p>".to_owned();
    }
    let mut output = String::from("<div class=\"comments-list\">");
    for comment in comments {
        let body = render_comment(&comment.body);
        let context = comment.parent_id.map_or_else(String::new, |id| {
            match id.checked_add(1) {
                Some(after) => format!("<p><a href=\"/books/{book_id}/comments?after={after}#comment-{id}\">Ответ на комментарий №{id}</a></p>"),
                None => format!("<p>Ответ на комментарий №{id}</p>"),
            }
        });
        let reply = csrf.map_or_else(String::new, |csrf| {
            format!(
                "<details class=\"comment-reply\"><summary>Ответить на комментарий №{id}</summary>
                 <form method=\"post\" action=\"/books/{book_id}/comments\" class=\"comments-form\">
                 <label for=\"reply-{id}\">Ваш ответ на комментарий №{id}</label>
                 <textarea data-mention-book=\"{book_id}\" class=\"ui-textarea comments-form__input\" id=\"reply-{id}\" name=\"body\" rows=\"3\" maxlength=\"2000\" required></textarea>
                 <input type=\"hidden\" name=\"parent_id\" value=\"{id}\">
                 <input type=\"hidden\" name=\"command_key\" value=\"{}\">
                 <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
                 <button class=\"ui-button ui-button--primary\" type=\"submit\">Отправить ответ</button>
                 </form></details>",
                generate_session_token(), pages::escape_html(csrf), id = comment.id,
            )
        });
        write!(
            output,
            "<article class=\"comment-card\" id=\"comment-{}\">
             <div class=\"comment-card__head\"><span class=\"comment-card__author\">Читатель · комментарий №{id}</span></div>
             {context}<div class=\"comment-card__body\">{body}</div>{reply}</article>",
            comment.id, id = comment.id
        )
        .expect("render comment");
    }
    output.push_str("</div>");
    if comments.len() == 25 {
        if let Some(last) = comments.last() {
            write!(
                output,
                "<nav aria-label=\"Страницы комментариев\"><a class=\"book-section-card__link\" href=\"/books/{book_id}/comments?after={}#comments\">Предыдущие комментарии</a></nav>",
                last.id
            )
            .expect("render pagination");
        }
    }
    output
}

fn comments_page(
    book_id: i64,
    title: &str,
    comments: &[PublicComment],
    csrf: Option<&str>,
) -> axum::response::Html<String> {
    let title = pages::escape_html(title);
    let form = match csrf {
        Some(csrf) => format!(
            "<form method=\"post\" action=\"/books/{book_id}/comments\" class=\"comments-form\">
             <label for=\"comment-body\">Ваш комментарий</label>
             <p id=\"spoiler-help\">Чтобы скрыть спойлер, заключите текст в ||двойные черты||.</p>
             <textarea data-mention-book=\"{book_id}\" class=\"ui-textarea comments-form__input\" id=\"comment-body\" aria-describedby=\"spoiler-help\" name=\"body\" rows=\"3\" maxlength=\"2000\" placeholder=\"Написать комментарий\" required></textarea>
             <input type=\"hidden\" name=\"command_key\" value=\"{}\">
             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
             <button class=\"ui-button ui-button--primary comments-form__submit\" type=\"submit\">Отправить</button>
             </form>",
            generate_session_token(),
            pages::escape_html(csrf),
        ),
        None => "<p><a class=\"ui-button ui-button--ghost ui-button--small comments-block__login\" href=\"/login\">Войти, чтобы оставить комментарий</a></p>".to_owned(),
    };
    let html = format!(
        "<section class=\"book-detail-page\">
         <div class=\"container book-detail-page__container\">
         <nav class=\"breadcrumbs\" aria-label=\"Навигационная цепочка\">
         <a href=\"/catalog\">Каталог</a><span aria-hidden=\"true\">/</span>
         <a href=\"/books/{book_id}\">Книга</a><span aria-hidden=\"true\">/</span>
         <span aria-current=\"page\">Обсуждение</span></nav>
         <section class=\"comments-block\" id=\"comments\" aria-labelledby=\"comments-title\">
         <div class=\"comments-block__head\"><div>
         <p class=\"comments-block__kicker\">Обсуждение произведения</p>
         <h1 class=\"comments-block__title\" id=\"comments-title\">Комментарии к книге «{title}»</h1>
         </div></div>
         {form}
         {list}
         </section></div></section><script defer src=\"/assets/mentions.js\"></script>",
        list = comments_list(book_id, comments, csrf),
    );
    pages::layout(
        "Обсуждение книги",
        "Комментарии читателей к произведению.",
        &html,
    )
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    Query(query): Query<CommentsQuery>,
    headers: HeaderMap,
) -> Response {
    if book_id <= 0 || query.after.is_some_and(|id| id <= 0) {
        return deny(StatusCode::NOT_FOUND);
    }
    let visible = match state.reader.public_book_page(book_id as u64, 1, None).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let store = PgPublicBookComments::new(state.accounts.pool().clone());
    let comments = match store.list(book_id, query.after, 25).await {
        Ok(Some(comments)) => comments,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let actor = match session_user(&state, &headers).await {
        Ok(actor) => actor,
        Err(status) => return deny(status),
    };
    let (token, cookies) = if actor.is_some() {
        let (token, cookies) = state.cookies.csrf_for_page(&headers);
        (Some(token), cookies)
    } else {
        (None, Vec::new())
    };
    let mut response = private_response(
        comments_page(book_id, &visible.title, &comments, token.as_deref()),
        &cookies,
    );
    // Permit only self-hosted JS on this editor page; all other private
    // pages retain their script-src 'none' policy.
    response.headers_mut().insert(
        "content-security-policy",
        "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'self'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'".parse().expect("static CSP"),
    );
    response
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<CommentForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !form
            .csrf_token
            .as_deref()
            .is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let body = match CommentBody::new(form.body) {
        Ok(body) => body,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let key = match CommentIntentKey::new(form.command_key) {
        Ok(key) => key,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    if book_id <= 0 {
        return deny(StatusCode::NOT_FOUND);
    }
    if form.parent_id.is_some_and(|id| id <= 0) {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    // Preserve existing root command hashes; reply identities include their parent.
    let payload = match form.parent_id {
        Some(parent) => format!("book:{book_id}\nreply:{parent}\n{}", body.as_str()),
        None => format!("book:{book_id}\n{}", body.as_str()),
    };
    let digest = Sha256::digest(payload.as_bytes());
    let store = PgPublicBookComments::new(state.accounts.pool().clone());
    match store
        .create_with_parent(
            actor,
            book_id,
            body.as_str(),
            key.as_str(),
            &digest[..],
            form.parent_id,
        )
        .await
    {
        CreateResult::Created(_) | CreateResult::Replayed(_) => private_response(
            Redirect::to(&format!("/books/{book_id}/comments#comments")),
            &[],
        ),
        CreateResult::Conflict => deny(StatusCode::CONFLICT),
        CreateResult::NotFound => deny(StatusCode::NOT_FOUND),
        CreateResult::Unavailable => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn e_04_spoiler_is_closed_escaped_and_mentions_are_masked() {
        let html =
            render_comment("до ||<img src=x onerror=alert(1)> @private_user @[user:42]|| после");
        assert!(html.contains("<details class=\"comment-spoiler\">"));
        assert!(!html.contains(" open"));
        assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(!html.contains("<img"));
        assert!(!html.contains("@private_user"));
        assert!(!html.contains("@[user:42]"));
        assert_eq!(render_comment("|||| ||не закрыт"), "|||| ||не закрыт");
        assert_eq!(render_comment("a\nб"), "a\nб");
    }

    #[test]
    fn e_02_page_escapes_comment_markup_and_uses_a_fresh_command_key() {
        let comments = vec![PublicComment {
            id: 12,
            parent_id: None,
            author_user_id: 1,
            body: "<script>alert(1)</script>\nПривет".to_owned(),
        }];
        let html = comments_page(7, "Книга <bad>", &comments, Some("csrf")).0;
        assert!(html.contains("Книга &lt;bad&gt;"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("name=\"command_key\""));
        assert!(!html.contains("author_user_id"));
        assert!(!html.contains("<script>alert(1)</script>"));
    }
}
