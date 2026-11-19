use axum::{http::header, response::IntoResponse};

/// NEwBOO public design tokens, components and layout, kept separate from
/// the functioning reader/author styles until their port is independently tested.
pub async fn newboo_stylesheet() -> impl IntoResponse {
    static CSS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let mut css = String::from(include_str!("../../assets/newboo-port.css"));
        css.push('\n');
        css.push_str(include_str!("../../assets/newboo-home-carousel.css"));
        css.push('\n');
        css.push_str(include_str!("../../assets/newboo-book-detail.css"));
        css.push('\n');
        css.push_str(include_str!("../../assets/newboo-comments.css"));
        css.push('\n');
        css.push_str(include_str!("../../assets/newboo-support-widget.css"));
        css.push('\n');
        css.push_str(include_str!("../../assets/newboo-reader.css"));
        css.push('\n');
        css.push_str(include_str!("../../assets/newboo-archive-toc.css"));
        css.push('\n');
        css.push_str(include_str!("../../assets/newboo-layer-storefront.css"));
        css.push('\n');
        css.push_str(include_str!("../../assets/newboo-cover-picker.css"));
        css.push('\n');
        css.push_str(include_str!("../../assets/account-ui.css"));
        css.push('\n');
        css.push_str(include_str!("../../assets/author-layers.css"));
        css.push('\n');
        css.push_str(include_str!("../../assets/newboo-library.css"));
        css.push('\n');
        css.push_str(include_str!("../../assets/tor-appearance.css"));
        css
    });
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        CSS.as_str(),
    )
}

/// Public preferences script runs before CSS to apply the saved appearance without flash.
pub async fn appearance_script() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../../assets/tor-appearance.js"),
    )
}

/// UI-05 public-only home layout order. No account or analytics data is exposed.
pub async fn home_order_script() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        include_str!("../../assets/tor-home-order.js"),
    )
}
