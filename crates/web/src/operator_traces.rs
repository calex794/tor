//! Restricted operator diagnostics; no account-cookie or public admin authority.
use axum::{
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::{fmt::Write, path::PathBuf};
#[cfg(unix)]
use tor_observability::services::lookup::{self, LookupResult};

fn equal_secret(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() || a.len() < 32 || a.len() > 256 {
        return false;
    }
    a.iter()
        .zip(b)
        .fold(0_u8, |different, (left, right)| different | (left ^ right))
        == 0
}

/// Independent operator password until TOR has reviewed staff-role permissions.
/// Never accept the ordinary site cookie or a user-supplied trace ID as authority.
pub(crate) fn authorized(headers: &HeaderMap, secret: Option<&str>) -> bool {
    let Some(secret) = secret.filter(|value| {
        (32..=256).contains(&value.len()) && value.bytes().all(|byte| (33..=126).contains(&byte))
    }) else {
        return false;
    };
    let mut provided = headers.get_all(header::AUTHORIZATION).iter();
    let Some(value) = provided.next() else {
        return false;
    };
    if provided.next().is_some() {
        return false;
    }
    let Some(encoded) = value.to_str().ok().and_then(|v| v.strip_prefix("Basic ")) else {
        return false;
    };
    if encoded.len() > 384 {
        return false;
    }
    let Some(decoded) = STANDARD.decode(encoded).ok() else {
        return false;
    };
    let Some(password) = decoded.strip_prefix(b"operator:") else {
        return false;
    };
    equal_secret(password, secret.as_bytes())
}

pub(crate) fn secure<T: IntoResponse>(value: T) -> Response {
    let mut response = value.into_response();
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    h.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    h.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    response
}

pub(crate) fn denied() -> Response {
    let mut response = secure(StatusCode::UNAUTHORIZED);
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"TOR operator diagnostics\", charset=\"UTF-8\""),
    );
    response
}

fn parse_id(query: Option<&str>) -> Result<Option<(u128, String)>, ()> {
    let Some(raw) = query else {
        return Ok(None);
    };
    let Some(hex) = raw.strip_prefix("id=") else {
        return Err(());
    };
    if hex.len() != 32 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(());
    }
    let id = u128::from_str_radix(hex, 16).map_err(|_| ())?;
    Ok(Some((id, format!("{id:032x}"))))
}

fn document(id: &str, details: &str) -> Html<String> {
    Html(
        include_str!("../templates/operator_traces.html")
            .replace("<!--TRACE_ID-->", id)
            .replace("<!--DETAILS-->", details),
    )
}

#[cfg(unix)]
fn trace_details(result: &LookupResult) -> String {
    let Some((observed, trace)) = &result.found else {
        return format!(
            "<section class=\"card\" role=\"status\"><h2>Запись не обнаружена</h2><p>Сохранено трасс: {}. Совпадений в архиве: {}. Запрос мог не попасть в выборку, быть удалён по сроку хранения или потеряться при сбое передачи.</p></section>",
            result.archived, result.matches
        );
    };
    let mut details = format!(
        "<section class=\"card\" aria-labelledby=\"trace-info\"><h2 id=\"trace-info\">Трасса {:032x}</h2><dl><dt>Сохранена (Unix UTC)</dt><dd>{observed}</dd><dt>Семейство маршрута</dt><dd>{}</dd><dt>Статус</dt><dd>{}</dd><dt>HTTP</dt><dd>{:.2} мс</dd><dt>Всего в архиве</dt><dd>{}</dd></dl>",
        trace.trace_id,
        trace.route.label(),
        trace.status.label(),
        trace.duration_us as f64 / 1000.0,
        result.archived,
    );
    match &trace.sqlx {
        None => details
            .push_str("<p>SQLx не измерялась: это не означает, что запросов к базе не было.</p>"),
        Some(sql) => {
            write!(
                details,
                "<h3>Обращения к PostgreSQL (SQLx)</h3><dl><dt>Измерено операций</dt><dd>{}</dd><dt>Суммарная длительность</dt><dd>{:.2} мс</dd><dt>Не показано операций</dt><dd>{}</dd></dl>",
                sql.observed_queries,
                sql.observed_elapsed_us as f64 / 1000.0,
                sql.uncaptured_spans,
            )
            .expect("write String");
            if !sql.spans.is_empty() {
                details.push_str("<ol aria-label=\"Числовые SQLx-операции\">");
                for span in &sql.spans {
                    write!(
                        details,
                        "<li>Завершена через {:.2} мс от начала HTTP; длительность {:.2} мс.</li>",
                        span.completed_at_us as f64 / 1000.0,
                        span.elapsed_us as f64 / 1000.0
                    )
                    .expect("write String");
                }
                details.push_str("</ol>");
            }
            if sql.uncaptured_spans > 0 {
                details.push_str("<p>Часть SQLx-операций не вошла в подробную запись; сумма времени учитывает их.</p>");
            }
            if sql.observed_elapsed_us >= trace.duration_us / 2 {
                details.push_str("<p><strong>Возможное узкое место:</strong> обращения SQLx занимают значительное время. Это клиентские измерения, а не время выполнения SQL внутри PostgreSQL; параллельные операции могут перекрываться.</p>");
            }
        }
    }
    if trace.duration_us >= 1_000_000 {
        details.push_str("<p>HTTP-запрос длился более одной секунды. Причину следует искать по временным измерениям и другим журналам.</p>");
    }
    details.push_str("</section>");
    details
}

