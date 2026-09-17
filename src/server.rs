//! Small middleware helpers that preserve macroni's selected error format.

use crate::Error;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::time::Duration;

/// Apply the runtime features enabled for generated routers.
#[doc(hidden)]
pub fn configure_router(router: axum::Router) -> axum::Router {
    #[cfg(feature = "gzip")]
    let router = router.layer(
        tower_http::compression::CompressionLayer::new()
            .no_br()
            .no_deflate()
            .no_zstd()
            .gzip(true),
    );
    router
}

/// Apply with `axum::middleware::from_fn_with_state(duration, timeout)`.
pub async fn timeout(State(duration): State<Duration>, request: Request, next: Next) -> Response {
    tokio::time::timeout(duration, next.run(request))
        .await
        .unwrap_or_else(|_| {
            Error::user(
                http::StatusCode::GATEWAY_TIMEOUT,
                "request_timeout",
                "The request exceeded its deadline",
            )
            .into_response()
        })
}

/// Configure Axum's request body limit. Extractor failures use the selected wire format.
pub fn body_limit(max_bytes: usize) -> axum::extract::DefaultBodyLimit {
    axum::extract::DefaultBodyLimit::max(max_bytes)
}
