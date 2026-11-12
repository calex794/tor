//! Favicon assets are baked into the Rust binary and served without a database.
use axum::{
    http::header,
    response::{IntoResponse, Response},
};

fn asset(mime: &'static str, body: &'static [u8]) -> Response {
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "public, max-age=86400"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    )
        .into_response()
}

pub(crate) async fn favicon_ico() -> Response {
    asset("image/x-icon", include_bytes!("../assets/brand/favicon.ico"))
}

pub(crate) async fn favicon_svg() -> Response {
    asset("image/svg+xml", include_bytes!("../assets/brand/favicon.svg"))
}

pub(crate) async fn favicon_png() -> Response {
    asset(
        "image/png",
        include_bytes!("../assets/brand/favicon-32x32.png"),
    )
}

pub(crate) async fn apple_touch_icon() -> Response {
    asset(
        "image/png",
        include_bytes!("../assets/brand/apple-touch-icon.png"),
    )
}

pub(crate) async fn android_icon() -> Response {
    asset("image/png", include_bytes!("../assets/brand/icon-192.png"))
}

pub(crate) async fn manifest() -> Response {
    asset(
        "application/manifest+json; charset=utf-8",
        include_bytes!("../assets/brand/site.webmanifest"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    #[test]
    fn favicon_files_have_valid_headers_and_dimensions() {
        let ico = include_bytes!("../assets/brand/favicon.ico");
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 3, 0]);

        for (path, expected) in [
            (
                include_bytes!("../assets/brand/favicon-32x32.png").as_slice(),
                32,
            ),
            (
                include_bytes!("../assets/brand/apple-touch-icon.png").as_slice(),
                180,
            ),
            (
                include_bytes!("../assets/brand/icon-192.png").as_slice(),
                192,
            ),
        ] {
            let image = image::load_from_memory_with_format(path, image::ImageFormat::Png)
                .expect("valid embedded PNG");
            assert_eq!(image.width(), expected);
            assert_eq!(image.height(), expected);
        }

        let svg = include_str!("../assets/brand/favicon.svg");
        assert!(svg.contains("<svg"));
        assert!(svg.contains("viewBox=\"0 0 64 64\""));
        assert!(!svg.contains("<script"));

        let manifest: serde_json::Value =
            serde_json::from_slice(include_bytes!("../assets/brand/site.webmanifest"))
                .expect("valid site manifest");
        assert_eq!(manifest["icons"][0]["sizes"], "192x192");
    }

    #[test]
    fn public_and_standalone_reader_layouts_include_favicons() {
        for html in [
            include_str!("../templates/layout.html"),
            include_str!("../templates/reader_layout.html"),
        ] {
            for required in [
                "/favicon.ico",
                "/favicon.svg",
                "/favicon-32x32.png",
                "/apple-touch-icon.png",
                "/site.webmanifest",
            ] {
                assert!(html.contains(required), "missing {required}");
            }
        }
    }

    #[tokio::test]
    async fn public_icon_endpoints_return_valid_mime_and_bytes() {
        let router = Router::new()
            .route("/favicon.ico", get(favicon_ico))
            .route("/favicon.svg", get(favicon_svg))
            .route("/favicon-32x32.png", get(favicon_png))
            .route("/apple-touch-icon.png", get(apple_touch_icon))
            .route("/icon-192.png", get(android_icon))
            .route("/site.webmanifest", get(manifest));
        for (path, mime) in [
            ("/favicon.ico", "image/x-icon"),
            ("/favicon.svg", "image/svg+xml"),
            ("/favicon-32x32.png", "image/png"),
            ("/apple-touch-icon.png", "image/png"),
            ("/icon-192.png", "image/png"),
            ("/site.webmanifest", "application/manifest+json; charset=utf-8"),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(
                response.headers()[header::CONTENT_TYPE].to_str().unwrap(),
                mime,
                "{path}"
            );
            assert_eq!(
                response.headers()[header::X_CONTENT_TYPE_OPTIONS],
                "nosniff",
                "{path}"
            );
            let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
            assert!(!body.is_empty(), "{path}");
        }
    }
}