async fn respond(
    headers: HeaderMap,
    query: Option<&str>,
    secret: Option<&str>,
    socket: Option<PathBuf>,
) -> Response {
    if !authorized(&headers, secret) {
        return denied();
    }
    let parsed = match parse_id(query) {
        Ok(parsed) => parsed,
        Err(()) => return secure((StatusCode::BAD_REQUEST, document("", "<section class=\"card\" role=\"alert\">Неверный идентификатор запроса. Требуются ровно 32 шестнадцатеричных знака.</section>"))),
    };
    let Some((trace_id, display)) = parsed else {
        return secure(document("", "<section class=\"card\"><p>Вставьте значение заголовка <code>x-request-id</code> из HTTP-ответа.</p></section>"));
    };
    #[cfg(unix)]
    {
        let Some(socket) = socket.filter(|p| p.is_absolute()) else {
            return secure((
                StatusCode::SERVICE_UNAVAILABLE,
                document(&display, "<section class=\"card\" role=\"alert\">Служба хранения трасс не включена.</section>"),
            ));
        };
        let found = tokio::task::spawn_blocking(move || lookup::request(&socket, trace_id)).await;
        match found {
            Ok(Ok(found)) => {
                let status = if found.matches == 0 {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::OK
                };
                secure((status, document(&display, &trace_details(&found))))
            }
            _ => secure((
                StatusCode::SERVICE_UNAVAILABLE,
                document(&display, "<section class=\"card\" role=\"alert\">Архив временно недоступен. Это не означает, что трасса отсутствует.</section>"),
            )),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (trace_id, socket);
        secure((
            StatusCode::SERVICE_UNAVAILABLE,
            document(
                &display,
                "<section class=\"card\">Архив поддерживается только на Unix.</section>",
            ),
        ))
    }
}

/// Only an independent operator credential grants access, not a site login.
pub(crate) async fn page(headers: HeaderMap, uri: Uri) -> Response {
    let secret = std::env::var("TOR_OBS_OPERATOR_PASSWORD").ok();
    let socket = std::env::var_os("TOR_TRACE_STORE_SOCKET").map(PathBuf::from);
    respond(headers, uri.query(), secret.as_deref(), socket).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    fn credentials(password: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!(
                "Basic {}",
                STANDARD.encode(format!("operator:{password}"))
            ))
            .unwrap(),
        );
        headers
    }

    #[tokio::test]
    async fn operator_password_is_required_before_query_validation() {
        let secret = "a".repeat(48);
        for (headers, value) in [
            (HeaderMap::new(), Some(secret.as_str())),
            (credentials("wrong"), Some(secret.as_str())),
            (credentials(&secret), None),
            (credentials("short"), Some("short")),
        ] {
            let response = respond(headers, Some("id=<script>"), value, None).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));
        }
    }

    #[tokio::test]
    async fn valid_password_yields_form_but_unavailable_store_is_not_a_missing_trace() {
        let secret = "a".repeat(48);
        let response = respond(credentials(&secret), None, Some(&secret), None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key("content-security-policy"));
        let response = respond(
            credentials(&secret),
            Some("id=00000000000000000000000000000010"),
            Some(&secret),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let contents = to_bytes(response.into_body(), 20_000).await.unwrap();
        let body = String::from_utf8(contents.to_vec()).unwrap();
        assert!(body.contains("не включена"));
        assert!(!body.contains("user_id"));
    }

    #[tokio::test]
    async fn malformed_input_is_rejected_without_echo() {
        let secret = "a".repeat(48);
        for query in [
            "id=<script>",
            "id=1",
            "other=1",
            "id=11111111111111111111111111111111&role=admin",
        ] {
            let response = respond(credentials(&secret), Some(query), Some(&secret), None).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = to_bytes(response.into_body(), 20_000).await.unwrap();
            assert!(!String::from_utf8(body.to_vec())
                .unwrap()
                .contains("<script>"));
        }
    }

    #[test]
    fn password_has_to_be_strong_ascii_and_a_single_header() {
        let secret = "a".repeat(48);
        let mut header = credentials(&secret);
        assert!(authorized(&header, Some(&secret)));
        header.append(
            header::AUTHORIZATION,
            HeaderValue::from_static("Basic Zm9vOmJhcg=="),
        );
        assert!(!authorized(&header, Some(&secret)));
        assert!(!authorized(&credentials(&secret), Some("short")));
        assert!(!authorized(&credentials(&secret), Some("💠")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn authenticated_operator_reads_complete_root_and_sqlx_from_socket() {
        use std::{
            fs,
            io::{Read, Write},
            os::unix::net::UnixListener,
            thread,
            time::{SystemTime, UNIX_EPOCH},
        };
        use tor_observability::{
            domain::{RouteGroup, StatusGroup},
            services::{
                lookup::{self, LookupResult},
                trace::{RequestTrace, SqlEvidence, SqlSpan},
            },
        };

        let dir = std::env::temp_dir().join(format!(
            "tor-operator-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        let socket = dir.join("trace.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let worker = thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            let mut length = [0_u8; 4];
            peer.read_exact(&mut length).unwrap();
            assert_eq!(u32::from_be_bytes(length) as usize, lookup::QUERY_SIZE);
            let mut query = [0_u8; lookup::QUERY_SIZE];
            peer.read_exact(&mut query).unwrap();
            assert_eq!(lookup::parse_query(&query), Some(0xabcdef));
            let result = LookupResult {
                archived: 150,
                matches: 1,
                found: Some((
                    1_900_000_000,
                    RequestTrace {
                        trace_id: 0xabcdef,
                        route: RouteGroup::Reader,
                        status: StatusGroup::ServerError,
                        duration_us: 240_000,
                        sqlx: Some(SqlEvidence {
                            observed_queries: 1,
                            observed_elapsed_us: 175_000,
                            spans: vec![SqlSpan {
                                completed_at_us: 200_000,
                                elapsed_us: 175_000,
                            }],
                            uncaptured_spans: 0,
                        }),
                    },
                )),
            };
            let reply = lookup::encode_reply(&result).unwrap();
            peer.write_all(&(reply.len() as u32).to_be_bytes()).unwrap();
            peer.write_all(&reply).unwrap();
        });
        let secret = "a".repeat(48);
        let response = respond(
            credentials(&secret),
            Some("id=00000000000000000000000000abcdef"),
            Some(&secret),
            Some(socket),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 50_000).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("00000000000000000000000000abcdef"));
        assert!(html.contains("150"));
        assert!(html.contains("PostgreSQL (SQLx)"));
        assert!(html.contains("175.00 мс"));
        assert!(!html.contains("user_id"));
        worker.join().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
}
